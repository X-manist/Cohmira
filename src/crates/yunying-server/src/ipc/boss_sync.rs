//! 员工端到老板端的显式工作上报桥。
//!
//! 网络请求始终从 Rust 后端发出，渲染层不会接触 `BOSS_EMPLOYEE_TOKEN`。当前只暴露
//! `employee.report_work_event` 与 `employee.list_week_reviews` 两个最小权限工具。

use std::time::Duration;

use anyhow::{anyhow, Context};
use reqwest::{Client, StatusCode, Url};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::AppState;

const DEFAULT_BOSS_SERVER_URL: &str = "http://127.0.0.1:8787";
const TOOL_ENDPOINT_PATH: &str = "/api/boss/tool";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct BossSyncConfig {
    endpoint: Url,
    token: Option<String>,
    owner_code: String,
    employee_id: String,
    employee_name: String,
    employee_role: String,
}

impl BossSyncConfig {
    fn from_env() -> anyhow::Result<Self> {
        let server_url = env_or_default("BOSS_SERVER_URL", DEFAULT_BOSS_SERVER_URL);
        let token = optional_env("BOSS_EMPLOYEE_TOKEN");
        let owner_code = required_env("BOSS_OWNER_CODE")?;
        let employee_id = required_env("EMPLOYEE_ID")?;
        let employee_name = required_env("EMPLOYEE_NAME")?;
        let employee_role = env_or_default("EMPLOYEE_ROLE", "员工");
        Self::new(
            &server_url,
            token,
            owner_code,
            employee_id,
            employee_name,
            employee_role,
        )
    }

    fn new(
        server_url: &str,
        token: Option<String>,
        owner_code: String,
        employee_id: String,
        employee_name: String,
        employee_role: String,
    ) -> anyhow::Result<Self> {
        let endpoint = tool_endpoint(server_url)?;
        let token = token.and_then(non_blank);
        if !url_is_loopback(&endpoint) {
            let remote_token = token.as_deref().ok_or_else(|| {
                anyhow!("远程老板端必须配置员工专用 BOSS_EMPLOYEE_TOKEN（至少 32 字节）")
            })?;
            if remote_token.len() < 32 {
                return Err(anyhow!("远程 BOSS_EMPLOYEE_TOKEN 必须至少 32 字节"));
            }
        }
        Ok(Self {
            endpoint,
            token,
            owner_code: non_empty("BOSS_OWNER_CODE", owner_code)?,
            employee_id: non_empty("EMPLOYEE_ID", employee_id)?,
            employee_name: non_empty("EMPLOYEE_NAME", employee_name)?,
            employee_role: non_empty("EMPLOYEE_ROLE", employee_role)?,
        })
    }
}

/// 员工端老板同步命名空间。所有调用均为 I/O-bound async 请求，不持有数据库锁。
pub async fn invoke(channel: &str, payload: Value, _state: &AppState) -> anyhow::Result<Value> {
    let config = BossSyncConfig::from_env()?;
    match channel {
        "boss-sync:report-work-event" => report_work_event(&config, &payload).await,
        "boss-sync:list-week-reviews" => list_week_reviews(&config, &payload).await,
        "boss-sync:connection-info" => Ok(json!({
            "serverUrl": public_server_url(&config.endpoint),
            "ownerCode": config.owner_code,
            "employeeId": config.employee_id,
            "employeeName": config.employee_name,
            "employeeRole": config.employee_role,
            "tokenConfigured": config.token.is_some(),
        })),
        other => Err(anyhow!("boss-sync 命名空间未实现通道: {other}")),
    }
}

async fn report_work_event(config: &BossSyncConfig, payload: &Value) -> anyhow::Result<Value> {
    let payload = object_payload(payload)?;
    let summary = required_payload_string(payload, "summary")?;
    let task_type = required_payload_string(payload, "task_type")?;
    let raw_event_id = required_payload_string(payload, "event_id")?;
    let event_id = bounded_event_id(&raw_event_id);

    let mut arguments = Map::new();
    arguments.insert("owner_code".into(), json!(config.owner_code));
    arguments.insert("event_id".into(), json!(event_id));
    arguments.insert("employee_id".into(), json!(config.employee_id));
    arguments.insert("employee_name".into(), json!(config.employee_name));
    arguments.insert("role".into(), json!(config.employee_role));
    arguments.insert("employee_status".into(), json!("active"));
    arguments.insert("task_type".into(), json!(task_type));
    arguments.insert("summary".into(), json!(summary));
    copy_optional_fields(
        payload,
        &mut arguments,
        &[
            "event_date",
            "material_count",
            "cost_cents",
            "quality_score",
            "employee_status",
        ],
    );

    call_tool(
        config,
        "employee.report_work_event",
        Value::Object(arguments),
    )
    .await
}

