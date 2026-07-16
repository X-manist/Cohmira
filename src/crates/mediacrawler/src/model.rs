//! 标准化采集结果实体。
//!
//! 跨平台统一 schema：把小红书/抖音/B站等不同来源归一为同一组结构，
//! 便于 [`crate::store`] 落盘与下游 [`yunying_ops`] 分析。

use serde::{Deserialize, Serialize};

/// 平台枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Xhs,
    Douyin,
    Bilibili,
    Zhihu,
    Tieba,
    Kuaishou,
    Weibo,
}

/// 内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Note,
    Video,
    Image,
}

/// 归一化的内容（笔记/视频/图文）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    pub platform: Platform,
    pub content_type: ContentType,
    pub platform_id: String,
    pub url: Option<String>,
    pub title: String,
    pub desc: Option<String>,
    pub author: Option<Creator>,
    pub published_at: Option<i64>,
    pub liked_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub collected_count: Option<i64>,
    pub share_count: Option<i64>,
    pub tags: Vec<String>,
    pub media_urls: Vec<String>,
}

/// 归一化的评论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub platform: Platform,
    pub platform_comment_id: String,
    pub content_id: String,
    pub text: String,
    pub author_id: Option<String>,
    pub author_nickname: Option<String>,
    pub liked_count: Option<i64>,
    pub published_at: Option<i64>,
    pub sub_comment_count: Option<i64>,
}

/// 归一化的创作者。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Creator {
    pub platform: Platform,
    pub platform_user_id: String,
    pub nickname: String,
    pub avatar: Option<String>,
    pub desc: Option<String>,
    pub fans_count: Option<i64>,
    pub follows_count: Option<i64>,
    pub note_count: Option<i64>,
}
