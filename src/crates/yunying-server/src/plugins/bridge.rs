//! 插件到九伴主程序的本地桥。
//!
//! Python 插件不直接持有火山引擎密钥；它把生成请求提交到这个 loopback 服务，最终仍由
//! `ipc::generation` 使用主程序设置中的 endpoint/key/model 完成轮询、下载与素材入库。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ipc::{dispatch_invoke, AppState};

pub const BRIDGE_PATH: &str = "/mcp/beav";
pub const APP_BRIDGE_URL_ENV: &str = "JIUBAN_APP_BRIDGE_URL";
pub const APP_BRIDGE_TOKEN_ENV: &str = "JIUBAN_APP_BRIDGE_TOKEN";
pub const OPENMONTAGE_BRIDGE_URL_ENV: &str = "BEAV_BRIDGE_URL";
pub const OPENMONTAGE_BRIDGE_PATH_ENV: &str = "BEAV_BRIDGE_PATH";
pub const OPENMONTAGE_BRIDGE_TOKEN_ENV: &str = "BEAV_BRIDGE_TOKEN";

/// 已绑定的本地插件桥端点。令牌刻意不实现 `Debug`，避免被启动日志意外打印。
#[derive(Clone)]
pub struct PluginBridgeEndpoint {
    address: SocketAddr,
    bearer_token: Arc<str>,
}

impl PluginBridgeEndpoint {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn request_url(&self) -> String {
        format!("{}{}", self.base_url(), BRIDGE_PATH)
    }

    /// 在任何 MCP 子进程启动前安装连接信息。子进程还会显式收到这些变量，
    /// 这里同时覆盖后续由插件管理页按需挂载的进程。
    pub fn install_process_environment(&self) {
        std::env::set_var(APP_BRIDGE_URL_ENV, self.request_url());
        std::env::set_var(APP_BRIDGE_TOKEN_ENV, self.bearer_token.as_ref());
        std::env::set_var(OPENMONTAGE_BRIDGE_URL_ENV, self.base_url());
        std::env::set_var(OPENMONTAGE_BRIDGE_PATH_ENV, BRIDGE_PATH);
        std::env::set_var(OPENMONTAGE_BRIDGE_TOKEN_ENV, self.bearer_token.as_ref());
    }
}

/// 先绑定随机 loopback 端口，再交给 Axum 提供服务。绑定失败会在 spawn 前返回，
/// 因而调用方可 fail closed，而不会留下一个看似就绪的调度器。
pub struct PluginBridgeServer {
    listener: tokio::net::TcpListener,
    endpoint: PluginBridgeEndpoint,
}

impl PluginBridgeServer {
    pub async fn bind_loopback() -> anyhow::Result<Self> {
        let listener =
            tokio::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await?;
        let address = listener.local_addr()?;
        let bearer_token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        Ok(Self {
            listener,
            endpoint: PluginBridgeEndpoint {
                address,
                bearer_token: Arc::from(bearer_token),
            },
        })
    }

    pub fn endpoint(&self) -> &PluginBridgeEndpoint {
        &self.endpoint
    }

    pub async fn serve(self, state: Arc<AppState>) -> anyhow::Result<()> {
        let router = router(state, self.endpoint.bearer_token);
        axum::serve(self.listener, router).await?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginBridgeRequest {
    action: String,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    source: String,
}

#[derive(Clone)]
struct PluginBridgeState {
    app: Arc<AppState>,
    bearer_token: Arc<str>,
}

fn router(state: Arc<AppState>, bearer_token: Arc<str>) -> Router {
    let state = PluginBridgeState {
        app: state,
        bearer_token,
    };
    Router::new()
        .route("/status", get(handle_status))
        .route(BRIDGE_PATH, post(handle_beav_bridge))
        .with_state(state)
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some((scheme, provided)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && !provided.is_empty()
        && constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "code": "JIUBAN_PLUGIN_BRIDGE_UNAUTHORIZED",
                "message": "A valid local bridge bearer token is required",
            }
        })),
    )
        .into_response()
}

