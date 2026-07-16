//! 运营操作工具集（与原 operations-mcp 契约对齐）。
//!
//! 工具列表：
//! - list_capabilities / start_task / get_status / list_data_files / read_data_file / archive_crawler_data
//! - create_note / run_app_command / generate_image / upload_note / upload_video
//! - social_login_prepare / social_check_account
//!
//! 每个工具默认 `dry_run=true`；真实执行需显式 `confirm=true` 且前置 check 通过。

/// 工具名常量，供 MCP 注册与 schema 校验引用。
pub mod names {
    pub const LIST_CAPABILITIES: &str = "list_capabilities";
    pub const START_TASK: &str = "start_task";
    pub const GET_STATUS: &str = "get_status";
    pub const LIST_DATA_FILES: &str = "list_data_files";
    pub const READ_DATA_FILE: &str = "read_data_file";
    pub const ARCHIVE_CRAWLER_DATA: &str = "archive_crawler_data";
    pub const CREATE_NOTE: &str = "create_note";
    pub const RUN_APP_COMMAND: &str = "run_app_command";
    pub const GENERATE_IMAGE: &str = "generate_image";
    pub const UPLOAD_NOTE: &str = "upload_note";
    pub const UPLOAD_VIDEO: &str = "upload_video";
    pub const SOCIAL_LOGIN_PREPARE: &str = "social_login_prepare";
    pub const SOCIAL_CHECK_ACCOUNT: &str = "social_check_account";
}

/// 所有支持的工具名。
pub fn all() -> &'static [&'static str] {
    &[
        names::LIST_CAPABILITIES,
        names::START_TASK,
        names::GET_STATUS,
        names::LIST_DATA_FILES,
        names::READ_DATA_FILE,
        names::ARCHIVE_CRAWLER_DATA,
        names::CREATE_NOTE,
        names::RUN_APP_COMMAND,
        names::GENERATE_IMAGE,
        names::UPLOAD_NOTE,
        names::UPLOAD_VIDEO,
        names::SOCIAL_LOGIN_PREPARE,
        names::SOCIAL_CHECK_ACCOUNT,
    ]
}
