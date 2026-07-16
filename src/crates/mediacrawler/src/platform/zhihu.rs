//! 知乎采集器（HTTP + `x-zse-96` 签名，纯 cookie）。
//!
//! 移植自 `MediaCrawler/media_platform/zhihu/client.py`（搜索）与
//! `MediaCrawler/media_platform/zhihu/help.py`（结果解析）。
//!
//! ## 搜索端点
//!
//! `GET https://www.zhihu.com/api/v4/search_v3?...`，请求需带 `Cookie`（含 `d_c0`）+
//! `x-zse-96` / `x-zst-81` 两个签名头。签名输入是「带查询串的完整 path」（即 Python 版 `final_uri`），
//! 由 [`crate::signing::zhihu::ZhihuSigner`] 计算。
//!
//! ## 设计
//!
//! 与 bilibili/weibo/kuaishou 一致：
//! - [`build_search_uri`]：纯函数，构造 `/api/v4/search_v3?...`（签名输入）。
//! - [`parse_search_results`]：纯函数，把 `data` 数组归一为 [`Content`]（fixture 单测）。
//! - [`ZhihuCrawler`]：持有 `reqwest::Client` + cookie + 惰性签名器，实现 [`PlatformCrawler`]。
//!
//! ## 解析语义（对齐 help.py::extract_contents_from_search）
//!
//! 1. `data` 数组中只保留 `type ∈ {search_result, zvideo}` 的项；
//! 2. 取每项的 `object` 字段（缺失则跳过）；
//! 3. 按 `object.type` 分发：`answer`→笔记、`article`→笔记、`zvideo`→视频；其余丢弃。

use crate::model::{Content, ContentType, Creator, Platform};
use crate::signing::zhihu::{ZhihuSignResult, ZhihuSigner};
use serde_json::Value;
use std::sync::OnceLock;

const HOST: &str = "https://www.zhihu.com";
const ZHUANLAN_HOST: &str = "https://zhuanlan.zhihu.com";
const SEARCH_URI: &str = "/api/v4/search_v3";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

// 搜索参数默认值（对齐 field.py 的 Enum 默认）：空串 = 综合排序 / 不限类型 / 不限时间。
pub const SORT_DEFAULT: &str = "";
pub const TYPE_DEFAULT: &str = "";
pub const TIME_DEFAULT: &str = "";

/// 知乎采集器。
///
/// `cookie` 必须含 `d_c0`；`signer` 在首次搜索时惰性构造（加载 `zhihu.js`，开销集中在首请求）。
pub struct ZhihuCrawler {
    http: reqwest::Client,
    cookie: String,
    signer: OnceLock<ZhihuSigner>,
}

