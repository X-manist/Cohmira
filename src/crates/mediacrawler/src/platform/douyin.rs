//! 抖音采集器（纯 HTTP + cookie + msToken；a_bogus 签名路径已接好）。
//!
//! 移植自 `MediaCrawler/media_platform/douyin/client.py` 的 `DouYinClient`。
//! 搜索端点 `/aweme/v1/web/general/search/single/`，参数构造对齐
//! `search_info_by_keyword` + `__process_req_params`（common params + msToken）。
//!
//! ## a_bogus 签名说明（重要：忠实移植 Python）
//!
//! `client.py::__process_req_params` 中：
//! ```text
//! if "/v1/web/general/search" not in uri:
//!     a_bogus = await get_a_bogus(...); params["a_bogus"] = a_bogus
//! ```
//! 即**搜索接口本身不附加 a_bogus**（仅详情/评论/用户等接口签名）。
//! 本模块用 [`should_sign_uri`] 编码此规则：搜索 URI 返回 `false`，跳过签名。
//! 签名路径本身已完整接好（[`DouyinCrawler::sign_query`] 懒加载 [`DouyinSign`] 并复用），
//! 仅被该开关门控；若后续抖音对搜索也要求 a_bogus，把 [`should_sign_uri`] 改为恒 `true` 即可。
//!
//! ## 设计
//!
//! 遵循 bilibili/weibo 模式：`build_*`（纯函数）+ `parse_*`（纯函数，fixture 单测）
//! + `struct DouyinCrawler { http: reqwest::Client, ... }` + `impl PlatformCrawler`。

use crate::model::{Content, ContentType, Creator, Platform};
use crate::signing::douyin::DouyinSign;
use serde::Deserialize;

const HOST: &str = "https://www.douyin.com";
/// 搜索端点（对齐 `client.py::search_info_by_keyword` 的 uri）。
pub const SEARCH_URI: &str = "/aweme/v1/web/general/search/single/";
/// 搜索来源固定分组（对齐 `client.py`）。
const FROM_GROUP_ID: &str = "7378810571505847586";
/// 每页条数（对齐 `client.py` 的 `count`）。
const PAGE_COUNT: usize = 15;
/// 搜索频道（`SearchChannelType.GENERAL = "aweme_general"`）。
const SEARCH_CHANNEL: &str = "aweme_general";

/// 默认 UA：Mac Chrome 125，与 [`common_params`] 的 `browser_version`/`os_name` 一致。
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// 搜索排序（对齐 `field.py::SearchSortType`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSort {
    /// 综合排序（0）。
    General,
    /// 最多点赞（1）。
    MostLiked,
    /// 最新发布（2）。
    Latest,
}
impl SearchSort {
    fn code(self) -> i64 {
        match self {
            SearchSort::General => 0,
            SearchSort::MostLiked => 1,
            SearchSort::Latest => 2,
        }
    }
}

/// 发布时间过滤（对齐 `field.py::PublishTimeType`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishTime {
    Unlimited,
    OneDay,
    OneWeek,
    SixMonth,
}
impl PublishTime {
    fn code(self) -> i64 {
        match self {
            PublishTime::Unlimited => 0,
            PublishTime::OneDay => 1,
            PublishTime::OneWeek => 7,
            PublishTime::SixMonth => 180,
        }
    }
}

/// 抖音采集器。
pub struct DouyinCrawler {
    http: reqwest::Client,
    /// `Cookie` 头完整值（登录态）。None 时匿名请求。
    cookie: Option<String>,
    /// `msToken`（对应 Python `localStorage.xmst`）。None 时为空。
    ms_token: Option<String>,
    /// webid 指纹。空时每次请求用 [`generate_web_id`] 现生成。
    web_id: String,
    /// 签名所用 UA。
    user_agent: String,
    /// 懒加载的签名器（boa JsEngine 较重，按需创建并复用）。
    /// `Mutex` 因 [`DouyinSign::get_a_bogus`] 需 `&mut self`。
    signer: std::sync::Mutex<Option<DouyinSign>>,
}