async fn list_week_reviews(config: &BossSyncConfig, payload: &Value) -> anyhow::Result<Value> {
    let payload = object_payload(payload)?;
    let mut arguments = Map::new();
    arguments.insert("owner_code".into(), json!(config.owner_code));
    arguments.insert("employee_id".into(), json!(config.employee_id));
    copy_optional_fields(
        payload,
        &mut arguments,
        &["date_from", "date_to", "week_start", "week_end"],
    );
    call_tool(
        config,
        "employee.list_week_reviews",
        Value::Object(arguments),
    )
    .await
}

async fn call_tool(
    config: &BossSyncConfig,
    name: &'static str,
    arguments: Value,
) -> anyhow::Result<Value> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(12))
        .build()
        .context("无法初始化老板端连接")?;
    let mut request = client
        .post(config.endpoint.clone())
        .json(&json!({ "name": name, "arguments": arguments }));
    if let Some(token) = config.token.as_deref() {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .await
        .context("无法连接老板端，请确认 BOSS_SERVER_URL 与老板端服务状态")?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(anyhow!("老板端响应超过 1 MiB，已拒绝处理"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("读取老板端响应失败")? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(anyhow!("老板端响应超过 1 MiB，已拒绝处理"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes).context("老板端返回了无效 JSON")?;
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(anyhow!(
            "老板端认证失败，请检查 BOSS_EMPLOYEE_TOKEN 与老板端代理配置"
        ));
    }
    if !status.is_success() || body.get("ok").and_then(Value::as_bool) != Some(true) {
        let message = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("老板端拒绝了请求");
        return Err(anyhow!("老板端请求失败（HTTP {status}）：{message}"));
    }
    body.get("result")
        .cloned()
        .ok_or_else(|| anyhow!("老板端响应缺少 result"))
}

fn tool_endpoint(server_url: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(server_url.trim()).context("BOSS_SERVER_URL 不是有效 URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("BOSS_SERVER_URL 仅支持 http 或 https"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(anyhow!("BOSS_SERVER_URL 不允许包含用户名或密码"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(anyhow!("BOSS_SERVER_URL 不允许包含 query 或 fragment"));
    }
    if url.host_str().is_none() {
        return Err(anyhow!("BOSS_SERVER_URL 缺少主机名"));
    }
    if url.scheme() == "http" && !url_is_loopback(&url) {
        return Err(anyhow!(
            "远程老板端必须使用 https；明文 http 只允许 localhost/loopback"
        ));
    }
    match url.path().trim_end_matches('/') {
        "" => url.set_path(TOOL_ENDPOINT_PATH),
        TOOL_ENDPOINT_PATH => url.set_path(TOOL_ENDPOINT_PATH),
        _ => {
            return Err(anyhow!(
                "BOSS_SERVER_URL 应为服务根地址或 {TOOL_ENDPOINT_PATH}"
            ))
        }
    }
    Ok(url)
}

fn url_is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

fn public_server_url(endpoint: &Url) -> String {
    let mut url = endpoint.clone();
    url.set_path("");
    url.to_string().trim_end_matches('/').to_string()
}

fn required_env(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .ok()
        .and_then(non_blank)
        .ok_or_else(|| anyhow!("未配置 {name}，请在启动员工端前设置该环境变量"))
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(non_blank)
}

fn env_or_default(name: &str, default: &str) -> String {
    optional_env(name).unwrap_or_else(|| default.to_string())
}

fn non_blank(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn non_empty(name: &str, value: String) -> anyhow::Result<String> {
    non_blank(value).ok_or_else(|| anyhow!("{name} 不能为空"))
}

fn object_payload(payload: &Value) -> anyhow::Result<&Map<String, Value>> {
    payload
        .as_object()
        .ok_or_else(|| anyhow!("boss-sync payload 必须是 JSON 对象"))
}

fn required_payload_string(payload: &Map<String, Value>, name: &str) -> anyhow::Result<String> {
    payload
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("缺少 {name}"))
}

fn copy_optional_fields(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    keys: &[&str],
) {
    for key in keys {
        if let Some(value) = source.get(*key) {
            target.insert((*key).to_string(), value.clone());
        }
    }
}

fn bounded_event_id(raw: &str) -> String {
    if raw.len() <= 160 {
        return raw.to_string();
    }
    let digest = Sha256::digest(raw.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("workboard-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode as AxumStatus};
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Option<Value>>>);

    async fn fake_tool(
        State(capture): State<Capture>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (AxumStatus, Json<Value>) {
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some("Bearer test-secret")
        {
            return (
                AxumStatus::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "unauthorized" })),
            );
        }
        *capture.0.lock().await = Some(body.clone());
        (
            AxumStatus::OK,
            Json(json!({
                "ok": true,
                "result": { "accepted": true, "tool": body["name"] }
            })),
        )
    }

    fn test_config(server_url: String) -> BossSyncConfig {
        BossSyncConfig::new(
            &server_url,
            Some("test-secret".into()),
            "BOSS-7429".into(),
            "employee-1".into(),
            "测试员工".into(),
            "内容运营".into(),
        )
        .expect("valid test config")
    }

    #[test]
    fn endpoint_accepts_only_safe_shapes() {
        assert_eq!(
            tool_endpoint("http://127.0.0.1:8787")
                .expect("local endpoint")
                .as_str(),
            "http://127.0.0.1:8787/api/boss/tool"
        );
        assert!(tool_endpoint("http://boss.example.com").is_err());
        assert!(tool_endpoint("https://boss.example.com/other").is_err());
        assert!(tool_endpoint("file:///tmp/boss.sock").is_err());
        assert!(BossSyncConfig::new(
            "https://boss.example.com",
            None,
            "BOSS-7429".into(),
            "employee-1".into(),
            "测试员工".into(),
            "内容运营".into(),
        )
        .is_err());
        assert!(BossSyncConfig::new(
            "https://boss.example.com",
            Some("too-short".into()),
            "BOSS-7429".into(),
            "employee-1".into(),
            "测试员工".into(),
            "内容运营".into(),
        )
        .is_err());
    }

    #[test]
    fn long_event_ids_are_stably_bounded() {
        let raw = "x".repeat(300);
        let first = bounded_event_id(&raw);
        assert!(first.len() <= 160);
        assert_eq!(first, bounded_event_id(&raw));
    }

    #[tokio::test]
    async fn report_uses_bearer_token_and_employee_identity() {
        let capture = Capture::default();
        let app = Router::new()
            .route(TOOL_ENDPOINT_PATH, post(fake_tool))
            .with_state(capture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test router");
        });

        let result = report_work_event(
            &test_config(format!("http://{address}")),
            &json!({
                "event_id": "workboard:generation:42",
                "event_date": "2026-07-15",
                "task_type": "generation",
                "summary": "完成视频素材生成"
            }),
        )
        .await
        .expect("report succeeds");
        assert_eq!(result["accepted"], true);
        let captured = capture.0.lock().await.clone().expect("captured request");
        assert_eq!(captured["name"], "employee.report_work_event");
        assert_eq!(captured["arguments"]["employee_id"], "employee-1");
        assert_eq!(captured["arguments"]["employee_name"], "测试员工");
        assert_eq!(captured["arguments"]["summary"], "完成视频素材生成");
        server.abort();
    }

    #[tokio::test]
    async fn authentication_errors_never_expose_the_token_value() {
        let capture = Capture::default();
        let app = Router::new()
            .route(TOOL_ENDPOINT_PATH, post(fake_tool))
            .with_state(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test router");
        });
        let secret = "employee-wrong-secret-that-must-not-leak";
        let config = BossSyncConfig::new(
            &format!("http://{address}"),
            Some(secret.into()),
            "BOSS-7429".into(),
            "employee-1".into(),
            "测试员工".into(),
            "内容运营".into(),
        )
        .expect("valid local config");
        let error = call_tool(&config, "employee.list_week_reviews", json!({}))
            .await
            .expect_err("wrong token must fail")
            .to_string();
        assert!(!error.contains(secret));
        assert!(error.contains("BOSS_EMPLOYEE_TOKEN"));
        server.abort();
    }
}
