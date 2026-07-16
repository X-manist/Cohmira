//! 微博采集器（纯 HTTP，无签名，靠 cookie）。
//!
//! 移植自 `MediaCrawler/media_platform/weibo/client.py`。
//! 搜索端点 `m.weibo.cn/api/container/getIndex`，containerid=`100103type={search_type}&q={keyword}`。

use crate::model::{Content, ContentType, Creator, Platform};
use anyhow::{bail, Context};
use reqwest::header::{ACCEPT, CONTENT_TYPE, REFERER, USER_AGENT};
use serde::Deserialize;

const HOST: &str = "https://m.weibo.cn";
const SEARCH_URI: &str = "/api/container/getIndex";

/// 微博采集器。
pub struct WeiboCrawler {
    http: reqwest::Client,
}

impl WeiboCrawler {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// 按关键词搜索微博（综合排序）。`limit` 控制返回条数上限。
    pub async fn search_notes(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        let mut all = Vec::new();
        let mut page = 1_i64;
        while all.len() < limit {
            let url = build_search_url(keyword, page);
            let response = self
                .http
                .get(&url)
                .header(USER_AGENT, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36")
                .header(REFERER, "https://m.weibo.cn/")
                .header(ACCEPT, "application/json, text/plain, */*")
                .send()
                .await
                .context("weibo search request failed")?
                .error_for_status()
                .context("weibo search returned an HTTP error")?;
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unknown")
                .to_string();
            if !content_type.to_ascii_lowercase().contains("json") {
                bail!(
                    "weibo search requires a valid login cookie or visitor verification (content-type: {content_type})"
                );
            }
            let body = response
                .bytes()
                .await
                .context("failed to read weibo search response")?;
            let resp: WeiboResp =
                serde_json::from_slice(&body).context("weibo search returned malformed JSON")?;
            let cards = resp.data.cards.unwrap_or_default();
            if cards.is_empty() {
                break;
            }
            let mapped = parse_search_results(&cards, keyword);
            if mapped.is_empty() {
                break;
            }
            all.extend(mapped);
            page += 1;
        }
        all.truncate(limit);
        Ok(all)
    }
}

#[async_trait::async_trait]
impl crate::platform::PlatformCrawler for WeiboCrawler {
    fn name(&self) -> &'static str {
        "weibo"
    }
    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        self.search_notes(keyword, limit).await
    }
}

/// 构造搜索 URL（纯函数，便于单测）。
pub fn build_search_url(keyword: &str, page: i64) -> String {
    // search_type=综合(1)。containerid 自身包含 `=`/`&`，必须作为一个查询值编码。
    let containerid = format!("100103type=1&q={}", keyword);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("containerid", &containerid)
        .append_pair("page_type", "searchall")
        .append_pair("page", &page.to_string())
        .finish();
    format!("{HOST}{SEARCH_URI}?{query}")
}

/// 解析 cards（card_type=9 含 mblog）为 [`Content`]（纯函数）。
fn parse_search_results(cards: &[WeiboCard], keyword: &str) -> Vec<Content> {
    cards
        .iter()
        .filter(|c| c.card_type == 9)
        .filter_map(|c| c.mblog.as_ref().map(|m| (c, m)))
        .map(|(_, m)| {
            let text = strip_html(&m.text);
            let title = text.chars().take(40).collect::<String>();
            Content {
                platform: Platform::Weibo,
                content_type: ContentType::Note,
                platform_id: m.id.to_string(),
                url: Some(format!("{HOST}/detail/{}", m.id)),
                title,
                desc: if text.is_empty() { None } else { Some(text) },
                author: m.user.as_ref().map(|u| Creator {
                    platform: Platform::Weibo,
                    platform_user_id: u.id.to_string(),
                    nickname: u.screen_name.clone(),
                    avatar: Some(u.profile_image_url.clone()),
                    desc: None,
                    fans_count: None,
                    follows_count: None,
                    note_count: None,
                }),
                published_at: None, // created_at 是相对时间字符串，需要额外解析；留空
                liked_count: m.attitudes_count,
                comment_count: m.comments_count,
                collected_count: None,
                share_count: m.reposts_count,
                tags: vec![keyword.to_string()],
                media_urls: m.pics.clone().unwrap_or_default(),
            }
        })
        .collect()
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
    out
}

// ---- 响应类型 ----

#[derive(Debug, Deserialize)]
struct WeiboResp {
    #[serde(default)]
    data: WeiboData,
}
#[derive(Debug, Default, Deserialize)]
struct WeiboData {
    #[serde(default)]
    cards: Option<Vec<WeiboCard>>,
}
#[derive(Debug, Deserialize)]
struct WeiboCard {
    #[serde(default)]
    card_type: i64,
    #[serde(default)]
    mblog: Option<Mblog>,
}
#[derive(Debug, Deserialize)]
struct Mblog {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    user: Option<WeiboUser>,
    #[serde(default)]
    attitudes_count: Option<i64>,
    #[serde(default)]
    comments_count: Option<i64>,
    #[serde(default)]
    reposts_count: Option<i64>,
    #[serde(default)]
    pics: Option<Vec<String>>,
}
#[derive(Debug, Deserialize)]
struct WeiboUser {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    screen_name: String,
    #[serde(default)]
    profile_image_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_shape() {
        let url = build_search_url("母婴", 2);
        assert!(url.starts_with("https://m.weibo.cn/api/container/getIndex?"));
        let parsed = url::Url::parse(&url).unwrap();
        let query = parsed
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            query.get("containerid").map(|value| value.as_ref()),
            Some("100103type=1&q=母婴")
        );
        assert_eq!(
            query.get("page_type").map(|value| value.as_ref()),
            Some("searchall")
        );
        assert_eq!(query.get("page").map(|value| value.as_ref()), Some("2"));
        assert!(!parsed.query().unwrap_or_default().contains("&q="));
    }

    #[test]
    fn parse_maps_mblog_cards_only() {
        let raw = r#"{
            "data": {
                "cards": [
                    {"card_type": 11, "card_group": []},
                    {"card_type": 9, "mblog": {
                        "id": 999,
                        "text": "<span>婴儿床</span>推荐<a>...</a>",
                        "user": {"id": 7, "screen_name": "宝妈日记", "profile_image_url": "https://x/u.jpg"},
                        "attitudes_count": 120,
                        "comments_count": 30,
                        "reposts_count": 5,
                        "pics": ["https://x/1.jpg", "https://x/2.jpg"]
                    }}
                ]
            }
        }"#;
        let resp: WeiboResp = serde_json::from_str(raw).unwrap();
        let cards = resp.data.cards.unwrap();
        let items = parse_search_results(&cards, "婴儿床");
        assert_eq!(items.len(), 1);
        let c = &items[0];
        assert_eq!(c.platform, Platform::Weibo);
        assert_eq!(c.platform_id, "999");
        assert!(c.desc.as_deref().unwrap().contains("婴儿床推荐"));
        assert!(!c.desc.as_deref().unwrap().contains("<")); // HTML 剥离
        assert_eq!(c.liked_count, Some(120));
        assert_eq!(c.share_count, Some(5));
        assert_eq!(c.media_urls.len(), 2);
        assert_eq!(c.author.as_ref().unwrap().nickname, "宝妈日记");
    }

    #[test]
    fn parse_empty_cards() {
        assert!(parse_search_results(&[], "x").is_empty());
    }
}
