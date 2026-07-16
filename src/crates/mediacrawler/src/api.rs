//! MediaCrawler HTTP API 契约类型与服务骨架。
//!
//! 对齐原 MediaCrawler Python FastAPI（`api/schemas/crawler.py` + `api/main.py`），
//! 使前端 `operationsRuntimeService` / 原 mediacrawler-mcp 的契约可被 Rust 版直接复用：
//! `/api/health`、`/api/crawler/start|stop|status|logs`、`/api/data/files` 等。
//!
//! 与 Python 版的关键差异：**不再 subprocess 拉起 `python main.py`**，而是在进程内直接调用
//! [`crate::platform`] 的采集器；日志经 tokio mpsc 推送而非读子进程 stdout。

use crate::model::Platform;
use serde::{Deserialize, Serialize};

/// 平台代号（与原 API 字符串一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlatformCode {
    Xhs,
    Dy,
    Ks,
    Bili,
    Wb,
    Tieba,
    Zhihu,
}

impl PlatformCode {
    /// 转为归一化 [`Platform`]。
    pub fn to_platform(self) -> Platform {
        match self {
            PlatformCode::Xhs => Platform::Xhs,
            PlatformCode::Dy => Platform::Douyin,
            PlatformCode::Ks => Platform::Kuaishou,
            PlatformCode::Bili => Platform::Bilibili,
            PlatformCode::Wb => Platform::Weibo,
            PlatformCode::Tieba => Platform::Tieba,
            PlatformCode::Zhihu => Platform::Zhihu,
        }
    }
}

/// 登录方式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoginType {
    #[default]
    Qrcode,
    Phone,
    Cookie,
}

/// 采集类型。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrawlerType {
    #[default]
    Search,
    Detail,
    Creator,
}

/// 落盘格式（与原 save_option 一致；v1 实现 json/jsonl/sqlite/csv）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveOption {
    Csv,
    Db,
    Json,
    Jsonl,
    Sqlite,
    Mongodb,
    Excel,
}

/// `/api/crawler/start` 请求体（与原 `CrawlerStartRequest` 对齐）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerStartRequest {
    pub platform: PlatformCode,
    #[serde(default)]
    pub login_type: LoginType,
    #[serde(default)]
    pub crawler_type: CrawlerType,
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
    pub specified_ids: Option<String>,
    #[serde(default)]
    pub creator_ids: Option<String>,
    #[serde(default = "default_start_page")]
    pub start_page: i64,
    #[serde(default = "default_true")]
    pub enable_comments: bool,
    #[serde(default)]
    pub enable_sub_comments: bool,
    #[serde(default)]
    pub save_option: Option<SaveOption>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub headless: bool,
    #[serde(default)]
    pub login_only: bool,
    #[serde(default)]
    pub max_notes_count: Option<i64>,
    #[serde(default)]
    pub max_comments_count: Option<i64>,
}

impl CrawlerStartRequest {
    /// 校验：search 需 keywords；count 在 1..=10000。
    pub fn validate(&self) -> Result<(), String> {
        if matches!(self.crawler_type, CrawlerType::Search)
            && self.keywords.as_deref().is_none_or(str::is_empty)
        {
            return Err("search 模式必须提供 keywords".into());
        }
        if let Some(n) = self.max_notes_count {
            if !(1..=10000).contains(&n) {
                return Err("max_notes_count 必须在 1..=10000".into());
            }
        }
        if let Some(n) = self.max_comments_count {
            if !(1..=10000).contains(&n) {
                return Err("max_comments_count 必须在 1..=10000".into());
            }
        }
        Ok(())
    }
}

/// 任务运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Idle,
    Running,
    Stopping,
    Error,
}

/// `/api/crawler/status` 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerStatusResponse {
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crawler_type: Option<CrawlerType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// 日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Success,
    Debug,
}

/// 日志条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: String,
    pub timestamp: i64,
    pub level: LogLevel,
    pub message: String,
}

fn default_start_page() -> i64 {
    1
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_request_roundtrip_defaults() {
        let raw = r#"{"platform":"xhs","keywords":"猫爬架"}"#;
        let req: CrawlerStartRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.platform, PlatformCode::Xhs);
        assert_eq!(req.login_type, LoginType::Qrcode); // 默认
        assert_eq!(req.crawler_type, CrawlerType::Search);
        assert_eq!(req.start_page, 1);
        assert!(req.enable_comments); // 默认 true
        assert!(!req.enable_sub_comments);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn validate_requires_keywords_for_search() {
        let req = CrawlerStartRequest {
            platform: PlatformCode::Dy,
            login_type: LoginType::Cookie,
            crawler_type: CrawlerType::Search,
            keywords: None,
            specified_ids: None,
            creator_ids: None,
            start_page: 1,
            enable_comments: true,
            enable_sub_comments: false,
            save_option: None,
            cookies: None,
            headless: false,
            login_only: false,
            max_notes_count: None,
            max_comments_count: None,
        };
        let err = req.validate().unwrap_err();
        assert!(err.contains("keywords"));
    }

    #[test]
    fn validate_count_bounds() {
        let mut req = CrawlerStartRequest {
            platform: PlatformCode::Bili,
            login_type: LoginType::Cookie,
            crawler_type: CrawlerType::Search,
            keywords: Some("母婴".into()),
            specified_ids: None,
            creator_ids: None,
            start_page: 1,
            enable_comments: true,
            enable_sub_comments: false,
            save_option: None,
            cookies: None,
            headless: false,
            login_only: false,
            max_notes_count: Some(0),
            max_comments_count: None,
        };
        assert!(req.validate().is_err());
        req.max_notes_count = Some(50);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn platform_code_maps() {
        assert_eq!(PlatformCode::Dy.to_platform(), Platform::Douyin);
        assert_eq!(PlatformCode::Ks.to_platform(), Platform::Kuaishou);
    }
}
