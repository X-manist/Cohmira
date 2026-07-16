//! 小红书采集器（纯 HTTP + xhshow 纯算法签名）。
//!
//! 移植自 `MediaCrawler/media_platform/xhs/client.py` 的搜索能力。
//! 搜索端点 `POST /api/sns/web/v1/search/notes`，请求体由
//! [`build_search_body_json`] 按 Python 字典序构造，再交 [`crate::signing::xhs`] 签名。
//!
//! 登录态：通过注入的 Cookie 串（至少需 `a1` 与 `web_session`）。`XhsCrawler` 持有一个
//! `reqwest::Client`（建议带 cookie store）与 Cookie 字符串。
//!
//! 设计遵循 bilibili/weibo 模式：`build_*`（纯函数）+ `parse_*`（纯函数，fixture 单测）
//! + `XhsCrawler { http }` + `impl PlatformCrawler`。

use crate::model::{Content, ContentType, Creator, Platform};
use crate::signing::xhs::{self, SignInput, XhsHeaders};
use serde::Deserialize;

const HOST: &str = "https://edith.xiaohongshu.com";
const SEARCH_URI: &str = "/api/sns/web/v1/search/notes";

/// 浏览器 UA（与 xhshow `PUBLIC_USERAGENT` 一致）。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36 Edg/142.0.0.0";

/// 搜索排序：综合（对齐 `SearchSortType.GENERAL`）。
pub const SORT_GENERAL: &str = "general";
/// 笔记类型：全部（对齐 `SearchNoteType.ALL = 0`）。
pub const NOTE_TYPE_ALL: i64 = 0;

// ---------------------------------------------------------------------------
// 采集器
// ---------------------------------------------------------------------------

/// 小红书采集器。
pub struct XhsCrawler {
    http: reqwest::Client,
    /// Cookie 串（`k=v; k2=v2`），至少需含 `a1`。
    cookie: String,
}

impl XhsCrawler {
    /// 用给定 HTTP client 与 Cookie 构造。
    pub fn new(http: reqwest::Client, cookie: impl Into<String>) -> Self {
        Self {
            http,
            cookie: cookie.into(),
        }
    }

    /// 按关键词搜索笔记。`limit` 映射为 page_size（上限 20，对齐 PC 端单页规模）。
    pub async fn search_notes(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        let page_size = limit.clamp(1, 20) as i64;
        let search_id = get_search_id();
        let body = build_search_body_json(
            keyword,
            1,
            page_size,
            &search_id,
            SORT_GENERAL,
            NOTE_TYPE_ALL,
        );
        let headers = sign_search(&body, &self.cookie);
        let url = format!("{HOST}{SEARCH_URI}");

        let resp = self
            .http
            .post(&url)
            .header("X-S", &headers.x_s)
            .header("X-T", &headers.x_t)
            .header("X-S-Common", &headers.x_s_common)
            .header("X-B3-Traceid", &headers.x_b3_traceid)
            .header("Cookie", &self.cookie)
            .header("Content-Type", "application/json;charset=UTF-8")
            .header("User-Agent", USER_AGENT)
            .header("Origin", "https://www.xiaohongshu.com")
            .header("Referer", "https://www.xiaohongshu.com/")
            .body(body.clone())
            .send()
            .await?
            .error_for_status()?;

        let resp_json: SearchResp = resp.json().await?;
        Ok(parse_search_results(&resp_json, keyword))
    }
}

#[async_trait::async_trait]
impl crate::platform::PlatformCrawler for XhsCrawler {
    fn name(&self) -> &'static str {
        "xhs"
    }
    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        self.search_notes(keyword, limit).await
    }
}

// ---------------------------------------------------------------------------
// 纯函数：签名 / 构造 / 解析
// ---------------------------------------------------------------------------

/// 对搜索 POST 请求签名（纯函数：仅依赖 body 与 cookie）。
pub fn sign_search(body_json: &str, cookie: &str) -> XhsHeaders {
    let input = SignInput::Post {
        uri: SEARCH_URI,
        body_json,
    };
    xhs::sign(&input, cookie, xhs::now_secs())
}

/// 构造搜索请求体 JSON（紧凑、键序与 Python `client.get_note_by_keyword` 一致）。
///
/// 注意：键序决定 `content_string`，进而决定签名；必须与 [`sign_search`] 送签的 body 完全
/// 相同。本函数与请求 body 使用同一字符串，故天然一致。
pub fn build_search_body_json(
    keyword: &str,
    page: i64,
    page_size: i64,
    search_id: &str,
    sort: &str,
    note_type: i64,
) -> String {
    let kw = serde_json::to_string(keyword).unwrap_or_else(|_| String::from("\"\""));
    let sid = serde_json::to_string(search_id).unwrap_or_else(|_| String::from("\"\""));
    let s = serde_json::to_string(sort).unwrap_or_else(|_| String::from("\"\""));
    format!(
        "{{\"keyword\":{kw},\"page\":{page},\"page_size\":{page_size},\
         \"search_id\":{sid},\"sort\":{s},\"note_type\":{note_type}}}"
    )
}

