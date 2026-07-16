use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Extension, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::BossCore;

#[derive(Debug, Deserialize)]
struct ToolRequest {
    name: String,
    #[serde(default, alias = "args")]
    arguments: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthScope {
    Local,
    Owner,
    Employee,
}

#[derive(Clone, Debug)]
struct AuthState {
    required: bool,
    owner_token: Option<Arc<str>>,
    employee_token: Option<Arc<str>>,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    address: SocketAddr,
    auth: AuthState,
}

impl ServerConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let address = std::env::var("BOSS_SERVER_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
            .parse()?;
        let allow_remote = matches!(
            std::env::var("BOSS_ALLOW_REMOTE").as_deref(),
            Ok("1" | "true" | "TRUE" | "yes" | "YES")
        );
        Self::new(
            address,
            allow_remote,
            std::env::var("BOSS_SERVER_TOKEN").ok(),
            std::env::var("BOSS_EMPLOYEE_TOKEN").ok(),
        )
    }

    fn new(
        address: SocketAddr,
        allow_remote: bool,
        owner_token: Option<String>,
        employee_token: Option<String>,
    ) -> anyhow::Result<Self> {
        if address.ip().is_loopback() {
            return Ok(Self {
                address,
                auth: AuthState {
                    required: false,
                    owner_token: None,
                    employee_token: None,
                },
            });
        }
        if !allow_remote {
            anyhow::bail!(
                "拒绝监听非 loopback 地址 {address}；如确需远程同步，请显式设置 BOSS_ALLOW_REMOTE=1"
            );
        }
        let owner_token = normalized_token(owner_token, "BOSS_SERVER_TOKEN")?;
        let employee_token = normalized_token(employee_token, "BOSS_EMPLOYEE_TOKEN")?;
        if token_matches(&owner_token, &employee_token) {
            anyhow::bail!("BOSS_SERVER_TOKEN 与 BOSS_EMPLOYEE_TOKEN 必须使用不同值");
        }
        Ok(Self {
            address,
            auth: AuthState {
                required: true,
                owner_token: Some(Arc::from(owner_token)),
                employee_token: Some(Arc::from(employee_token)),
            },
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

fn normalized_token(token: Option<String>, name: &str) -> anyhow::Result<String> {
    let token = token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("远程模式必须配置非空 {name}"))?;
    if token.len() < 32 {
        anyhow::bail!("远程模式的 {name} 至少需要 32 字节");
    }
    Ok(token)
}

fn token_matches(supplied: &str, expected: &str) -> bool {
    bool::from(supplied.as_bytes().ct_eq(expected.as_bytes()))
}

fn authorization_scope(auth: &AuthState, authorization: Option<&str>) -> Option<AuthScope> {
    if !auth.required {
        return Some(AuthScope::Local);
    }
    let (scheme, supplied) = authorization?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || supplied.is_empty() {
        return None;
    }
    if auth
        .owner_token
        .as_deref()
        .is_some_and(|expected| token_matches(supplied, expected))
    {
        return Some(AuthScope::Owner);
    }
    if auth
        .employee_token
        .as_deref()
        .is_some_and(|expected| token_matches(supplied, expected))
    {
        return Some(AuthScope::Employee);
    }
    None
}

fn tool_allowed(scope: AuthScope, name: &str) -> bool {
    scope != AuthScope::Employee || name.starts_with("employee.")
}

async fn require_bearer(
    State(auth): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let Some(scope) = authorization_scope(&auth, authorization) else {
        let mut response = (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": "未授权" })),
        )
            .into_response();
        response
            .headers_mut()
            .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    };
    request.extensions_mut().insert(scope);
    next.run(request).await
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "boss-server", "backend": "rust" }))
}

async fn call_tool(
    State(core): State<BossCore>,
    Extension(scope): Extension<AuthScope>,
    Json(request): Json<ToolRequest>,
) -> Response {
    if !tool_allowed(scope, &request.name) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "ok": false,
                "error": "员工凭据只能调用 employee.* 工具"
            })),
        )
            .into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let arguments = if request.arguments.is_null() {
            json!({})
        } else {
            request.arguments
        };
        core.dispatch_tool(&request.name, arguments)
    })
    .await;
    match result {
        Ok(Ok(result)) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "result": result })),
        )
            .into_response(),
        Ok(Err(error)) => (error.status_code(), Json(error.response_body())).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("工具任务异常：{error}") })),
        )
            .into_response(),
    }
}

