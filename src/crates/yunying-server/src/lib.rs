//! # yunying-server
//!
//! 商媒运营助手桌面应用的 Rust 后端。以 **Goose 为底层 agent 框架（嵌入为 lib）**，
//! 聚合 mediacrawler / socialconnect 工具 crate 与 yunying-ops 操作面；视频能力由插件提供，
//! 对前端（Beav TS 桌面 UI）暴露聊天与工具契约。
//!
//! ## 与前端契约
//!
//! 保持与原 `goosed` sidecar 一致的 HTTP/SSE 表面（`/agent/start`、`/sessions/{id}/events`、
//! `/sessions/{id}/reply` 等，见 docs/14），前端最小改动即可切换到嵌入式后端；并提供 Tauri command
//! 适配层（见 `frontend` feature）以便后续迁移到 Tauri 单壳。
//!
//! ## 模块
//!
//! - [`goose_bridge`]：嵌入式 Goose 会话驱动（替代 sidecar + stdio 桥）。
//! - [`http`]：HTTP/SSE API（前端契约）。
//! - [`runtime`]：运行时组装（配置 → Goose + 工具 + MCP 注册）。

pub mod db;
pub mod goose_bridge;
pub mod http;
pub mod ipc;
pub mod login;
pub mod plugins;
pub mod runtime;
pub mod workspace;