impl DouyinCrawler {
    /// 用给定 HTTP client 构造（cookie/msToken/webid 后续注入）。
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            cookie: None,
            ms_token: None,
            web_id: String::new(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            signer: std::sync::Mutex::new(None),
        }
    }

    /// 注入 `Cookie` 头（含登录态）。
    pub fn with_cookie(mut self, cookie: impl Into<String>) -> Self {
        self.cookie = Some(cookie.into());
        self
    }

    /// 注入 `msToken`。
    pub fn with_ms_token(mut self, ms_token: impl Into<String>) -> Self {
        self.ms_token = Some(ms_token.into());
        self
    }

    /// 注入固定 webid（否则每请求随机生成）。
    pub fn with_web_id(mut self, web_id: impl Into<String>) -> Self {
        self.web_id = web_id.into();
        self
    }

    /// 注入签名 UA（默认 [`DEFAULT_USER_AGENT`]）。
    pub fn with_user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// 对 (uri, query) 计算 a_bogus（懒加载并复用 [`DouyinSign`]）。
    /// 仅由非搜索接口（[`should_sign_uri`] 为 true）调用。
    fn sign_query(&self, uri: &str, query: &str, ua: &str) -> anyhow::Result<String> {
        let mut guard = self
            .signer
            .lock()
            .map_err(|e| anyhow::anyhow!("douyin signer lock poisoned: {e}"))?;
        if guard.is_none() {
            *guard = Some(DouyinSign::new()?);
        }
        // as_mut 一定 Some，但用 ok_or 兜底避免 panic。
        guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("douyin signer 未初始化"))?
            .get_a_bogus(uri, query, ua)
    }

    /// 按关键词搜索（综合排序、不限发布时间）。`limit` 截断返回条数。
    ///
    /// v1 单页（offset=0, count=[PAGE_COUNT]），未实现分页（与 v1 小样本范围一致）。
    pub async fn search_videos(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        let web_id = if self.web_id.is_empty() {
            generate_web_id()
        } else {
            self.web_id.clone()
        };
        let ms_token = self.ms_token.as_deref().unwrap_or("");
        let params = build_search_params(
            keyword,
            0,
            SearchSort::General,
            PublishTime::Unlimited,
            ms_token,
            &web_id,
        );
        let query_string = urlencode(&params);
        // 忠实移植：搜索接口跳过 a_bogus（见 should_sign_uri）。
        let a_bogus = if should_sign_uri(SEARCH_URI) {
            Some(self.sign_query(SEARCH_URI, &query_string, &self.user_agent)?)
        } else {
            None
        };
        let url = build_search_url(SEARCH_URI, &params, a_bogus.as_deref());

        let mut req = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, self.user_agent.as_str())
            .header(
                reqwest::header::REFERER,
                format!("https://www.douyin.com/search/{}", quote_plus(keyword)),
            );
        if let Some(cookie) = self.cookie.as_ref() {
            req = req.header(reqwest::header::COOKIE, cookie.clone());
        }
        let resp: SearchResp = req.send().await?.error_for_status()?.json().await?;
        let items = parse_search_results(&resp, keyword);
        Ok(items.into_iter().take(limit).collect())
    }
}

#[async_trait::async_trait]
impl crate::platform::PlatformCrawler for DouyinCrawler {
    fn name(&self) -> &'static str {
        "douyin"
    }
    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        self.search_videos(keyword, limit).await
    }
}

// ================================ 纯函数（可单测） ================================

/// 是否对该 URI 做 a_bogus 签名。
///
/// 忠实移植 `client.py::__process_req_params`：
/// `if "/v1/web/general/search" not in uri:` 才签名 ——
/// 故搜索接口（含 `/v1/web/general/search`）返回 `false`，其余返回 `true`。
/// 若需对搜索也签名，把本函数改为恒 `true`。
pub fn should_sign_uri(uri: &str) -> bool {
    !uri.contains("/v1/web/general/search")
}