impl ZhihuCrawler {
    /// 用给定 HTTP client 构造；调用方需再通过 [`Self::with_cookie`] 注入 cookie。
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            cookie: String::new(),
            signer: OnceLock::new(),
        }
    }

    /// 注入原始 `Cookie` 头字符串（需含 `d_c0`）。
    pub fn with_cookie(mut self, cookie: impl Into<String>) -> Self {
        self.cookie = cookie.into();
        self
    }

    /// 惰性获取签名器（首次调用时构造，后续复用）。
    fn signer(&self) -> anyhow::Result<&ZhihuSigner> {
        // ZhihuSigner 无状态（new 恒 Ok）；用稳定的 get_or_init 避免 unstable once_cell_try。
        Ok(self
            .signer
            .get_or_init(|| ZhihuSigner::new().unwrap_or(ZhihuSigner)))
    }

    /// 按关键词搜索知乎内容。`limit` 控制返回条数上限（每页 20，自动翻页，封顶 50 页防失控）。
    ///
    /// 返回归一化后的 [`Content`] 列表；`sort`/`note_type`/`search_time` 分别对应
    /// field.py 的 `SearchSort`/`SearchType`/`SearchTime` 的 `.value`，传空串即默认。
    pub async fn search_notes(
        &self,
        keyword: &str,
        limit: usize,
        sort: &str,
        note_type: &str,
        search_time: &str,
    ) -> anyhow::Result<Vec<Content>> {
        if self.cookie.trim().is_empty() {
            anyhow::bail!("zhihu cookie 未配置：搜索需要含 d_c0 的 Cookie");
        }
        let signer = self.signer()?;
        let limit = limit.max(1);
        let page_size = 20_i64;
        let mut all: Vec<Content> = Vec::with_capacity(limit);
        let mut page = 1_i64;
        while all.len() < limit {
            let uri = build_search_uri(keyword, page, page_size, sort, note_type, search_time);
            let sign = signer.sign(&uri, &self.cookie)?;
            let url = format!("{HOST}{uri}");
            let resp = self
                .http
                .get(&url)
                .headers(build_headers(&sign, &self.cookie)?)
                .send()
                .await?;
            let body: Value = resp.json().await?;
            // 顶层 error 字段非空 → 抛出（对齐 client.py::request 的 error 检查）。
            if let Some(err) = body.get("error").filter(|v| !v.is_null()) {
                anyhow::bail!("zhihu search error: {err}");
            }
            let data = match body.get("data").and_then(|d| d.as_array()) {
                Some(arr) if !arr.is_empty() => arr,
                _ => break, // 无更多数据
            };
            let mapped = parse_search_results(data, keyword);
            if mapped.is_empty() {
                break;
            }
            all.extend(mapped);
            page += 1;
            if page > 50 {
                break; // 防御性封顶
            }
        }
        all.truncate(limit);
        Ok(all)
    }
}

#[async_trait::async_trait]
impl crate::platform::PlatformCrawler for ZhihuCrawler {
    fn name(&self) -> &'static str {
        "zhihu"
    }

    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        self.search_notes(keyword, limit, SORT_DEFAULT, TYPE_DEFAULT, TIME_DEFAULT)
            .await
    }
}

/// 构造搜索 URI（带查询串，**签名输入**，不含 host）。
///
/// 纯函数，参数顺序与 `client.py::get_note_by_keyword` 完全一致；query 编码对齐 Python
/// `urllib.parse.urlencode`（quote_plus，键值均编码）。
pub fn build_search_uri(
    keyword: &str,
    page: i64,
    page_size: i64,
    sort: &str,
    note_type: &str,
    search_time: &str,
) -> String {
    let offset = (page - 1).max(0) * page_size;
    // 顺序严格对齐 Python dict 插入序（影响签名 md5 输入）。
    let params: [(&str, String); 13] = [
        ("gk_version", "gz-gaokao".to_string()),
        ("t", "general".to_string()),
        ("q", keyword.to_string()),
        ("correction", "1".to_string()),
        ("offset", offset.to_string()),
        ("limit", page_size.to_string()),
        ("filter_fields", String::new()),
        ("lc_idx", offset.to_string()),
        ("show_all_topics", "0".to_string()),
        ("search_source", "Filter".to_string()),
        ("time_interval", search_time.to_string()),
        ("sort", sort.to_string()),
        ("vertical", note_type.to_string()),
    ];
    let query = urlencode(&params);
    format!("{SEARCH_URI}?{query}")
}

/// 解析 `search_v3` 的 `data` 数组为归一化 [`Content`]（纯函数，便于 fixture 单测）。
///
/// 语义见模块文档：仅 `type ∈ {search_result, zvideo}` 项的 `object` 被解析，
/// `object.type ∈ {answer, article, zvideo}` 才产出。
pub fn parse_search_results(items: &[Value], keyword: &str) -> Vec<Content> {
    items
        .iter()
        .filter(|it| {
            matches!(
                it.get("type").and_then(|v| v.as_str()),
                Some("search_result") | Some("zvideo")
            )
        })
        .filter_map(|it| it.get("object"))
        .filter_map(|obj| map_object(obj, keyword))
        .collect()
}

/// 按内容类型分发映射（answer/article/zvideo）。
fn map_object(obj: &Value, keyword: &str) -> Option<Content> {
    let t = obj.get("type").and_then(|v| v.as_str())?;
    let content = match t {
        "answer" => map_answer(obj),
        "article" => map_article(obj),
        "zvideo" => map_zvideo(obj),
        _ => return None,
    };
    let mut content = content;
    if content.tags.is_empty() {
        content.tags.push(keyword.to_string());
    }
    Some(content)
}