pub fn router(core: BossCore, config: &ServerConfig) -> Router {
    let origins = [
        HeaderValue::from_static("http://127.0.0.1:5181"),
        HeaderValue::from_static("http://localhost:5181"),
    ];
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::POST, Method::GET])
        .allow_headers([CONTENT_TYPE, AUTHORIZATION]);
    Router::new()
        .route("/health", get(health))
        .route("/api/boss/tool", post(call_tool))
        .layer(middleware::from_fn_with_state(
            config.auth.clone(),
            require_bearer,
        ))
        .layer(DefaultBodyLimit::max(22 * 1024 * 1024))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(core)
}

pub async fn serve(core: BossCore, config: ServerConfig) -> anyhow::Result<()> {
    if !config.address.ip().is_loopback() {
        tracing::warn!(
            address = %config.address,
            "远程模式已启用 owner/employee 分权 Bearer 鉴权；仍建议在反向代理层配置 TLS"
        );
    }
    let listener = tokio::net::TcpListener::bind(config.address).await?;
    tracing::info!(address = %config.address, "Rust boss-server 已启动");
    axum::serve(listener, router(core, &config)).await?;
    Ok(())
}

pub async fn serve_from_env(core: BossCore) -> anyhow::Result<()> {
    serve(core, ServerConfig::from_env()?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_remains_token_optional() {
        let address: SocketAddr = "127.0.0.1:8787".parse().unwrap();
        let config = ServerConfig::new(address, false, None, None).expect("loopback config");
        assert_eq!(
            authorization_scope(&config.auth, None),
            Some(AuthScope::Local)
        );
    }

    #[test]
    fn remote_listener_requires_opt_in_and_distinct_scoped_tokens() {
        let address: SocketAddr = "0.0.0.0:8787".parse().unwrap();
        let owner = "owner-token-0123456789abcdef0123456789abcdef";
        let employee = "employee-token-0123456789abcdef0123456789abcdef";
        assert!(ServerConfig::new(
            address,
            false,
            Some(owner.to_string()),
            Some(employee.to_string())
        )
        .is_err());
        assert!(ServerConfig::new(address, true, Some(owner.to_string()), None).is_err());
        assert!(ServerConfig::new(address, true, None, Some(employee.to_string())).is_err());
        assert!(ServerConfig::new(
            address,
            true,
            Some("weak-owner".to_string()),
            Some(employee.to_string())
        )
        .is_err());
        let same = "same-token-0123456789abcdef0123456789abcdef";
        assert!(ServerConfig::new(
            address,
            true,
            Some(same.to_string()),
            Some(same.to_string())
        )
        .is_err());
    }

    #[test]
    fn owner_and_employee_tokens_have_distinct_tool_scope() {
        let address: SocketAddr = "0.0.0.0:8787".parse().unwrap();
        let owner_token = "owner-secret-0123456789abcdef0123456789abcdef";
        let employee_token = "employee-secret-0123456789abcdef0123456789abcdef";
        let config = ServerConfig::new(
            address,
            true,
            Some(owner_token.to_string()),
            Some(employee_token.to_string()),
        )
        .expect("remote config");
        let owner_header = format!("Bearer {owner_token}");
        let employee_header = format!("Bearer {employee_token}");
        let owner = authorization_scope(&config.auth, Some(&owner_header)).unwrap();
        let employee = authorization_scope(&config.auth, Some(&employee_header)).unwrap();
        assert_eq!(owner, AuthScope::Owner);
        assert_eq!(employee, AuthScope::Employee);
        assert!(tool_allowed(owner, "ledger.report"));
        assert!(tool_allowed(owner, "employee.report_work_event"));
        assert!(tool_allowed(employee, "employee.report_work_event"));
        assert!(tool_allowed(employee, "employee.list_week_reviews"));
        assert!(!tool_allowed(employee, "boss.owner_context"));
        assert!(!tool_allowed(employee, "ledger.add_transaction"));
        assert_eq!(
            authorization_scope(&config.auth, Some("Bearer wrong")),
            None
        );
    }
}
