//! bilibili 采集器（纯 HTTP + wbi 签名，并带 SSR 搜索页回退）。
//!
//! 移植自 `MediaCrawler/media_platform/bilibili/client.py`。
//! 搜索端点 `/x/web-interface/wbi/search/type`，参数经 [`crate::signing::bilibili::WbiSign`] 签名。
//!
//! wbi 的 img_key/sub_key 从 `/x/web-interface/nav` 的 `wbi_img.img_url`/`sub_url` 文件名提取，
//! 或由调用方注入。登录态（cookie）通过 HTTP client 的 cookie store 携带。

use crate::model::{Content, ContentType, Creator, Platform};
use crate::signing::bilibili::{SignedQuery, WbiSign};
use anyhow::Context;
use chrono::{NaiveDate, TimeZone, Utc};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use std::collections::HashSet;

const HOST: &str = "https://api.bilibili.com";
const SEARCH_URI: &str = "/x/web-interface/wbi/search/type";
const NAV_URI: &str = "/x/web-interface/nav";
const WEB_SEARCH_HOST: &str = "https://search.bilibili.com/all";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// bilibili 采集器。
pub struct BilibiliCrawler {
    http: reqwest::Client,
    /// wbi 签名键（img_key, sub_key）。None 时由 [`Self::fetch_wbi_keys`] 获取。
    wbi_keys: std::sync::Mutex<Option<(String, String)>>,
}

impl BilibiliCrawler {
    /// 用给定 HTTP client 构造。wbi 键惰性获取。
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            wbi_keys: std::sync::Mutex::new(None),
        }
    }

    /// 直接注入 wbi 键（避免每次请求打 nav）。
    pub fn with_wbi_keys(self, img_key: impl Into<String>, sub_key: impl Into<String>) -> Self {
        *self.wbi_keys.lock().unwrap() = Some((img_key.into(), sub_key.into()));
        self
    }

    /// 从 nav 接口提取 wbi 键（img_key, sub_key）。
    pub async fn fetch_wbi_keys(&self) -> anyhow::Result<(String, String)> {
        if let Some(k) = self.wbi_keys.lock().unwrap().clone() {
            return Ok(k);
        }
        let resp: NavResp = self
            .http
            .get(format!("{HOST}{NAV_URI}"))
            .send()
            .await?
            .json()
            .await?;
        let img_key = extract_key(&resp.data.wbi_img.img_url);
        let sub_key = extract_key(&resp.data.wbi_img.sub_url);
        Ok((img_key, sub_key))
    }

    /// 搜索视频。`limit` 映射为 page_size（上限 50）。
    pub async fn search_videos(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        let page_size = limit.clamp(1, 50);
        match self.search_videos_via_api(keyword, page_size).await {
            Ok(items) if !items.is_empty() => Ok(items),
            Ok(_) => self
                .search_videos_via_ssr(keyword, page_size)
                .await
                .context("Bilibili WBI API returned no items and SSR fallback failed"),
            Err(api_error) => self
                .search_videos_via_ssr(keyword, page_size)
                .await
                .with_context(|| {
                    format!("Bilibili WBI API failed ({api_error}) and SSR fallback failed")
                }),
        }
    }

    async fn search_videos_via_api(
        &self,
        keyword: &str,
        page_size: usize,
    ) -> anyhow::Result<Vec<Content>> {
        let (img_key, sub_key) = self.fetch_wbi_keys().await?;
        let sign = WbiSign::new(&img_key, &sub_key);
        let url = build_search_url(&sign, keyword, 1, page_size as i64);
        let resp: SearchResp = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if resp.code != 0 {
            anyhow::bail!("Bilibili search failed with API code {}", resp.code);
        }
        Ok(parse_search_results(&resp, page_size, keyword))
    }

    async fn search_videos_via_ssr(
        &self,
        keyword: &str,
        page_size: usize,
    ) -> anyhow::Result<Vec<Content>> {
        let url = build_web_search_url(keyword)?;
        let html = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.7")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(parse_web_search_results(&html, keyword, page_size))
    }
}

#[async_trait::async_trait]
impl crate::platform::PlatformCrawler for BilibiliCrawler {
    fn name(&self) -> &'static str {
        "bilibili"
    }

    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        self.search_videos(keyword, limit).await
    }
}

