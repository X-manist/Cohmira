//! # mediacrawler
//!
//! 自媒体平台公开信息采集 crate，纯 Rust 实现，替代原 [`MediaCrawler`](https://github.com/NanmiCoder/MediaCrawler) Python 项目。
//!
//! ## 设计目标
//!
//! - 消灭 Python 运行时与浏览器自动化（Playwright/DrissionPage）依赖，缩小桌面打包体积。
//! - 仅做小样本、合规、可人工复核的运营调研采集，不做绕风控或批量采集。
//! - 提供统一的平台抽象 [`platform::PlatformCrawler`]，每个平台实现搜索/详情/评论/创作者四类能力。
//! - 对外暴露与原 `MediaCrawler` HTTP API（`/api/crawler/start` 等）契约兼容的 Rust 接口，便于 [`crate::api`] 直接服务化。
//!
//! ## 模块
//!
//! - [`platform`]：平台抽象与各平台实现（xhs/douyin/bilibili/...）。
//! - [`login`]：登录态管理（cookie 注入优先；扫码登录降级为外部浏览器 CDP 引导）。
//! - [`model`]：标准化采集结果实体（笔记、视频、评论、创作者）。
//! - [`store`]：结果落盘（JSON / CSV，兼容原 `save_data_option`）。
//! - [`api`]：可选的 HTTP API 服务（替代 `uvicorn api.main:app`）。
//!
//! 注意：实现分阶段推进。v1 优先落地纯 HTTP + cookie 即可采集的平台（如小红书、抖音、B站搜索）；
//! 强依赖浏览器渲染/签名逆向的平台能力标注为 `todo!()` 并在文档中说明降级策略。

pub mod api;
pub mod login;
pub mod model;
pub mod platform;
pub mod signing;
pub mod store;

pub use model::*;
