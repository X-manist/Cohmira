//! # yunying-config
//!
//! 商媒运营助手统一配置加载 crate。用 `config.json` 取代散落的 `.env`，便于桌面应用打包与人工复核。
//!
//! ## 设计
//!
//! - [`Config`]：强类型配置根，覆盖 Goose/OpenAI、MediaCrawler、Social Connection、OpenMontage、
//!   图像(Seedream/GPT image 2)、视频(Seedance/Ark Plan)、安全开关（`RUN_REAL_*`）等全部原 `.env` 项。
//! - [`loader::load`]：按优先级合并 默认值 ← `config.json` ← 环境变量（兼容旧 `.env` 行为，平滑迁移）。
//! - [`Config::redact`]：对 cookie / api_key 等敏感字段脱敏，供写入 work-package / 日志时使用。
//!
//! 配置文件查找顺序：显式指定路径 → `YUNYING_CONFIG_PATH` → 当前目录 `config.json` → 应用数据目录。

pub mod loader;
pub mod schema;

pub use loader::load;
pub use schema::Config;
