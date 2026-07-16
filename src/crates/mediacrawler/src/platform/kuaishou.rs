//! 快手采集器（GraphQL，纯 HTTP，靠 cookie）。
//!
//! 移植自 `MediaCrawler/media_platform/kuaishou/client.py`。
//! 搜索端点 `https://www.kuaishou.com/graphql`（POST），operationName=`visionSearchPhoto`。
//! 查询串来自原 `graphql/search_query.graphql`（`include_str!` 嵌入，保持与上游一致）。

use crate::model::{Content, ContentType, Creator, Platform};
use serde::Deserialize;

const HOST: &str = "https://www.kuaishou.com/graphql";
/// 搜索 GraphQL 查询（含 fragments），与上游 `search_query.graphql` 一致。
const SEARCH_QUERY: &str = include_str!("kuaishou_search.graphql");

/// 快手采集器。
pub struct KuaishouCrawler {
    http: reqwest::Client,
}

impl KuaishouCrawler {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// 按关键词搜索视频，分页直到达到 `limit` 或无更多结果。
    pub async fn search_videos(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        let mut all = Vec::new();
        let mut pcursor = String::new();
        while all.len() < limit {
            let body = build_search_body(keyword, &pcursor);
            let resp: KsResp = self
                .http
                .post(HOST)
                .json(&body)
                .send()
                .await?
                .json()
                .await?;
            let search = resp.data.vision_search_photo;
            let feeds = search.feeds.unwrap_or_default();
            if feeds.is_empty() {
                break;
            }
            all.extend(parse_feeds(&feeds, keyword));
            pcursor = search.pcursor.unwrap_or_default();
            if pcursor == "no_more" || pcursor.is_empty() {
                break;
            }
        }
        all.truncate(limit);
        Ok(all)
    }
}

#[async_trait::async_trait]
impl crate::platform::PlatformCrawler for KuaishouCrawler {
    fn name(&self) -> &'static str {
        "kuaishou"
    }
    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        self.search_videos(keyword, limit).await
    }
}

/// 构造 GraphQL 请求体（纯函数，便于单测）。
pub fn build_search_body(keyword: &str, pcursor: &str) -> serde_json::Value {
    serde_json::json!({
        "operationName": "visionSearchPhoto",
        "variables": {
            "keyword": keyword,
            "pcursor": pcursor,
            "page": "search",
            "searchSessionId": "",
        },
        "query": SEARCH_QUERY,
    })
}

/// 解析 feeds 为 [`Content`]（纯函数）。
fn parse_feeds(feeds: &[KsFeed], keyword: &str) -> Vec<Content> {
    feeds
        .iter()
        .filter_map(|f| {
            let p = f.photo.as_ref()?;
            let author = f.author.as_ref();
            Some(Content {
                platform: Platform::Kuaishou,
                content_type: ContentType::Video,
                platform_id: p.id.clone(),
                url: p.photo_url.clone(),
                title: p.caption.clone().unwrap_or_default(),
                desc: p.origin_caption.clone(),
                author: author.map(|a| Creator {
                    platform: Platform::Kuaishou,
                    platform_user_id: a.id.clone(),
                    nickname: a.name.clone(),
                    avatar: a.header_url.clone(),
                    desc: None,
                    fans_count: a.fan_count,
                    follows_count: None,
                    note_count: a.photo_count,
                }),
                published_at: p.timestamp,
                liked_count: p.like_count,
                comment_count: p.comment_count,
                collected_count: None,
                share_count: p.view_count,
                tags: vec![keyword.to_string()],
                media_urls: p.cover_urls.clone().unwrap_or_default(),
            })
        })
        .collect()
}

// ---- 响应类型 ----

#[derive(Debug, Deserialize)]
struct KsResp {
    #[serde(default)]
    data: KsData,
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KsData {
    #[serde(default)]
    vision_search_photo: KsSearch,
}
#[derive(Debug, Default, Deserialize)]
struct KsSearch {
    #[serde(default)]
    feeds: Option<Vec<KsFeed>>,
    #[serde(default)]
    pcursor: Option<String>,
}
#[derive(Debug, Deserialize)]
struct KsFeed {
    #[serde(default)]
    photo: Option<KsPhoto>,
    #[serde(default)]
    author: Option<KsAuthor>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KsPhoto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    origin_caption: Option<String>,
    #[serde(default)]
    like_count: Option<i64>,
    #[serde(default)]
    view_count: Option<i64>,
    #[serde(default)]
    comment_count: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    photo_url: Option<String>,
    #[serde(default)]
    cover_urls: Option<Vec<String>>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KsAuthor {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    header_url: Option<String>,
    #[serde(default)]
    fan_count: Option<i64>,
    #[serde(default)]
    photo_count: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_has_operation_and_query() {
        let body = build_search_body("猫爬架", "");
        assert_eq!(body["operationName"], "visionSearchPhoto");
        assert_eq!(body["variables"]["keyword"], "猫爬架");
        assert_eq!(body["variables"]["page"], "search");
        let q = body["query"].as_str().unwrap();
        assert!(q.contains("visionSearchPhoto") || q.contains("photoContent"));
    }

    #[test]
    fn parse_feeds_maps_fields() {
        let raw = r#"{
            "data": {
                "visionSearchPhoto": {
                    "pcursor": "no_more",
                    "feeds": [
                        {
                            "photo": {
                                "id": "3xabc",
                                "caption": "猫爬架开箱",
                                "originCaption": "猫爬架开箱详细",
                                "likeCount": 500,
                                "viewCount": 8000,
                                "commentCount": 42,
                                "timestamp": 1700000000000,
                                "photoUrl": "https://x/v.mp4",
                                "coverUrls": ["https://x/c.jpg"]
                            },
                            "author": {
                                "id": "u1", "name": "宠物博主", "headerUrl": "https://x/h.jpg",
                                "fanCount": 120000, "photoCount": 300
                            }
                        }
                    ]
                }
            }
        }"#;
        let resp: KsResp = serde_json::from_str(raw).unwrap();
        let feeds = resp.data.vision_search_photo.feeds.unwrap();
        let items = parse_feeds(&feeds, "猫爬架");
        assert_eq!(items.len(), 1);
        let c = &items[0];
        assert_eq!(c.platform, Platform::Kuaishou);
        assert_eq!(c.platform_id, "3xabc");
        assert_eq!(c.title, "猫爬架开箱");
        assert_eq!(c.liked_count, Some(500));
        assert_eq!(c.share_count, Some(8000)); // view_count 映射到 share 位（参考指标）
        assert_eq!(c.media_urls, vec!["https://x/c.jpg"]);
        let a = c.author.as_ref().unwrap();
        assert_eq!(a.nickname, "宠物博主");
        assert_eq!(a.fans_count, Some(120000));
    }

    #[test]
    fn parse_empty_feeds() {
        assert!(parse_feeds(&[], "x").is_empty());
    }
}
