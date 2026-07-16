use axum::http::StatusCode;
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum BossError {
    #[error("参数错误：{0}")]
    Validation(String),
    #[error("未找到：{0}")]
    NotFound(String),
    #[error("状态冲突：{0}")]
    Conflict(String),
    #[error("数据库错误：{0}")]
    Database(#[from] rusqlite::Error),
    #[error("文件错误：{0}")]
    Io(#[from] std::io::Error),
    #[error("数据格式错误：{0}")]
    Json(#[from] serde_json::Error),
    #[error("内部状态不可用：{0}")]
    State(String),
}

impl BossError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Database(_) | Self::Io(_) | Self::Json(_) | Self::State(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    pub fn response_body(&self) -> Value {
        json!({ "ok": false, "error": self.to_string() })
    }
}

pub type BossResult<T> = Result<T, BossError>;
