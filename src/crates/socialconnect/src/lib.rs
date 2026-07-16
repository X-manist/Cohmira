//! # socialconnect
//!
//! 自媒体多平台账号与发布 crate。账号文件、运行时编排、CDP 浏览器控制和各平台上传器
//! 全部在 Rust 进程内运行，不依赖 Python、Node、Playwright Server 或 chromedriver。
//!
//! ## 设计目标
//!
//! - 多平台账号 profile 与登录态（cookie）管理，登录态文件兼容原 `cookies/<platform>_<profile>.json`。
//! - 平台统一的发布抽象 [`uploader::Uploader`]：图文笔记（upload_note）与视频（upload_video）。
//! - 安全闸：所有写操作默认 `dry_run=true`，真实发布需显式 `confirm=true`，并强制 `check_account` 通过。
//! - 六个平台均由原生 Rust 上传器承接，登录、校验、扫码和发布共用同一套 CDP 会话。
//!
//! ## 模块
//!
//! - [`uploader`]：平台发布抽象与各平台适配器。
//! - [`account`]：账号 profile / cookie 存储。
//! - [`schedule`]：多平台批量发布排期（兼容原 `--schedule "YYYY-MM-DD HH:MM"`）。
//! - [`cli`]：纯 Rust 动作参数模型与平台能力校验。

pub mod account;
pub mod browser;
pub mod cli;
#[cfg(feature = "cdp")]
pub mod native;
pub mod schedule;
pub mod uploader;