/// 解析搜索响应为归一化 [`Content`]（纯函数，便于用 fixture 单测）。
pub fn parse_search_results(resp: &SearchResp, keyword: &str) -> Vec<Content> {
    let items = resp.data.items.as_deref().unwrap_or(&[]);
    items
        .iter()
        .filter_map(|it| {
            let nc = it.note_card.as_ref()?;
            let content_type = if nc.r#type.as_deref() == Some("video") {
                ContentType::Video
            } else {
                ContentType::Note
            };

            let mut media: Vec<String> = Vec::new();
            if let Some(cover) = nc.cover.as_ref().and_then(|c| c.url.clone()) {
                media.push(cover);
            }
            if let Some(imgs) = nc.image_list.as_ref() {
                for img in imgs {
                    if let Some(u) = img.url.as_ref() {
                        media.push(u.clone());
                    }
                }
            }

            let title = if nc.display_title.is_empty() {
                nc.desc.chars().take(40).collect::<String>()
            } else {
                nc.display_title.clone()
            };

            let interact = nc.interact_info.as_ref();
            let pop = |s: &Option<String>| s.as_ref().and_then(|v| v.parse::<i64>().ok());

            Some(Content {
                platform: Platform::Xhs,
                content_type,
                platform_id: nc.note_id.clone(),
                url: Some(format!(
                    "https://www.xiaohongshu.com/explore/{}",
                    nc.note_id
                )),
                title,
                desc: if nc.desc.is_empty() {
                    None
                } else {
                    Some(nc.desc.clone())
                },
                author: nc.user.as_ref().map(|u| Creator {
                    platform: Platform::Xhs,
                    platform_user_id: u.user_id.clone(),
                    nickname: u.nickname.clone(),
                    avatar: u.avatar.clone(),
                    desc: None,
                    fans_count: None,
                    follows_count: None,
                    note_count: None,
                }),
                published_at: None,
                liked_count: interact.and_then(|i| pop(&i.liked_count)),
                comment_count: interact.and_then(|i| pop(&i.comment_count)),
                collected_count: interact.and_then(|i| pop(&i.collected_count)),
                share_count: interact.and_then(|i| pop(&i.share_count)),
                tags: vec![keyword.to_string()],
                media_urls: media,
            })
        })
        .collect()
}

/// 生成 `search_id`（对齐 `help.get_search_id`：`(ms << 64) + rand` 的 base36）。
pub fn get_search_id() -> String {
    let ms = xhs::now_secs() as u128 * 1000;
    let e = ms << 64;
    let t = (xhs::next_search_rand() % 2_147_483_646) as u128;
    base36_encode(e.wrapping_add(t))
}

/// base36 编码（大写字母表，对齐 `help.base36encode`）。
fn base36_encode(mut n: u128) -> String {
    const A: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if n == 0 {
        return String::from("0");
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(A[(n % 36) as usize]);
        n /= 36;
    }
    buf.iter().rev().map(|&b| b as char).collect()
}

// ---------------------------------------------------------------------------
// 响应类型（仅取需要的字段，未知字段忽略；interact_info 计数为字符串）
// ---------------------------------------------------------------------------

/// 搜索响应外层（`{"success":..., "data":{...}}`）。
#[derive(Debug, Deserialize)]
pub struct SearchResp {
    #[serde(default)]
    pub data: SearchData,
}
#[derive(Debug, Default, Deserialize)]
pub struct SearchData {
    #[serde(default)]
    pub items: Option<Vec<SearchItem>>,
}
#[derive(Debug, Deserialize)]
pub struct SearchItem {
    #[serde(default)]
    pub note_card: Option<NoteCard>,
}
#[derive(Debug, Default, Deserialize)]
pub struct NoteCard {
    #[serde(default)]
    pub note_id: String,
    #[serde(default)]
    pub display_title: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub user: Option<NoteUser>,
    #[serde(default)]
    pub interact_info: Option<InteractInfo>,
    #[serde(default)]
    pub cover: Option<Cover>,
    #[serde(default)]
    pub image_list: Option<Vec<ImageItem>>,
}
#[derive(Debug, Default, Deserialize)]
pub struct NoteUser {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub avatar: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
pub struct InteractInfo {
    #[serde(default)]
    pub liked_count: Option<String>,
    #[serde(default)]
    pub collected_count: Option<String>,
    #[serde(default)]
    pub comment_count: Option<String>,
    #[serde(default)]
    pub share_count: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
pub struct Cover {
    #[serde(default)]
    pub url: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
pub struct ImageItem {
    #[serde(default)]
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContentType, Platform};