/// 构造已 wbi 签名的搜索 URL（纯函数，便于单测）。
pub fn build_search_url(sign: &WbiSign, keyword: &str, page: i64, page_size: i64) -> String {
    let mut params = vec![
        ("search_type".to_string(), "video".to_string()),
        ("keyword".to_string(), keyword.to_string()),
        ("page".to_string(), page.to_string()),
        ("page_size".to_string(), page_size.to_string()),
        ("order".to_string(), "default".to_string()),
        ("pubtime_begin_s".to_string(), "0".to_string()),
        ("pubtime_end_s".to_string(), "0".to_string()),
    ];
    let wts = unix_ts();
    let SignedQuery { query, .. } = sign.sign(&mut params, wts);
    format!("{HOST}{SEARCH_URI}?{query}")
}

/// 构造 Bilibili 服务端渲染搜索页 URL。
pub fn build_web_search_url(keyword: &str) -> anyhow::Result<url::Url> {
    let mut url = url::Url::parse(WEB_SEARCH_HOST)?;
    url.query_pairs_mut().append_pair("keyword", keyword);
    Ok(url)
}

/// 解析搜索响应为归一化 [`Content`]（纯函数，便于用 fixture 单测）。
fn parse_search_results(resp: &SearchResp, _expected: usize, keyword: &str) -> Vec<Content> {
    let Some(results) = resp.data.result.as_ref() else {
        return Vec::new();
    };
    results
        .iter()
        .map(|r| Content {
            platform: Platform::Bilibili,
            content_type: ContentType::Video,
            platform_id: r.bvid.clone(),
            url: Some(format!("https://www.bilibili.com/video/{}", r.bvid)),
            title: strip_html(&r.title),
            desc: r.description.clone().filter(|s| !s.is_empty()),
            author: Some(Creator {
                platform: Platform::Bilibili,
                platform_user_id: r.mid.map(|m| m.to_string()).unwrap_or_default(),
                nickname: r.author.clone().unwrap_or_default(),
                avatar: r.upic.clone(),
                desc: None,
                fans_count: None,
                follows_count: None,
                note_count: None,
            }),
            published_at: r.pubdate,
            liked_count: r.play,
            comment_count: r.video_review,
            collected_count: None,
            share_count: None,
            tags: r
                .tag
                .as_ref()
                .map(|t| t.split(',').map(String::from).collect())
                .unwrap_or_default(),
            media_urls: r.pic.iter().cloned().collect(),
        })
        .map(|mut c| {
            if c.tags.is_empty() {
                c.tags.push(keyword.to_string());
            }
            c
        })
        .collect()
}

/// 从 Bilibili 服务端渲染的搜索页解析视频卡片。
///
/// 这里使用 HTML5 DOM + CSS selector，避免依赖页面源码的空白、属性顺序或换行格式。
pub fn parse_web_search_results(html: &str, keyword: &str, limit: usize) -> Vec<Content> {
    let document = Html::parse_document(html);
    let card_selector = static_selector(".bili-video-card");
    let video_link_selector = static_selector("a[href*='/video/BV']");
    let title_selector = static_selector(".bili-video-card__info--tit");
    let owner_selector = static_selector(".bili-video-card__info--owner");
    let author_selector = static_selector(".bili-video-card__info--author");
    let date_selector = static_selector(".bili-video-card__info--date");
    let image_selector = static_selector("img[src]");
    let stat_selector =
        static_selector(".bili-video-card__stats--left .bili-video-card__stats--item span");
    let mut seen = HashSet::new();
    let mut items = Vec::new();

    for card in document.select(&card_selector) {
        let Some(link) = card.select(&video_link_selector).next() else {
            continue;
        };
        let Some((platform_id, video_url)) = parse_video_link(link.value().attr("href")) else {
            continue;
        };
        if !seen.insert(platform_id.clone()) {
            continue;
        }

        let title_element = card.select(&title_selector).next();
        let title = title_element
            .and_then(|element| element.value().attr("title").map(str::to_owned))
            .or_else(|| title_element.map(element_text))
            .unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }

        let owner = card.select(&owner_selector).next();
        let nickname = card
            .select(&author_selector)
            .next()
            .map(element_text)
            .unwrap_or_default();
        let platform_user_id = owner
            .and_then(|element| element.value().attr("href"))
            .and_then(parse_space_user_id)
            .unwrap_or_default();
        let published_at = card
            .select(&date_selector)
            .next()
            .map(element_text)
            .and_then(|date| parse_full_date(&date));
        let stats = card
            .select(&stat_selector)
            .map(element_text)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let media_urls = card
            .select(&image_selector)
            .filter_map(|image| image.value().attr("src"))
            .filter_map(normalize_public_url)
            .take(1)
            .collect();

        items.push(Content {
            platform: Platform::Bilibili,
            content_type: ContentType::Video,
            platform_id,
            url: Some(video_url),
            title: title.trim().to_string(),
            desc: None,
            author: Some(Creator {
                platform: Platform::Bilibili,
                platform_user_id,
                nickname,
                avatar: None,
                desc: None,
                fans_count: None,
                follows_count: None,
                note_count: None,
            }),
            published_at,
            liked_count: stats.first().and_then(|value| parse_localized_count(value)),
            comment_count: stats.get(1).and_then(|value| parse_localized_count(value)),
            collected_count: None,
            share_count: None,
            tags: vec![keyword.to_string()],
            media_urls,
        });

        if items.len() >= limit {
            break;
        }
    }

    items
}

