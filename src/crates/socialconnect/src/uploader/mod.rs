//! 平台发布抽象与各平台适配器。
//!
//! 安全闸：所有 `upload_*` 默认 `dry_run=true`；真实发布需 `confirm=true` 且 [`account`] 已 check 通过。
//!
//! ## 平台适配器
//!
//! - [`douyin`]：抖音（creator.douyin.com）视频/图文上传（RPA via [`crate::browser::BrowserDriver`]）。

pub mod bilibili;
pub mod douyin;
pub mod kuaishou;
pub mod tencent;
pub mod xiaohongshu;
pub mod youtube;

pub use douyin::DouyinUploader;

/// 发布请求（平台无关草稿）。
#[derive(Debug, Clone)]
pub struct UploadRequest {
    pub platform: String,
    pub account_profile: String,
    pub title: String,
    pub desc: String,
    pub tags: Vec<String>,
    pub media_paths: Vec<String>,
    pub schedule: Option<String>,
    pub dry_run: bool,
    pub confirm: bool,
}

/// 发布结果。
#[derive(Debug, Clone)]
pub struct UploadResult {
    pub platform_post_id: Option<String>,
    pub url: Option<String>,
    pub dry_run: bool,
    pub note: String,
}

/// 发布器统一接口。
#[async_trait::async_trait]
pub trait Uploader: Send + Sync {
    fn platform(&self) -> &'static str;
    async fn upload_note(&self, req: &UploadRequest) -> anyhow::Result<UploadResult>;
    async fn upload_video(&self, req: &UploadRequest) -> anyhow::Result<UploadResult>;
}
