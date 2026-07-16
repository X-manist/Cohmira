//! 登录态管理。
//!
//! 策略（与原 MediaCrawler 对齐，见 docs/17）：
//! - cookie 注入优先：从 [`crate::model`] 之外的环境/配置读取平台 cookie，注入 HTTP client。
//! - 扫码登录降级：Rust 核心不内嵌浏览器自动化；引导用户在真实浏览器登录后回填 cookie，
//!   或通过 CDP（外部 Chrome 9222）读取登录态。这避免把 Chromium 打进桌面包。

use crate::model::Platform;

/// 某平台的登录态。
#[derive(Debug, Clone)]
pub struct Login {
    pub platform: Platform,
    pub cookies: String,
}

impl Login {
    pub fn new(platform: Platform, cookies: impl Into<String>) -> Self {
        Self {
            platform,
            cookies: cookies.into(),
        }
    }
}