/// common params（24 项），1:1 对齐 `__process_req_params`。
/// `ms_token`/`web_id` 由调用方注入。
pub fn common_params(ms_token: &str, web_id: &str) -> Vec<(String, String)> {
    vec![
        ("device_platform".into(), "webapp".into()),
        ("aid".into(), "6383".into()),
        ("channel".into(), "channel_pc_web".into()),
        ("version_code".into(), "190600".into()),
        ("version_name".into(), "19.6.0".into()),
        ("update_version_code".into(), "170400".into()),
        ("pc_client_type".into(), "1".into()),
        ("cookie_enabled".into(), "true".into()),
        ("browser_language".into(), "zh-CN".into()),
        ("browser_platform".into(), "MacIntel".into()),
        ("browser_name".into(), "Chrome".into()),
        ("browser_version".into(), "125.0.0.0".into()),
        ("browser_online".into(), "true".into()),
        ("engine_name".into(), "Blink".into()),
        ("os_name".into(), "Mac OS".into()),
        ("os_version".into(), "10.15.7".into()),
        ("cpu_core_num".into(), "8".into()),
        ("device_memory".into(), "8".into()),
        ("engine_version".into(), "109.0".into()),
        ("platform".into(), "PC".into()),
        ("screen_width".into(), "2560".into()),
        ("screen_height".into(), "1440".into()),
        ("effective_type".into(), "4g".into()),
        ("round_trip_time".into(), "50".into()),
        ("webid".into(), web_id.to_string()),
        ("msToken".into(), ms_token.to_string()),
    ]
}

/// 构造搜索请求参数（search-specific + common 合并）。
///
/// 顺序对齐 Python：`query_params`（search-specific）在前，`common_params.update` 追加在后。
/// 当 `sort`/`publish_time` 任一非默认时，追加 `filter_selected` 并置 `is_filter_search=1`。
pub fn build_search_params(
    keyword: &str,
    offset: usize,
    sort: SearchSort,
    publish_time: PublishTime,
    ms_token: &str,
    web_id: &str,
) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = vec![
        ("search_channel".into(), SEARCH_CHANNEL.into()),
        ("enable_history".into(), "1".into()),
        ("keyword".into(), keyword.to_string()),
        ("search_source".into(), "tab_search".into()),
        ("query_correct_type".into(), "1".into()),
        ("is_filter_search".into(), "0".into()),
        ("from_group_id".into(), FROM_GROUP_ID.into()),
        ("offset".into(), offset.to_string()),
        ("count".into(), PAGE_COUNT.to_string()),
        ("need_filter_settings".into(), "1".into()),
        ("list_type".into(), "multi".into()),
        ("search_id".into(), String::new()),
    ];
    if sort != SearchSort::General || publish_time != PublishTime::Unlimited {
        let filter = serde_json::json!({
            "sort_type": sort.code().to_string(),
            "publish_time": publish_time.code().to_string(),
        })
        .to_string();
        params.push(("filter_selected".into(), filter));
        // Python 中覆盖既有项；这里用同键去重保留最后一次值。
        params.push(("is_filter_search".into(), "1".into()));
        params.push(("search_source".into(), "tab_search".into()));
    }
    params.extend(common_params(ms_token, web_id));
    dedup_keep_last(params)
}

/// 构造已（可选）签名的搜索 URL（纯函数）。
///
/// `params` 经 [`urlencode`] 编码；`a_bogus`（若 `Some`）追加为 `&a_bogus=...`。
pub fn build_search_url(uri: &str, params: &[(String, String)], a_bogus: Option<&str>) -> String {
    let mut qs = urlencode(params);
    if let Some(ab) = a_bogus {
        qs.push_str("&a_bogus=");
        qs.push_str(&quote_plus(ab));
    }
    format!("{HOST}{uri}?{qs}")
}

/// 解析搜索响应为归一化 [`Content`]（纯函数，便于 fixture 单测）。
///
/// 跳过无 `aweme_info`（广告/用户卡片等）的 data 项。图文笔记（`images` 非空）
/// 映射为 [`ContentType::Image`]，否则 [`ContentType::Video`]。
pub fn parse_search_results(resp: &SearchResp, keyword: &str) -> Vec<Content> {
    resp.data
        .iter()
        .filter_map(|d| d.aweme_info.as_ref().map(|a| (d, a)))
        .map(|(_, a)| aweme_to_content(a, keyword))
        .collect()
}