/// 映射回答（answer）。
fn map_answer(obj: &Value) -> Content {
    let id = as_string(obj.get("id"));
    let qid = as_string(obj.get("question").and_then(|q| q.get("id")));
    let url = if id.is_empty() || qid.is_empty() {
        None
    } else {
        Some(format!("{HOST}/question/{qid}/answer/{id}"))
    };
    Content {
        platform: Platform::Zhihu,
        content_type: ContentType::Note,
        platform_id: id,
        url,
        title: strip_html(as_opt_str(obj.get("title")).unwrap_or("")),
        desc: desc_of(
            as_opt_str(obj.get("description")),
            as_opt_str(obj.get("excerpt")),
        ),
        author: map_author(obj.get("author")),
        published_at: as_i64(obj.get("created_time")),
        liked_count: as_i64(obj.get("voteup_count")),
        comment_count: as_i64(obj.get("comment_count")),
        collected_count: None,
        share_count: None,
        tags: Vec::new(),
        media_urls: Vec::new(),
    }
}

/// 映射专栏文章（article）。
fn map_article(obj: &Value) -> Content {
    let id = as_string(obj.get("id"));
    let url = if id.is_empty() {
        None
    } else {
        Some(format!("{ZHUANLAN_HOST}/p/{id}"))
    };
    Content {
        platform: Platform::Zhihu,
        content_type: ContentType::Note,
        platform_id: id,
        url,
        title: strip_html(as_opt_str(obj.get("title")).unwrap_or("")),
        desc: as_opt_str(obj.get("excerpt"))
            .map(strip_html)
            .filter(|s| !s.is_empty()),
        author: map_author(obj.get("author")),
        published_at: as_i64(obj.get("created_time")).or_else(|| as_i64(obj.get("created"))),
        liked_count: as_i64(obj.get("voteup_count")),
        comment_count: as_i64(obj.get("comment_count")),
        collected_count: None,
        share_count: None,
        tags: Vec::new(),
        media_urls: Vec::new(),
    }
}

/// 映射知乎视频（zvideo）。
///
/// 与 help.py 一致：若对象含 `video` 子字典（创作者主页视频列表形态），URL 用
/// `https://www.zhihu.com/zvideo/{id}`；否则用 `video_url` 字段（搜索结果形态）。
fn map_zvideo(obj: &Value) -> Content {
    let id = as_string(obj.get("id"));
    let has_video_dict = obj.get("video").is_some_and(|v| v.is_object());
    let url = if has_video_dict {
        if id.is_empty() {
            None
        } else {
            Some(format!("{HOST}/zvideo/{id}"))
        }
    } else {
        as_opt_str(obj.get("video_url")).map(String::from)
    };
    let published_at = if has_video_dict {
        as_i64(obj.get("published_at")).or_else(|| as_i64(obj.get("updated_at")))
    } else {
        as_i64(obj.get("created_at"))
    };
    Content {
        platform: Platform::Zhihu,
        content_type: ContentType::Video,
        platform_id: id,
        url,
        title: strip_html(as_opt_str(obj.get("title")).unwrap_or("")),
        desc: as_opt_str(obj.get("description"))
            .map(strip_html)
            .filter(|s| !s.is_empty()),
        author: map_author(obj.get("author")),
        published_at,
        liked_count: as_i64(obj.get("voteup_count")),
        comment_count: as_i64(obj.get("comment_count")),
        collected_count: None,
        share_count: None,
        tags: Vec::new(),
        media_urls: Vec::new(),
    }
}

