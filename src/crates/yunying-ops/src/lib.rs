//! # yunying-ops
//!
//! 运营智能体统一操作面 crate。聚合采集、发布、创作、客服等能力，对 Goose 暴露单一 MCP 工具集，
//! 替代原 `mcps/operations-mcp`（TS）以及散落的 beav/mediacrawler/social-connection/openmontage MCP。
//!
//! ## 工具面（与原 operations-mcp 契约对齐）
//!
//! - 采集：`start_task` / `get_status` / `list_data_files` / `read_data_file` / `archive_crawler_data`
//! - 笔记/报告：`create_note`
//! - 应用命令：`run_app_command`
//! - 创作：`generate_image`；视频生成由 OpenMontage 插件调用桌面端统一视频服务
//! - 发布：`upload_note` / `upload_video` / `social_login_prepare` / `social_check_account`
//! - 能力发现：`list_capabilities`
//!
//! ## 安全闸
//!
//! 所有写/真实操作默认 `dry_run=true`；真实执行需显式 `confirm=true` 且 `check_account` 通过；
//! 高风险操作（发布/退款/删除）强制人工审批，并对敏感参数（cookie/key）脱敏后落审计。

pub mod scenarios;
pub mod tools;
pub mod work_params;

/// 运营操作统一入口，持有所需子能力（采集器、发布器、创作器）的句柄。
///
/// 由 [`yunying-server`] 在启动时组装并注入 Goose 的 MCP 注册表。
pub struct Operations {}