/// 单个 aweme → [`Content`]。
fn aweme_to_content(a: &AwemeInfo, keyword: &str) -> Content {
    let desc = a.desc.trim();
    let title: String = desc
        .split('\n')
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| strip_html(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "抖音内容".to_string());

    let is_image_note = !a.images.is_empty();
    let content_type = if is_image_note {
        ContentType::Image
    } else {
        ContentType::Video
    };

    let media_urls = if is_image_note {
        a.images
            .iter()
            .flat_map(|img| img.url_list.iter().cloned())
            .collect::<Vec<_>>()
    } else {
        a.video
            .as_ref()
            .and_then(|v| v.cover.as_ref())
            .map(|c| c.url_list.clone())
            .unwrap_or_default()
    };

    let mut tags: Vec<String> = a
        .text_extra
        .iter()
        .filter_map(|t| t.hashtag_name.clone())
        .filter(|s| !s.is_empty())
        .collect();
    if tags.is_empty() {
        tags.push(keyword.to_string());
    }

    let author = a.author.as_ref().map(|au| Creator {
        platform: Platform::Douyin,
        platform_user_id: json_to_string(&au.uid),
        nickname: au.nickname.clone(),
        avatar: au
            .avatar_thumb
            .as_ref()
            .and_then(|at| at.url_list.first().cloned()),
        desc: None,
        fans_count: None,
        follows_count: None,
        note_count: None,
    });

    let stats = a.statistics.as_ref();

    Content {
        platform: Platform::Douyin,
        content_type,
        platform_id: a.aweme_id.clone(),
        url: Some(format!("https://www.douyin.com/video/{}", a.aweme_id)),
        title,
        desc: if desc.is_empty() {
            None
        } else {
            Some(desc.to_string())
        },
        author,
        published_at: if a.create_time > 0 {
            Some(a.create_time)
        } else {
            None
        },
        liked_count: stats.and_then(|s| s.digg_count),
        comment_count: stats.and_then(|s| s.comment_count),
        collected_count: stats.and_then(|s| s.collect_count),
        share_count: stats.and_then(|s| s.share_count),
        tags,
        media_urls,
    }
}

/// 生成 webid：19 位数字指纹（移植 `help.py::get_web_id` 的形态）。
///
/// 具体随机值不要求与 Python 一致（随机指纹）；用时间种子的 xorshift*
/// 生成，避免引入 `rand` 依赖。首位非 0，更贴近真实 webid 形态（如 `7362810250930783783`）。
pub fn generate_web_id() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let mut state: u64 = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x6D2B_79F5);
    let mut out = String::with_capacity(19);
    for i in 0..19usize {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let mut d: u32 = (((state.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 33) as u32) % 10;
        if i == 0 && d == 0 {
            d = 1 + (((state >> 33) as u32) % 9);
        }
        out.push(char::from_digit(d, 10).unwrap_or('0'));
    }
    out
}

// ================================ 编码/工具 ================================

/// 匹配 Python `urllib.parse.urlencode`（quote_plus）：按 (k,v) 顺序拼 `k=v`，以 `&` 连接。
fn urlencode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// quote_plus：保留 `A-Za-z0-9 _.-~`，空格→`+`，其余→`%XX`（大写）。匹配 Python。
fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 剥离 HTML 标签（抖音 desc 偶含 `<em>` 等高亮标签）。
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// 把任意 JSON 值（number/string/null）转为字符串（抖音 `author.uid` 可能是数字或字符串）。
fn json_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 同键去重，保留最后一次出现的项（模拟 Python dict 后写覆盖前写）。
fn dedup_keep_last(params: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<(String, String)> = Vec::with_capacity(params.len());
    for (k, v) in params.into_iter().rev() {
        if seen.insert(k.clone()) {
            out.push((k, v));
        }
    }
    out.reverse();
    out
}

// ================================ 响应类型（宽松解析） ================================