fn static_selector(css: &str) -> Selector {
    Selector::parse(css).expect("static Bilibili selector must be valid")
}

fn element_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_video_link(href: Option<&str>) -> Option<(String, String)> {
    let url = href.and_then(normalize_public_url)?;
    let parsed = url::Url::parse(&url).ok()?;
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let video_index = segments.iter().position(|segment| *segment == "video")?;
    let bvid = segments.get(video_index + 1)?.trim_end_matches('/');
    if !bvid.starts_with("BV") || !bvid[2..].chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    Some((
        bvid.to_string(),
        format!("https://www.bilibili.com/video/{bvid}"),
    ))
}

fn parse_space_user_id(href: &str) -> Option<String> {
    let url = normalize_public_url(href)?;
    let parsed = url::Url::parse(&url).ok()?;
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let id = if parsed.host_str() == Some("space.bilibili.com") {
        segments.first().copied()
    } else {
        segments
            .iter()
            .position(|segment| *segment == "space")
            .and_then(|index| segments.get(index + 1).copied())
    }?;
    (!id.is_empty() && id.chars().all(|ch| ch.is_ascii_digit())).then(|| id.to_string())
}

fn normalize_public_url(raw: &str) -> Option<String> {
    let url = if raw.starts_with("//") {
        format!("https:{raw}")
    } else if raw.starts_with('/') {
        format!("https://www.bilibili.com{raw}")
    } else {
        raw.to_string()
    };
    url::Url::parse(&url).ok().map(|parsed| parsed.to_string())
}

fn parse_localized_count(raw: &str) -> Option<i64> {
    let compact = raw.trim().replace(',', "");
    let (number, multiplier) = if let Some(number) = compact.strip_suffix('万') {
        (number, 10_000_f64)
    } else if let Some(number) = compact.strip_suffix('亿') {
        (number, 100_000_000_f64)
    } else {
        (compact.as_str(), 1_f64)
    };
    let count = number.parse::<f64>().ok()? * multiplier;
    count.is_finite().then(|| count.round() as i64)
}

fn parse_full_date(raw: &str) -> Option<i64> {
    let date = raw.trim().trim_start_matches('·').trim();
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let midnight = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&midnight).timestamp())
}

/// 从 wbi_img URL（如 `https://i0.hdslb.com/bfs/wbi/<key>.png`）提取键。
fn extract_key(url: &str) -> String {
    url.rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".png")
        .to_string()
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

fn unix_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---- 响应类型（仅取需要的字段，未知字段忽略）----