    #[test]
    fn build_search_body_is_compact_and_ordered() {
        let body = build_search_body_json("猫爬架", 1, 20, "ABC", "general", 0);
        // 键序须为 keyword,page,page_size,search_id,sort,note_type（与 Python 字典序一致）
        let expected = r#"{"keyword":"猫爬架","page":1,"page_size":20,"search_id":"ABC","sort":"general","note_type":0}"#;
        assert_eq!(body, expected);
        // 中文不转义（ensure_ascii=False）
        assert!(body.contains("猫爬架"));
    }

    #[test]
    fn sign_search_returns_well_formed_headers() {
        let body = r#"{"keyword":"测试","page":1,"page_size":20,"search_id":"X","sort":"general","note_type":0}"#;
        let cookie =
            "a1=1900000000000abcdef0123456789abcdef0123456789abcdef012345678; web_session=s";
        let h = sign_search(body, cookie);
        assert!(h.x_s.starts_with("XYS_"), "bad x-s: {}", h.x_s);
        assert!(h.x_s.len() > 200);
        assert!(h.x_t.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(h.x_t.len(), 13);
        assert!(h.x_s_common.len() > 200);
        assert_eq!(h.x_b3_traceid.len(), 16);
    }

    #[test]
    fn parse_maps_note_card_to_content() {
        let raw = r#"{
            "success": true,
            "data": {
                "items": [
                    {
                        "id": "item1",
                        "model_type": "note",
                        "note_card": {
                            "note_id": "abc123",
                            "display_title": "猫爬架测评",
                            "desc": "好用不贵",
                            "type": "normal",
                            "user": {"user_id": "u1", "nickname": "萌宠家", "avatar": "https://x/a.jpg"},
                            "interact_info": {
                                "liked_count": "10200",
                                "collected_count": "500",
                                "comment_count": "88",
                                "share_count": "12"
                            },
                            "cover": {"url": "https://x/cover.jpg"},
                            "image_list": [{"url": "https://x/1.jpg"}, {"url": "https://x/2.jpg"}]
                        },
                        "xsec_token": "tok"
                    }
                ]
            }
        }"#;
        let resp: SearchResp = serde_json::from_str(raw).unwrap();
        let items = parse_search_results(&resp, "猫爬架");
        assert_eq!(items.len(), 1);
        let c = &items[0];
        assert_eq!(c.platform, Platform::Xhs);
        assert_eq!(c.content_type, ContentType::Note);
        assert_eq!(c.platform_id, "abc123");
        assert_eq!(c.title, "猫爬架测评");
        assert_eq!(c.desc.as_deref(), Some("好用不贵"));
        assert_eq!(
            c.url.as_deref(),
            Some("https://www.xiaohongshu.com/explore/abc123")
        );
        assert_eq!(c.liked_count, Some(10200));
        assert_eq!(c.collected_count, Some(500));
        assert_eq!(c.comment_count, Some(88));
        assert_eq!(c.share_count, Some(12));
        assert_eq!(c.media_urls.len(), 3); // cover + 2 images
        assert_eq!(c.media_urls[0], "https://x/cover.jpg");
        let author = c.author.as_ref().unwrap();
        assert_eq!(author.nickname, "萌宠家");
        assert_eq!(author.platform_user_id, "u1");
        assert_eq!(c.tags, vec!["猫爬架"]);
    }

    #[test]
    fn parse_video_note_type() {
        let raw = r#"{"data":{"items":[{"note_card":{"note_id":"v1","display_title":"","desc":"视频简介","type":"video"}}]}}"#;
        let resp: SearchResp = serde_json::from_str(raw).unwrap();
        let items = parse_search_results(&resp, "kw");
        let c = &items[0];
        assert_eq!(c.content_type, ContentType::Video);
        // display_title 为空时取 desc 前 40 字
        assert_eq!(c.title, "视频简介");
    }

    #[test]
    fn parse_empty_items() {
        let resp: SearchResp = serde_json::from_str(r#"{"data":{}}"#).unwrap();
        assert!(parse_search_results(&resp, "x").is_empty());
    }

    #[test]
    fn base36_encode_known() {
        assert_eq!(base36_encode(0), "0");
        assert_eq!(base36_encode(10), "A");
        assert_eq!(base36_encode(35), "Z");
        assert_eq!(base36_encode(36), "10");
    }

    #[test]
    fn search_id_is_nonempty_base36() {
        let id = get_search_id();
        assert!(!id.is_empty());
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() && c.is_ascii_uppercase() || c.is_ascii_digit()),
            "search_id should be base36: {id}"
        );
    }
}