async fn handle_status(State(state): State<PluginBridgeState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.bearer_token) {
        return unauthorized_response();
    }
    (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response()
}

async fn handle_beav_bridge(
    State(state): State<PluginBridgeState>,
    headers: HeaderMap,
    Json(request): Json<PluginBridgeRequest>,
) -> Response {
    if !authorized(&headers, &state.bearer_token) {
        return unauthorized_response();
    }
    if request.action != "app_cli" {
        return (
            StatusCode::OK,
            Json(json!({
                "error": {
                    "code": "JIUBAN_PLUGIN_ACTION_UNSUPPORTED",
                    "message": format!("Unsupported plugin bridge action: {}", request.action),
                }
            })),
        )
            .into_response();
    }

    let command = request
        .payload
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let payload = request
        .payload
        .get("payload")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let channel = match command {
        "video generate" => "video-gen:generate",
        "image generate" => "image-gen:generate",
        _ => {
            return (
                StatusCode::OK,
                Json(json!({
                    "error": {
                        "code": "JIUBAN_APP_CLI_COMMAND_UNSUPPORTED",
                        "message": format!("Unsupported app_cli command from plugin: {command}"),
                    }
                })),
            )
                .into_response();
        }
    };

    match dispatch_invoke(channel, payload, &state.app).await {
        Ok(generated) => {
            let success = generated
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let assets = generated
                .get("assets")
                .cloned()
                .unwrap_or_else(|| json!([]));
            let first_asset = assets
                .as_array()
                .and_then(|items| items.first())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let provider = first_asset
                .get("provider")
                .cloned()
                .unwrap_or_else(|| json!("jiuban-video"));
            let model = first_asset
                .get("model")
                .cloned()
                .unwrap_or_else(|| json!("app-managed"));
            (
                StatusCode::OK,
                Json(json!({
                    "result": {
                        "success": success,
                        "error": generated.get("error").cloned().unwrap_or(Value::Null),
                        "data": {
                            "assets": assets,
                            "provider": provider,
                            "model": model,
                            "managedBy": channel,
                            "source": request.source,
                        }
                    }
                })),
            )
                .into_response()
        }
        Err(error) => (
            StatusCode::OK,
            Json(json!({
                "error": {
                    "code": "JIUBAN_PLUGIN_BRIDGE_FAILED",
                    "message": error.to_string(),
                }
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state() -> Arc<AppState> {
        let data = tempfile::tempdir().unwrap().keep();
        let db = crate::db::Db::open(&data.join("bridge-auth.db")).unwrap();
        Arc::new(AppState {
            redclaw_scheduler: crate::ipc::redclaw_runner::RedClawScheduler::inactive(db.clone()),
            db,
            goose: crate::goose_bridge::GooseBridge::default(),
            emitter: Arc::new(crate::ipc::NoopEmitter),
            login: Arc::new(crate::login::LoginService::new(Arc::new(
                crate::login::StubLoginDriver,
            ))),
        })
    }

    #[tokio::test]
    async fn bridge_rejects_missing_or_wrong_bearer_and_accepts_valid_bearer() {
        let app = router(test_state(), Arc::from("local-test-token"));
        let body = r#"{"action":"unsupported","payload":{}}"#;

        let missing = app
            .clone()
            .oneshot(
                Request::post(BRIDGE_PATH)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = app
            .clone()
            .oneshot(
                Request::post(BRIDGE_PATH)
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, "Bearer wrong-token")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                Request::post(BRIDGE_PATH)
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, "Bearer local-test-token")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bound_bridge_uses_loopback_random_port_and_unique_secret() {
        let first = PluginBridgeServer::bind_loopback().await.unwrap();
        let second = PluginBridgeServer::bind_loopback().await.unwrap();

        assert!(first.endpoint().address().ip().is_loopback());
        assert_ne!(first.endpoint().address().port(), 0);
        assert_ne!(
            first.endpoint().bearer_token,
            second.endpoint().bearer_token
        );
    }
}