#[derive(Debug, Deserialize)]
pub(crate) struct SearchResp {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub data: SearchData,
}
#[derive(Debug, Default, Deserialize)]
pub(crate) struct SearchData {
    #[serde(default)]
    pub result: Option<Vec<SearchItem>>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct SearchItem {
    #[serde(default)]
    pub bvid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub mid: Option<i64>,
    #[serde(default)]
    pub upic: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pubdate: Option<i64>,
    #[serde(default)]
    pub play: Option<i64>,
    #[serde(default)]
    pub video_review: Option<i64>,
    #[serde(default)]
    pub pic: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NavResp {
    #[serde(default)]
    data: NavData,
}
#[derive(Debug, Default, Deserialize)]
struct NavData {
    #[serde(default)]
    wbi_img: NavWbi,
}
#[derive(Debug, Default, Deserialize)]
struct NavWbi {
    #[serde(default)]
    img_url: String,
    #[serde(default)]
    sub_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC_IMG_KEY: &str = "7cd084941338484aae1ad9425b84077c";
    const DOC_SUB_KEY: &str = "4932caff0ff746eab6f01bf08b70ac45";

    #[test]
    fn build_search_url_contains_signed_params() {
        let sign = WbiSign::new(DOC_IMG_KEY, DOC_SUB_KEY);
        let url = build_search_url(&sign, "猫爬架", 1, 20);
        assert!(url.starts_with("https://api.bilibili.com/x/web-interface/wbi/search/type?"));
        assert!(url.contains("search_type=video"));
        assert!(url.contains("keyword=%E7%8C%AB%E7%88%AC%E6%9E%B6")); // "猫爬架" quote_plus
        assert!(url.contains("page=1"));
        assert!(url.contains("page_size=20"));
        assert!(url.contains("w_rid="));
        assert!(url.contains("wts="));
    }

    #[test]
    fn parse_results_maps_fields_and_strips_html() {
        let raw = r#"{
            "code": 0,
            "data": {
                "result": [
                    {
                        "bvid": "BV1abcd",
                        "title": "<em class=\"keyword\">猫爬架</em>测评",
                        "author": "萌宠家",
                        "mid": 12345,
                        "upic": "https://x/u.jpg",
                        "description": "好看的猫爬架",
                        "pubdate": 1700000000,
                        "play": 10200,
                        "video_review": 88,
                        "pic": "https://x/p.jpg",
                        "tag": "猫,宠物,猫爬架"
                    }
                ]
            }
        }"#;
        let resp: SearchResp = serde_json::from_str(raw).unwrap();
        let items = parse_search_results(&resp, 20, "猫爬架");
        assert_eq!(items.len(), 1);
        let c = &items[0];
        assert_eq!(c.platform, Platform::Bilibili);
        assert_eq!(c.content_type, ContentType::Video);
        assert_eq!(c.platform_id, "BV1abcd");
        assert_eq!(c.title, "猫爬架测评"); // HTML 标签已剥离
        assert_eq!(
            c.url.as_deref(),
            Some("https://www.bilibili.com/video/BV1abcd")
        );
        assert_eq!(c.liked_count, Some(10200));
        assert_eq!(c.comment_count, Some(88));
        assert_eq!(c.published_at, Some(1700000000));
        assert_eq!(c.tags, vec!["猫", "宠物", "猫爬架"]);
        assert_eq!(c.media_urls, vec!["https://x/p.jpg"]);
        let author = c.author.as_ref().unwrap();
        assert_eq!(author.nickname, "萌宠家");
        assert_eq!(author.platform_user_id, "12345");
    }

    #[test]
    fn parse_empty_result_returns_empty() {
        let resp: SearchResp = serde_json::from_str(r#"{"code":0,"data":{}}"#).unwrap();
        assert!(parse_search_results(&resp, 20, "x").is_empty());
    }

    #[test]
    fn parses_ssr_search_fixture_structurally() {
        let html = include_str!("../../fixtures/bilibili_search.html");
        let items = parse_web_search_results(html, "猫爬架", 20);

        assert_eq!(items.len(), 2);
        let first = &items[0];
        assert_eq!(first.platform_id, "BV15k4y1k76k");
        assert_eq!(first.title, "选购猫爬架的经验分享 | 450元猫爬架的翻车经历");
        assert_eq!(
            first.url.as_deref(),
            Some("https://www.bilibili.com/video/BV15k4y1k76k")
        );
        assert_eq!(first.liked_count, Some(204_000));
        assert_eq!(first.comment_count, Some(349));
        assert_eq!(first.published_at, Some(1_588_809_600));
        assert_eq!(first.author.as_ref().unwrap().nickname, "畅洋兄");
        assert_eq!(first.author.as_ref().unwrap().platform_user_id, "268866642");
        assert_eq!(
            first.media_urls,
            vec!["https://i1.hdslb.com/bfs/archive/cover.jpg"]
        );

        assert_eq!(items[1].platform_id, "BV1SeM76HEWb");
        assert_eq!(items[1].published_at, None, "缺少年份的日期不应臆测年份");
    }

    #[test]
    fn ssr_parser_deduplicates_bvid_and_honors_limit() {
        let html = include_str!("../../fixtures/bilibili_search.html");
        let items = parse_web_search_results(html, "猫爬架", 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].platform_id, "BV15k4y1k76k");
    }

    #[test]
    fn localized_counts_are_parsed() {
        assert_eq!(parse_localized_count("20.4万"), Some(204_000));
        assert_eq!(parse_localized_count("1.2亿"), Some(120_000_000));
        assert_eq!(parse_localized_count("1,243"), Some(1_243));
        assert_eq!(parse_localized_count("--"), None);
    }

    #[test]
    fn web_search_url_encodes_keyword() {
        let url = build_web_search_url("猫爬架 & 木制").unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "keyword")
                .unwrap()
                .1,
            "猫爬架 & 木制"
        );
    }

    #[test]
    fn extract_key_from_url() {
        assert_eq!(
            extract_key("https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png"),
            "7cd084941338484aae1ad9425b84077c"
        );
    }

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html("<em>hi</em> <b>x"), "hi x");
    }
}
