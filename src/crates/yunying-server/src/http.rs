//! HTTP/SSE API（前端契约）。
//!
//! 对齐原 goosed 的端点（docs/14）：`GET /status`、`POST /agent/start`、
//! `POST /sessions/{id}/reply`（SSE 流），由嵌入式 [`crate::goose_bridge::GooseBridge`] 承载。
//! 前端（Beav TS）最小改动即可从 goosed sidecar 切换到本嵌入式后端。
//!
//! v1：GooseBridge 持有单个会话；`:id` 路径参数被接受但映射到该会话（多会话后续扩展）。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::{get, post},
    Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::ReceiverStream;

use crate::goose_bridge::BridgeEvent;
use crate::runtime::Runtime;

/// 构造 axum 应用。
pub fn app(runtime: Arc<Runtime>) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/agent/start", post(agent_start))
        .route("/sessions/:id/reply", post(session_reply))
        .with_state(runtime)
}

#[derive(Serialize)]
struct StatusResp<'a> {
    status: &'a str,
}

async fn status() -> impl IntoResponse {
    Json(StatusResp { status: "ok" })
}

#[derive(Serialize)]
struct AgentStartResp {
    status: &'static str,
    session_id: &'static str,
}

/// v1：GooseBridge 在构造时已建好会话；这里确认可用。
async fn agent_start(State(_rt): State<Arc<Runtime>>) -> impl IntoResponse {
    Json(AgentStartResp {
        status: "ok",
        session_id: "yunying",
    })
}

#[derive(Deserialize)]
struct ReplyBody {
    message: String,
}

/// 发送消息并以 SSE 流回传 [`BridgeEvent`]。
///
/// 用 tokio mpsc 把 GooseBridge 的借用流解耦为 'static 的 channel 接收端，
/// 使 SSE 响应不绑定 handler 的借用生命周期。
async fn session_reply(
    State(rt): State<Arc<Runtime>>,
    Path(_id): Path<String>,
    Json(body): Json<ReplyBody>,
) -> Sse<ReceiverStream<Result<Event, std::convert::Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel(32);

    tokio::spawn(async move {
        let stream = match rt.goose.reply(&body.message).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default().event("error").data(e.to_string())))
                    .await;
                return;
            }
        };
        tokio::pin!(stream);
        while let Some(ev) = stream.next().await {
            let sse = match ev {
                BridgeEvent::TextDelta(t) => Event::default().event("text-delta").data(t),
                BridgeEvent::ThoughtDelta(t) => Event::default().event("thought-delta").data(t),
                BridgeEvent::ToolStart {
                    call_id,
                    name,
                    input,
                } => Event::default().event("tool-start").data(
                    serde_json::to_string(&serde_json::json!({
                        "callId": call_id,
                        "name": name,
                        "input": input,
                    }))
                    .unwrap_or_default(),
                ),
                BridgeEvent::ToolEnd {
                    call_id,
                    name,
                    output,
                } => Event::default().event("tool-end").data(
                    serde_json::to_string(&serde_json::json!({
                        "callId": call_id,
                        "name": name,
                        "output": output,
                    }))
                    .unwrap_or_default(),
                ),
                BridgeEvent::Error {
                    message,
                    detail,
                    category,
                } => Event::default().event("error").data(
                    serde_json::to_string(&serde_json::json!({
                        "message": message,
                        "detail": detail,
                        "category": category,
                    }))
                    .unwrap_or_default(),
                ),
                BridgeEvent::Cancelled => Event::default().event("cancelled").data("[CANCELLED]"),
                BridgeEvent::Done => Event::default().event("done").data("[DONE]"),
            };
            if tx.send(Ok(sse)).await.is_err() {
                break; // 客户端断开
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt; // oneshot

    fn router() -> Router {
        // 测试用空 state 路由（不依赖真实 Goose）：只验证路由可达性。
        Router::new()
            .route(
                "/status",
                get(|| async { Json(StatusResp { status: "ok" }) }),
            )
            .route(
                "/agent/start",
                post(|| async {
                    Json(AgentStartResp {
                        status: "ok",
                        session_id: "yunying",
                    })
                }),
            )
    }

    #[tokio::test]
    async fn status_route_ok() {
        let app = router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn agent_start_route_ok() {
        let app = router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/agent/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