#[derive(Debug, Default, Deserialize)]
pub struct SearchResp {
    #[serde(default)]
    pub status_code: i64,
    #[serde(default)]
    pub data: Vec<SearchDataItem>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchDataItem {
    #[serde(default)]
    pub aweme_info: Option<AwemeInfo>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AwemeInfo {
    #[serde(default)]
    pub aweme_id: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub create_time: i64,
    #[serde(default)]
    pub author: Option<Author>,
    #[serde(default)]
    pub statistics: Option<Statistics>,
    #[serde(default)]
    pub text_extra: Vec<TextExtra>,
    #[serde(default)]
    pub images: Vec<DouyinImage>,
    #[serde(default)]
    pub video: Option<Video>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Author {
    #[serde(default)]
    pub uid: serde_json::Value,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub sec_uid: String,
    #[serde(default)]
    pub avatar_thumb: Option<UrlList>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Statistics {
    #[serde(default)]
    pub digg_count: Option<i64>,
    #[serde(default)]
    pub comment_count: Option<i64>,
    #[serde(default)]
    pub collect_count: Option<i64>,
    #[serde(default)]
    pub share_count: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct TextExtra {
    #[serde(default)]
    pub hashtag_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DouyinImage {
    #[serde(default)]
    pub url_list: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Video {
    #[serde(default)]
    pub cover: Option<UrlList>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UrlList {
    #[serde(default)]
    pub url_list: Vec<String>,
}

// ================================ 单测 ================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_sign_uri_skips_search_only() {
        // 搜索接口不签名（忠实移植 client.py）
        assert!(!should_sign_uri("/aweme/v1/web/general/search/single/"));
        assert!(!should_sign_uri("/aweme/v1/web/general/search/"));
        // 其余接口签名
        assert!(should_sign_uri("/aweme/v1/web/aweme/detail/"));
        assert!(should_sign_uri("/aweme/v1/web/comment/list/"));
        assert!(should_sign_uri("/aweme/v1/web/comment/list/reply/"));
        assert!(should_sign_uri("/aweme/v1/web/user/profile/other/"));
    }

    #[test]
    fn build_search_url_constructs_params_and_skips_abogus_for_search() {
        let params = build_search_params(
            "rust 教程",
            0,
            SearchSort::General,
            PublishTime::Unlimited,
            "ms-fake-token",
            "7362810250930783783",
        );
        // 搜索不签名 → a_bogus=None
        let url = build_search_url(SEARCH_URI, &params, None);
        assert!(url.starts_with("https://www.douyin.com/aweme/v1/web/general/search/single/?"));
        // keyword 含空格与中文：空格→+，中文→UTF-8 %XX
        assert!(url.contains("keyword=rust+%E6%95%99%E7%A8%8B"));
        // common params 存在
        assert!(url.contains("device_platform=webapp"));
        assert!(url.contains("aid=6383"));
        assert!(url.contains("search_channel=aweme_general"));
        assert!(url.contains("from_group_id=7378810571505847586"));
        assert!(url.contains("count=15"));
        assert!(url.contains("offset=0"));
        assert!(url.contains("webid=7362810250930783783"));
        assert!(url.contains("msToken=ms-fake-token"));
        // 搜索被跳过，不应出现 a_bogus
        assert!(!url.contains("a_bogus="), "搜索接口不应带 a_bogus: {url}");
    }

    #[test]
    fn build_search_url_appends_abogus_when_provided() {
        // 模拟「对搜索也签名」的场景（如未来抖音调整）：a_bogus 应被追加。
        let params = build_search_params(
            "x",
            0,
            SearchSort::General,
            PublishTime::Unlimited,
            "",
            "123",
        );
        let url = build_search_url("/aweme/v1/web/aweme/detail/", &params, Some("DSaBqX=="));
        assert!(url.contains("a_bogus=DSaBqX%3D%3D")); // == 被 quote_plus
    }

    #[test]
    fn filter_selected_added_when_sort_or_time_non_default() {
        let params = build_search_params("x", 0, SearchSort::Latest, PublishTime::OneWeek, "", "1");
        let qs = urlencode(&params);
        assert!(qs.contains("is_filter_search=1"));
        assert!(qs.contains("filter_selected="));
        assert!(qs.contains("%22sort_type%22")); // "sort_type"
        assert!(qs.contains("%22publish_time%22"));
    }

    #[test]
    fn parse_search_results_maps_video_aweme() {
        let raw = r#"{
            "status_code": 0,
            "data": [
                {
                    "type": 1,
                    "aweme_info": {
                        "aweme_id": "7300000000000000001",
                        "desc": "rust 入门教程\n更多内容点击关注",
                        "create_time": 1700000000,
                        "author": {
                            "uid": "987654",
                            "nickname": "码农小馆",
                            "sec_uid": "MS4wLjABAAAAXXX",
                            "avatar_thumb": { "url_list": ["https://p3.douyinpic.com/avatar.jpg"] }
                        },
                        "statistics": {
                            "digg_count": 1024,
                            "comment_count": 88,
                            "collect_count": 12,
                            "share_count": 5
                        },
                        "text_extra": [
                            { "hashtag_name": "rust" },
                            { "hashtag_name": "编程" }
                        ],
                        "video": {
                            "cover": { "url_list": ["https://p3.douyinpic.com/cover.jpg"] }
                        }
                    }
                }
            ]
        }"#;
        let resp: SearchResp = serde_json::from_str(raw).unwrap();
        let items = parse_search_results(&resp, "rust");
        assert_eq!(items.len(), 1);
        let c = &items[0];
        assert_eq!(c.platform, Platform::Douyin);
        assert_eq!(c.content_type, ContentType::Video);
        assert_eq!(c.platform_id, "7300000000000000001");
        assert_eq!(
            c.url.as_deref(),
            Some("https://www.douyin.com/video/7300000000000000001")
        );
        assert_eq!(c.title, "rust 入门教程"); // 取 desc 首行
        assert_eq!(c.desc.as_deref(), Some("rust 入门教程\n更多内容点击关注"));
        assert_eq!(c.published_at, Some(1700000000));
        assert_eq!(c.liked_count, Some(1024));
        assert_eq!(c.comment_count, Some(88));
        assert_eq!(c.collected_count, Some(12));
        assert_eq!(c.share_count, Some(5));
        assert_eq!(c.tags, vec!["rust", "编程"]);
        assert_eq!(c.media_urls, vec!["https://p3.douyinpic.com/cover.jpg"]);
        let au = c.author.as_ref().unwrap();
        assert_eq!(au.nickname, "码农小馆");
        assert_eq!(au.platform_user_id, "987654");
        assert_eq!(
            au.avatar.as_deref(),
            Some("https://p3.douyinpic.com/avatar.jpg")
        );
    }

    #[test]
    fn parse_search_results_maps_image_note_and_numeric_uid() {
        // 图文笔记（images 非空）→ Image；uid 为数字时仍正确取值。
        let raw = r#"{
            "status_code": 0,
            "data": [
                {
                    "type": 1503,
                    "aweme_info": {
                        "aweme_id": "7300000000000000002",
                        "desc": "<em>图文</em>笔记分享",
                        "create_time": 0,
                        "author": { "uid": 12345, "nickname": "A" },
                        "statistics": { "digg_count": 7 },
                        "text_extra": [],
                        "images": [
                            { "url_list": ["https://x/1.jpg", "https://x/2.jpg"] }
                        ]
                    }
                },
                { "type": 99 }
            ]
        }"#;
        let resp: SearchResp = serde_json::from_str(raw).unwrap();
        let items = parse_search_results(&resp, "图文");
        // 第二项无 aweme_info → 跳过
        assert_eq!(items.len(), 1);
        let c = &items[0];
        assert_eq!(c.content_type, ContentType::Image);
        assert_eq!(c.platform_id, "7300000000000000002");
        assert_eq!(c.title, "图文笔记分享"); // HTML 标签已剥离
        assert_eq!(c.media_urls, vec!["https://x/1.jpg", "https://x/2.jpg"]);
        assert_eq!(c.published_at, None); // create_time=0
        assert_eq!(c.liked_count, Some(7));
        assert_eq!(c.tags, vec!["图文"]); // 无 hashtag → 回退 keyword
        assert_eq!(c.author.as_ref().unwrap().platform_user_id, "12345"); // 数字 uid
    }

    #[test]
    fn generate_web_id_is_19_digit_string() {
        let id = generate_web_id();
        assert_eq!(id.len(), 19, "webid 应为 19 位: {id}");
        assert!(
            id.chars().all(|c| c.is_ascii_digit()),
            "webid 应全数字: {id}"
        );
        assert_ne!(id.chars().next(), Some('0'), "首位不应为 0: {id}");
    }

    #[test]
    fn quote_plus_matches_python_urlencode() {
        assert_eq!(quote_plus("a b"), "a+b");
        assert_eq!(quote_plus("rust 教程"), "rust+%E6%95%99%E7%A8%8B");
        assert_eq!(quote_plus("a&b=c"), "a%26b%3Dc");
        assert_eq!(quote_plus("A1-_.~"), "A1-_.~");
    }
}