/// 映射作者（对齐 help.py::_extract_content_or_comment_author）。
///
/// 若 `author` 无 `id`（评论等形态），则下沉到 `author.member`；任一字段缺失都返回 `None`，
/// 由上层落到 `Content.author = None`。
fn map_author(author: Option<&Value>) -> Option<Creator> {
    let raw = author?;
    let a = if raw.get("id").is_some() {
        raw
    } else {
        raw.get("member")?
    };
    let uid = as_string(a.get("id"));
    let token = as_string(a.get("url_token"));
    if uid.is_empty() && token.is_empty() {
        return None;
    }
    Some(Creator {
        platform: Platform::Zhihu,
        platform_user_id: uid,
        nickname: as_string(a.get("name")),
        avatar: as_opt_str(a.get("avatar_url")).map(String::from),
        desc: None,
        fans_count: None,
        follows_count: None,
        note_count: None,
    })
}

/// description 优先，其次 excerpt；剥 HTML 后若空则 None（对齐 Python `description or excerpt`）。
fn desc_of(desc: Option<&str>, excerpt: Option<&str>) -> Option<String> {
    let primary = desc.map(strip_html);
    match primary {
        Some(s) if !s.is_empty() => Some(s),
        _ => excerpt.map(strip_html).filter(|s| !s.is_empty()),
    }
}

fn as_opt_str(v: Option<&Value>) -> Option<&str> {
    v.and_then(|val| val.as_str())
}

/// 把 JSON 值（字符串/数字）统一成 `String`（知乎 id 可能以数字或字符串形式出现）。
fn as_string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// 容错的整数提取（兼容 i64 / f64 / 数字串）。
fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.parse::<i64>().ok(),
        _ => None,
    }
}

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
    out.trim().to_string()
}

/// quote_plus：保留 `A-Za-z0-9_.-~`，空格→`+`，其余→`%XX`（大写），对齐 Python urlencode。
fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// urlencode：按给定 (k,v) 顺序拼 `k=v`，以 `&` 连接（键值均 quote_plus）。
fn urlencode(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// 组装请求头（Cookie + 两个签名头 + UA）。
fn build_headers(
    sign: &ZhihuSignResult,
    cookie: &str,
) -> anyhow::Result<reqwest::header::HeaderMap> {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(
        reqwest::header::COOKIE,
        cookie.parse().map_err(anyhow::Error::from)?,
    );
    h.insert(
        "x-zse-96",
        sign.x_zse_96.parse().map_err(anyhow::Error::from)?,
    );
    h.insert(
        "x-zst-81",
        sign.x_zst_81.parse().map_err(anyhow::Error::from)?,
    );
    h.insert(
        reqwest::header::USER_AGENT,
        UA.parse().map_err(anyhow::Error::from)?,
    );
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_search_uri_shape_and_encoding() {
        let uri = build_search_uri("猫爬架", 2, 20, SORT_DEFAULT, TYPE_DEFAULT, TIME_DEFAULT);
        // path 正确
        assert!(uri.starts_with("/api/v4/search_v3?"), "uri was: {uri}");
        // 关键参数齐全且顺序对齐 client.py
        assert!(uri.contains("gk_version=gz-gaokao"));
        assert!(uri.contains("t=general"));
        assert!(uri.contains("q=%E7%8C%AB%E7%88%AC%E6%9E%B6")); // "猫爬架" quote_plus
        assert!(uri.contains("correction=1"));
        assert!(uri.contains("offset=20")); // (page-1)*page_size = 20
        assert!(uri.contains("limit=20"));
        assert!(uri.contains("lc_idx=20"));
        assert!(uri.contains("show_all_topics=0"));
        assert!(uri.contains("search_source=Filter"));
        assert!(uri.contains("time_interval="));
        assert!(uri.contains("sort="));
        assert!(uri.contains("vertical="));
        // 第 1 页 offset 应为 0
        let uri_p1 = build_search_uri("x", 1, 20, "", "", "");
        assert!(uri_p1.contains("offset=0") && uri_p1.contains("lc_idx=0"));
    }

    #[test]
    fn parse_search_results_maps_answer_and_zvideo() {
        // fixture：1 个 answer（search_result 包裹）+ 1 个 zvideo + 1 个未知类型 + 1 个广告项（被过滤）。
        let raw = r#"[
          {
            "type": "search_result",
            "object": {
              "type": "answer",
              "id": 123456,
              "title": "<p>如何挑选<b>猫爬架</b>？</p>",
              "content": "<p>正文</p>",
              "description": "很实用的回答",
              "excerpt": "摘选",
              "created_time": 1700000000,
              "updated_time": 1700000100,
              "voteup_count": 88,
              "comment_count": 12,
              "question": {"id": 987654},
              "author": {"id": "uid-1", "name": "猫奴日记", "url_token": "maonv", "avatar_url": "https://x/a.jpg"}
            }
          },
          {
            "type": "search_result",
            "object": {
              "type": "zvideo",
              "id": "vid-9",
              "title": "猫爬架视频评测",
              "description": "<em>视频</em>描述",
              "video_url": "https://www.zhihu.com/zvideo/vid-9",
              "created_at": 1700000200,
              "voteup_count": 7,
              "comment_count": 1,
              "author": {"id": "uid-2", "name": "视频主", "url_token": "vmain", "avatar_url": "https://x/b.jpg"}
            }
          },
          {"type": "search_result", "object": {"type": "unknown", "id": "x"}},
          {"type": "ad", "object": {"type": "answer", "id": "should-be-skipped"}}
        ]"#;
        let items: Vec<Value> = serde_json::from_str(raw).unwrap();
        let results = parse_search_results(&items, "猫爬架");
        assert_eq!(
            results.len(),
            2,
            "answer + zvideo 各 1 条，未知类型与广告项被过滤"
        );

        // ---- answer ----
        let ans = &results[0];
        assert_eq!(ans.platform, Platform::Zhihu);
        assert_eq!(ans.content_type, ContentType::Note);
        assert_eq!(ans.platform_id, "123456");
        assert_eq!(
            ans.url.as_deref(),
            Some("https://www.zhihu.com/question/987654/answer/123456")
        );
        assert_eq!(ans.title, "如何挑选猫爬架？"); // HTML 标签已剥离
        assert_eq!(ans.desc.as_deref(), Some("很实用的回答")); // description 优先于 excerpt
        assert_eq!(ans.published_at, Some(1700000000));
        assert_eq!(ans.liked_count, Some(88));
        assert_eq!(ans.comment_count, Some(12));
        assert_eq!(ans.tags, vec!["猫爬架"]); // 关键词回填
        let author = ans.author.as_ref().unwrap();
        assert_eq!(author.platform_user_id, "uid-1");
        assert_eq!(author.nickname, "猫奴日记");
        assert_eq!(author.avatar.as_deref(), Some("https://x/a.jpg"));

        // ---- zvideo ----
        let vid = &results[1];
        assert_eq!(vid.content_type, ContentType::Video);
        assert_eq!(vid.platform_id, "vid-9");
        assert_eq!(
            vid.url.as_deref(),
            Some("https://www.zhihu.com/zvideo/vid-9")
        ); // 无 video 字典 → 用 video_url
        assert_eq!(vid.desc.as_deref(), Some("视频描述")); // HTML 剥离
        assert_eq!(vid.liked_count, Some(7));
        assert_eq!(vid.published_at, Some(1700000200));
    }

    #[test]
    fn parse_empty_or_malformed_returns_empty() {
        assert!(parse_search_results(&[], "x").is_empty());
        // 没有 object 字段的项被跳过
        let v: Vec<Value> =
            serde_json::from_str(r#"[{"type":"search_result"},{"type":"ad"}]"#).unwrap();
        assert!(parse_search_results(&v, "x").is_empty());
    }

    #[test]
    fn author_falls_back_to_member() {
        // 评论形态：author 无 id，需下沉到 member（对齐 help.py）。
        let obj: Value = serde_json::from_str(
            r#"{"type":"answer","id":1,"title":"t","question":{"id":2},
               "author":{"member":{"id":"m1","name":"路人","url_token":"lr","avatar_url":"u"}}}"#,
        )
        .unwrap();
        let c = map_object(&obj, "kw").unwrap();
        let a = c.author.as_ref().unwrap();
        assert_eq!(a.platform_user_id, "m1");
        assert_eq!(a.nickname, "路人");
    }
}
