//! RedClaw 的持久化异步调度器。
//!
//! 调度定义和执行状态存放在 `agent_tasks`，Tokio 监督循环只负责认领到期任务。
//! 认领通过带状态条件的 SQLite UPDATE 完成，确保手动触发与定时触发不会重复执行。
//! Agent / MCP I/O 在锁外执行；暂停、删除和应用退出通过 CancellationToken 取消。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Datelike, Local, LocalResult, NaiveDate, TimeZone};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{EventEmitter, NoopEmitter};
use crate::db::Db;
use crate::goose_bridge::{BridgeEvent, GooseBridge};

const CONFIG_ROW_ID: &str = "redclaw-runner-config";
const CONFIG_TASK_TYPE: &str = "redclaw-runner-config";
const SCHEDULED_TASK_TYPE: &str = "redclaw-scheduled";
const LONG_CYCLE_TASK_TYPE: &str = "redclaw-long-cycle";
const MAX_RESULT_CHARS: usize = 32_000;
const MAX_RECEIPT_BYTES: usize = 512 * 1024;
const MAX_RECEIPT_STRING_CHARS: usize = 32_000;
const MAX_TOOL_CALLS_PER_EXECUTION: usize = 128;
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const MAX_TASK_INTERVAL_MINUTES: i64 = 30 * 24 * 60;
const LEASE_TTL_MS: i64 = 60_000;
const LEASE_RENEW_MARGIN_MS: i64 = 20_000;
const CANCELLATION_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectState {
    project_id: String,
    enabled: bool,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    last_run_at: Option<i64>,
    #[serde(default)]
    last_result: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatConfig {
    enabled: bool,
    interval_minutes: i64,
    suppress_empty_report: bool,
    report_to_main_session: bool,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    last_run_at: Option<i64>,
    #[serde(default)]
    next_run_at: Option<i64>,
    #[serde(default)]
    last_digest: Option<String>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 30,
            suppress_empty_report: true,
            report_to_main_session: true,
            prompt: None,
            last_run_at: None,
            next_run_at: None,
            last_digest: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerConfig {
    enabled: bool,
    interval_minutes: i64,
    keep_alive_when_no_window: bool,
    max_projects_per_tick: i64,
    max_automation_per_tick: i64,
    #[serde(default)]
    project_states: HashMap<String, ProjectState>,
    #[serde(default)]
    heartbeat: HeartbeatConfig,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            // 创建定时任务后应按时执行；用户仍可通过 runner-stop 全局暂停。
            enabled: true,
            interval_minutes: 20,
            keep_alive_when_no_window: false,
            max_projects_per_tick: 2,
            max_automation_per_tick: 2,
            project_states: HashMap::new(),
            heartbeat: HeartbeatConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTask {
    id: String,
    name: String,
    enabled: bool,
    mode: String,
    prompt: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    work_item_id: Option<String>,
    #[serde(default)]
    subagent_roles: Vec<String>,
    #[serde(default)]
    required_tools: Vec<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    authorized_tools: Vec<String>,
    #[serde(default)]
    interval_minutes: Option<i64>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    weekdays: Vec<i64>,
    #[serde(default)]
    run_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
    #[serde(default)]
    last_run_at: Option<i64>,
    #[serde(default)]
    last_result: Option<String>,
    #[serde(default)]
    last_output: Option<String>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_run_at: Option<i64>,
    #[serde(default)]
    last_receipt_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LongCycleTask {
    id: String,
    name: String,
    enabled: bool,
    status: String,
    objective: String,
    step_prompt: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    work_item_id: Option<String>,
    #[serde(default)]
    subagent_roles: Vec<String>,
    #[serde(default)]
    required_tools: Vec<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    authorized_tools: Vec<String>,
    interval_minutes: i64,
    total_rounds: i64,
    completed_rounds: i64,
    created_at: i64,
    updated_at: i64,
    #[serde(default)]
    last_run_at: Option<i64>,
    #[serde(default)]
    last_result: Option<String>,
    #[serde(default)]
    last_output: Option<String>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_run_at: Option<i64>,
    #[serde(default)]
    last_receipt_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskKind {
    Scheduled,
    LongCycle,
}

impl TaskKind {
    fn task_type(self) -> &'static str {
        match self {
            Self::Scheduled => SCHEDULED_TASK_TYPE,
            Self::LongCycle => LONG_CYCLE_TASK_TYPE,
        }
    }

    fn receipt_label(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::LongCycle => "long-cycle",
        }
    }
}

#[derive(Clone, Debug)]
struct DueTask {
    id: String,
    kind: TaskKind,
}

#[derive(Clone, Debug)]
struct RuntimeState {
    status: String,
    started_at: Option<i64>,
}

#[derive(Clone, Debug)]
struct ClaimedTask {
    id: String,
    kind: TaskKind,
    execution_id: String,
    prompt: String,
    required_tools: Vec<String>,
    allowed_tools: Vec<String>,
    authorized_tools: Vec<String>,
    started_at: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolEvidence {
    call_id: String,
    name: String,
    input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
}

#[derive(Clone, Debug)]
struct ExecutionOutput {
    response: String,
    tools: Vec<ToolEvidence>,
}

#[derive(Clone, Debug)]
enum ExecutionFailure {
    Cancelled,
    Failed(String),
    ToolFailed {
        error: String,
        tools: Vec<ToolEvidence>,
    },
}

#[async_trait]
trait TaskExecutor: Send + Sync {
    async fn execute(
        &self,
        task: &ClaimedTask,
        cancel: CancellationToken,
    ) -> Result<ExecutionOutput, ExecutionFailure>;
}

struct GooseTaskExecutor {
    goose: GooseBridge,
}

#[async_trait]
impl TaskExecutor for GooseTaskExecutor {
    async fn execute(
        &self,
        task: &ClaimedTask,
        cancel: CancellationToken,
    ) -> Result<ExecutionOutput, ExecutionFailure> {
        let session_id = format!("redclaw:{}", task.execution_id);
        let result = self.execute_session(task, cancel).await;
        if let Err(error) = self.goose.close_chat_session(&session_id).await {
            if let Some(tools) = result.as_ref().ok().map(|output| output.tools.clone()) {
                return Err(ExecutionFailure::ToolFailed {
                    error: format!("后台 Agent 会话清理失败，任务未记录为成功：{error}"),
                    tools,
                });
            }
            tracing::warn!("RedClaw session {session_id} cleanup failed: {error}");
        }
        result
    }
}

impl GooseTaskExecutor {
    async fn execute_session(
        &self,
        task: &ClaimedTask,
        cancel: CancellationToken,
    ) -> Result<ExecutionOutput, ExecutionFailure> {
        let session_id = format!("redclaw:{}", task.execution_id);
        if task.allowed_tools.is_empty() {
            return Err(ExecutionFailure::Failed(
                "任务缺少 allowedTools 授权快照；默认拒绝全部工具".into(),
            ));
        }
        let required_not_allowed: Vec<&str> = task
            .required_tools
            .iter()
            .filter(|required| {
                !task
                    .allowed_tools
                    .iter()
                    .any(|allowed| tool_name_matches(allowed, required))
            })
            .map(String::as_str)
            .collect();
        if !required_not_allowed.is_empty() {
            return Err(ExecutionFailure::Failed(format!(
                "requiredTools 不在 allowedTools 授权中：{}",
                required_not_allowed.join(", ")
            )));
        }
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => return Err(ExecutionFailure::Cancelled),
            _ = tokio::time::sleep(EXECUTION_TIMEOUT) => {
                return Err(ExecutionFailure::Failed("Agent 会话初始化超过 45 分钟，已取消".into()));
            }
            result = self.goose.reply_for_session_with_allowed_tools(
                &session_id,
                &task.prompt,
                &task.allowed_tools,
            ) => {
                result.map_err(|error| ExecutionFailure::Failed(error.to_string()))?
            }
        };
        let deadline = tokio::time::sleep(EXECUTION_TIMEOUT);
        tokio::pin!(deadline);
        let mut response = String::new();
        let mut errors = Vec::new();
        let mut tools: Vec<ToolEvidence> = Vec::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.goose.cancel_chat_session(&session_id).await;
                    return Err(ExecutionFailure::Cancelled);
                }
                _ = &mut deadline => {
                    self.goose.cancel_chat_session(&session_id).await;
                    return Err(ExecutionFailure::ToolFailed {
                        error: "Agent 执行超过 45 分钟，已取消".into(),
                        tools,
                    });
                }
                event = stream.next() => {
                    match event {
                        Some(BridgeEvent::TextDelta(text)) => response.push_str(&text),
                        Some(BridgeEvent::ThoughtDelta(_)) => {}
                        Some(BridgeEvent::ToolStart { call_id, name, input }) => {
                            if tools.iter().any(|tool| tool.name == name) {
                                self.goose.cancel_chat_session(&session_id).await;
                                return Err(ExecutionFailure::ToolFailed {
                                    error: format!("同一工具每次执行最多调用一次，已拒绝重复调用：{name}"),
                                    tools,
                                });
                            }
                            if tools.len() >= MAX_TOOL_CALLS_PER_EXECUTION {
                                self.goose.cancel_chat_session(&session_id).await;
                                return Err(ExecutionFailure::ToolFailed {
                                    error: "单次任务工具调用超过 128 次，已停止以限制费用与回执大小".into(),
                                    tools,
                                });
                            }
                            tools.push(ToolEvidence { call_id, name, input, output: None });
                        }
                        Some(BridgeEvent::ToolEnd { call_id, name, output }) => {
                            let completed_tool_name = name.clone();
                            if let Some(tool) = tools.iter_mut().rev().find(|tool| tool.call_id == call_id) {
                                tool.output = Some(output);
                            } else {
                                tools.push(ToolEvidence { call_id, name, input: Value::Null, output: Some(output) });
                            }
                            if let Err(error) = self
                                .goose
                                .revoke_chat_session_tool(&session_id, &completed_tool_name)
                                .await
                            {
                                self.goose.cancel_chat_session(&session_id).await;
                                return Err(ExecutionFailure::ToolFailed {
                                    error: format!("工具单次调用授权撤销失败，已停止任务：{error}"),
                                    tools,
                                });
                            }
                        }
                        Some(BridgeEvent::Error { message, detail, .. }) => {
                            errors.push(format!("{message}: {detail}"));
                        }
                        Some(BridgeEvent::Cancelled) => return Err(ExecutionFailure::Cancelled),
                        Some(BridgeEvent::Done) | None => break,
                    }
                }
            }
        }

        if !errors.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: errors.join("\n"),
                tools,
            });
        }
        let unauthorized_tools: Vec<String> = tools
            .iter()
            .filter(|tool| {
                !task
                    .allowed_tools
                    .iter()
                    .any(|allowed| tool_name_matches(&tool.name, allowed))
            })
            .map(|tool| tool.name.clone())
            .collect();
        if !unauthorized_tools.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: format!(
                    "Agent 尝试调用 allowedTools 之外的工具：{}",
                    unauthorized_tools.join(", ")
                ),
                tools,
            });
        }
        let unauthorized_high_risk: Vec<String> = tools
            .iter()
            .filter(|tool| requires_explicit_authorization(&tool.name))
            .filter(|tool| {
                !task
                    .authorized_tools
                    .iter()
                    .any(|authorized| tool_name_matches(&tool.name, authorized))
            })
            .map(|tool| tool.name.clone())
            .collect();
        if !unauthorized_high_risk.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: format!(
                    "付费或发布工具缺少任务级显式授权：{}",
                    unauthorized_high_risk.join(", ")
                ),
                tools,
            });
        }
        let invalid_inputs: Vec<&str> = tools
            .iter()
            .filter(|tool| contains_disallowed_tool_input(&tool.input))
            .map(|tool| tool.name.as_str())
            .collect();
        if !invalid_inputs.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: format!(
                    "工具输入仍是 dry-run，拒绝把任务记录为成功：{}",
                    invalid_inputs.join(", ")
                ),
                tools,
            });
        }
        let failed_tools: Vec<&str> = tools
            .iter()
            .filter(|tool| {
                tool.output
                    .as_ref()
                    .is_some_and(contains_explicit_tool_failure)
            })
            .map(|tool| tool.name.as_str())
            .collect();
        if !failed_tools.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: format!(
                    "工具明确返回失败，拒绝把任务记录为成功：{}",
                    failed_tools.join(", ")
                ),
                tools,
            });
        }
        let incomplete_tools: Vec<&str> = tools
            .iter()
            .filter(|tool| tool.output.is_none())
            .map(|tool| tool.name.as_str())
            .collect();
        if !incomplete_tools.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: format!(
                    "工具调用没有返回结果，拒绝把任务记录为成功：{}",
                    incomplete_tools.join(", ")
                ),
                tools,
            });
        }
        let missing_tools = missing_required_tools(&task.required_tools, &tools);
        if !missing_tools.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: format!(
                    "任务要求的工具没有被真实调用，拒绝记录为成功：{}",
                    missing_tools.join(", ")
                ),
                tools,
            });
        }
        let unverified_tools = unverified_required_tools(&task.required_tools, &tools);
        if !unverified_tools.is_empty() {
            return Err(ExecutionFailure::ToolFailed {
                error: format!(
                    "必需工具缺少真实产物证明，拒绝记录为成功：{}",
                    unverified_tools.join(", ")
                ),
                tools,
            });
        }
        if response.trim().is_empty() && tools.is_empty() {
            return Err(ExecutionFailure::Failed(
                "Agent 执行结束但没有文本或工具结果，拒绝记录为成功".into(),
            ));
        }
        Ok(ExecutionOutput {
            response: truncate_chars(response.trim(), MAX_RESULT_CHARS),
            tools,
        })
    }
}

trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        system_now_ms()
    }
}

#[derive(Clone)]
struct RuntimeContext {
    db: Db,
    instance_id: String,
    emitter: Arc<dyn EventEmitter>,
    executor: Arc<dyn TaskExecutor>,
    clock: Arc<dyn Clock>,
    receipt_root: PathBuf,
    active: Arc<Mutex<HashMap<String, CancellationToken>>>,
    control: Arc<Mutex<()>>,
}

struct Inner {
    context: RuntimeContext,
    shutdown: CancellationToken,
    lease_owned: AtomicBool,
    lease_valid_until: AtomicI64,
    supervisor: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
        let _ = self.context.db.execute_json(
            "UPDATE agent_tasks SET owner_session_id=NULL, started_at=NULL \
             WHERE id=?1 AND task_type=?2 AND owner_session_id=?3",
            &[
                json!(CONFIG_ROW_ID),
                json!(CONFIG_TASK_TYPE),
                json!(self.context.instance_id),
            ],
        );
        if let Ok(mut handle) = self.supervisor.lock() {
            if let Some(handle) = handle.take() {
                handle.abort();
            }
        }
    }
}

/// 与 `AppState` 共享的 RedClaw 调度服务。
#[derive(Clone)]
pub struct RedClawScheduler {
    inner: Arc<Inner>,
}

impl RedClawScheduler {
    /// 启动生产调度服务并恢复 SQLite 中的任务定义。
    pub async fn start(
        db: Db,
        goose: GooseBridge,
        emitter: Arc<dyn EventEmitter>,
    ) -> anyhow::Result<Self> {
        let receipt_root = execution_receipt_root(&db);
        let executor: Arc<dyn TaskExecutor> = Arc::new(GooseTaskExecutor { goose });
        Self::start_with(
            db,
            emitter,
            executor,
            Arc::new(SystemClock),
            receipt_root,
            Duration::from_millis(500),
        )
        .await
    }

    /// 不启动监督循环的占位实例，仅供与 RedClaw 无关的单元测试构造 AppState。
    pub fn inactive(db: Db) -> Self {
        let instance_id = format!("inactive_{}", uuid::Uuid::new_v4());
        let context = RuntimeContext {
            receipt_root: execution_receipt_root(&db),
            db,
            instance_id,
            emitter: Arc::new(NoopEmitter),
            executor: Arc::new(GooseTaskExecutor {
                goose: GooseBridge::default(),
            }),
            clock: Arc::new(SystemClock),
            active: Arc::new(Mutex::new(HashMap::new())),
            control: Arc::new(Mutex::new(())),
        };
        Self {
            inner: Arc::new(Inner {
                context,
                shutdown: CancellationToken::new(),
                lease_owned: AtomicBool::new(false),
                lease_valid_until: AtomicI64::new(0),
                supervisor: std::sync::Mutex::new(None),
            }),
        }
    }

    async fn start_with(
        db: Db,
        emitter: Arc<dyn EventEmitter>,
        executor: Arc<dyn TaskExecutor>,
        clock: Arc<dyn Clock>,
        receipt_root: PathBuf,
        tick_interval: Duration,
    ) -> anyhow::Result<Self> {
        let now = clock.now_ms();
        ensure_config_row(db.clone(), now).await?;
        let instance_id = format!("scheduler_{}", uuid::Uuid::new_v4());
        let lease_valid_until = now + LEASE_TTL_MS;
        let lease_owned =
            acquire_or_renew_lease(db.clone(), instance_id.clone(), now, lease_valid_until).await?;
        if lease_owned {
            if let Err(error) = recover_interrupted_tasks(db.clone(), now).await {
                let _ = release_lease(db.clone(), instance_id.clone()).await;
                return Err(error);
            }
        }
        let context = RuntimeContext {
            db,
            instance_id,
            emitter,
            executor,
            clock,
            receipt_root,
            active: Arc::new(Mutex::new(HashMap::new())),
            control: Arc::new(Mutex::new(())),
        };
        let inner = Arc::new(Inner {
            context,
            shutdown: CancellationToken::new(),
            lease_owned: AtomicBool::new(lease_owned),
            lease_valid_until: AtomicI64::new(if lease_owned { lease_valid_until } else { 0 }),
            supervisor: std::sync::Mutex::new(None),
        });
        let weak = Arc::downgrade(&inner);
        let shutdown = inner.shutdown.clone();
        let handle = tokio::spawn(async move {
            let mut timer = tokio::time::interval(tick_interval);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = timer.tick() => {
                        let Some(inner) = weak.upgrade() else { break };
                        if let Err(error) = inner.process_due().await {
                            tracing::error!("RedClaw 调度扫描失败: {error}");
                        }
                    }
                }
            }
        });
        *inner
            .supervisor
            .lock()
            .map_err(|_| anyhow::anyhow!("RedClaw supervisor mutex poisoned"))? = Some(handle);
        Ok(Self { inner })
    }

    pub async fn status(&self) -> anyhow::Result<Value> {
        let config = load_config(self.inner.context.db.clone()).await?;
        let lease = load_lease(self.inner.context.db.clone()).await?;
        let now = self.inner.context.clock.now_ms();
        let owns_lease = lease.as_ref().is_some_and(|(owner, expires_at)| {
            owner == &self.inner.context.instance_id && *expires_at > now
        });
        let scheduled = load_scheduled(self.inner.context.db.clone()).await?;
        let long_cycle = load_long_cycle(self.inner.context.db.clone()).await?;
        let scheduled_states =
            load_runtime_states(self.inner.context.db.clone(), SCHEDULED_TASK_TYPE).await?;
        let long_states =
            load_runtime_states(self.inner.context.db.clone(), LONG_CYCLE_TASK_TYPE).await?;
        let active = self.inner.context.active.lock().await;
        let active_ids: HashSet<&str> = active.keys().map(String::as_str).collect();
        let scheduled_map: Map<String, Value> = scheduled
            .iter()
            .map(|task| {
                (
                    task.id.clone(),
                    task_json_with_runtime(scheduled_json(task), scheduled_states.get(&task.id)),
                )
            })
            .collect();
        let long_map: Map<String, Value> = long_cycle
            .iter()
            .map(|task| {
                (
                    task.id.clone(),
                    task_json_with_runtime(long_cycle_json(task), long_states.get(&task.id)),
                )
            })
            .collect();
        let in_flight_task_ids: Vec<&str> = scheduled
            .iter()
            .filter(|task| active_ids.contains(task.id.as_str()))
            .map(|task| task.id.as_str())
            .collect();
        let in_flight_long_cycle_task_ids: Vec<&str> = long_cycle
            .iter()
            .filter(|task| active_ids.contains(task.id.as_str()))
            .map(|task| task.id.as_str())
            .collect();
        let next_fire = scheduled
            .iter()
            .filter(|task| {
                task.enabled
                    && scheduled_states
                        .get(&task.id)
                        .is_some_and(|state| state.status == "scheduled")
            })
            .filter_map(|task| task.next_run_at)
            .chain(
                long_cycle
                    .iter()
                    .filter(|task| {
                        task.enabled
                            && long_states
                                .get(&task.id)
                                .is_some_and(|state| state.status == "scheduled")
                    })
                    .filter_map(|task| task.next_run_at),
            )
            .min();
        Ok(json!({
            "enabled": config.enabled,
            "lockState": if owns_lease { "owner" } else { "passive" },
            "blockedBy": lease.as_ref()
                .filter(|(owner, expires_at)| owner != &self.inner.context.instance_id && *expires_at > now)
                .map(|(owner, _)| owner.clone()),
            "leaseExpiresAt": lease.as_ref()
                .map(|(_, expires_at)| timestamp_value(Some(*expires_at)))
                .unwrap_or(Value::Null),
            "intervalMinutes": config.interval_minutes,
            "keepAliveWhenNoWindow": config.keep_alive_when_no_window,
            "maxProjectsPerTick": config.max_projects_per_tick,
            "maxAutomationPerTick": config.max_automation_per_tick,
            "isTicking": !active.is_empty(),
            "currentProjectId": Value::Null,
            "currentAutomationTaskId": active.keys().next(),
            "nextAutomationFireAt": timestamp_value(next_fire),
            "inFlightTaskIds": in_flight_task_ids,
            "inFlightLongCycleTaskIds": in_flight_long_cycle_task_ids,
            "heartbeatInFlight": false,
            "lastTickAt": Value::Null,
            "nextTickAt": timestamp_value(next_fire),
            "lastError": Value::Null,
            "nextMaintenanceAt": Value::Null,
            "projectStates": config.project_states.values().map(|item| {
                (item.project_id.clone(), project_state_json(item))
            }).collect::<Map<String, Value>>(),
            "heartbeat": heartbeat_json(&config.heartbeat),
            "scheduledTasks": Value::Object(scheduled_map),
            "longCycleTasks": Value::Object(long_map),
            "capabilities": {
                "scheduledDefinitions": true,
                "longCycleDefinitions": true,
                "automaticExecution": true,
                "persistentDefinitions": true,
                // 定义与错过的触发时间会在下次启动恢复，但进程退出期间不会后台执行。
                "survivesAppExit": false,
                "cancellableExecution": true,
                "executionReceipts": true,
                "heartbeatExecution": false,
                "projectTickExecution": false,
                "statusText": "任务由 Rust/Tokio 持久化调度，到点后使用独立 Goose 会话真实执行；结果写入 SQLite 轨迹和 JSON 回执"
            },
        }))
    }

    pub async fn start_runner(&self) -> anyhow::Result<Value> {
        let guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能启动任务");
        }
        let mut config = load_config(self.inner.context.db.clone()).await?;
        if config.enabled {
            drop(guard);
            return self.emit_status().await;
        }
        if !self.inner.context.active.lock().await.is_empty() {
            anyhow::bail!("仍有任务正在停止，请等待执行状态结束后再启动调度器");
        }
        config.enabled = true;
        let now = self.inner.context.clock.now_ms();
        save_config(self.inner.context.db.clone(), config, now).await?;
        resume_enabled_definitions(self.inner.context.db.clone(), now).await?;
        drop(guard);
        self.emit_status().await
    }

    pub async fn stop_runner(&self) -> anyhow::Result<Value> {
        let guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能停止任务");
        }
        let mut config = load_config(self.inner.context.db.clone()).await?;
        config.enabled = false;
        save_config(
            self.inner.context.db.clone(),
            config,
            self.inner.context.clock.now_ms(),
        )
        .await?;
        self.cancel_all().await;
        drop(guard);
        let drained = self.wait_for_cancellation().await;
        if !drained {
            self.emit_status_event().await;
            anyhow::bail!("任务仍在停止中；调度器已暂停，但不会在执行清空前重新启动");
        }
        self.emit_status().await
    }

    pub async fn set_config(&self, payload: &Value) -> anyhow::Result<Value> {
        if payload
            .get("keepAliveWhenNoWindow")
            .and_then(Value::as_bool)
            == Some(true)
        {
            anyhow::bail!("应用进程退出后无法继续调度；请保持桌面应用运行");
        }
        if payload.get("heartbeatEnabled").and_then(Value::as_bool) == Some(true) {
            anyhow::bail!("心跳巡检执行器尚未接入；未保存 enabled=true，避免显示假运行状态");
        }
        let requested_enabled = payload.get("enabled").and_then(Value::as_bool);
        let guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能修改运行设置");
        }
        let mut config = load_config(self.inner.context.db.clone()).await?;
        let previous_enabled = config.enabled;
        if let Some(value) = payload.get("intervalMinutes").and_then(Value::as_i64) {
            config.interval_minutes = value.clamp(1, 180);
        }
        if let Some(value) = payload
            .get("keepAliveWhenNoWindow")
            .and_then(Value::as_bool)
        {
            config.keep_alive_when_no_window = value;
        }
        if let Some(value) = payload.get("maxProjectsPerTick").and_then(Value::as_i64) {
            config.max_projects_per_tick = value.clamp(1, 10);
        }
        if let Some(value) = payload.get("maxAutomationPerTick").and_then(Value::as_i64) {
            config.max_automation_per_tick = value.clamp(1, 10);
        }
        if let Some(value) = payload.get("heartbeatEnabled").and_then(Value::as_bool) {
            config.heartbeat.enabled = value;
        }
        if let Some(value) = payload
            .get("heartbeatIntervalMinutes")
            .and_then(Value::as_i64)
        {
            config.heartbeat.interval_minutes = value.clamp(5, 360);
        }
        if let Some(value) = payload
            .get("heartbeatSuppressEmptyReport")
            .and_then(Value::as_bool)
        {
            config.heartbeat.suppress_empty_report = value;
        }
        if let Some(value) = payload
            .get("heartbeatReportToMainSession")
            .and_then(Value::as_bool)
        {
            config.heartbeat.report_to_main_session = value;
        }
        if let Some(value) = payload.get("heartbeatPrompt").and_then(Value::as_str) {
            config.heartbeat.prompt = non_empty(value);
        }
        if let Some(value) = requested_enabled {
            if value && !previous_enabled && !self.inner.context.active.lock().await.is_empty() {
                anyhow::bail!("仍有任务正在停止，请等待执行状态结束后再启动调度器");
            }
            config.enabled = value;
        }
        save_config(
            self.inner.context.db.clone(),
            config,
            self.inner.context.clock.now_ms(),
        )
        .await?;
        if requested_enabled == Some(true) && !previous_enabled {
            resume_enabled_definitions(
                self.inner.context.db.clone(),
                self.inner.context.clock.now_ms(),
            )
            .await?;
        }
        if requested_enabled == Some(false) {
            // The disabled state and cancellation signal share the control critical section,
            // so the supervisor cannot claim a new task between them.
            self.cancel_all().await;
        }
        drop(guard);
        if requested_enabled == Some(false) && !self.wait_for_cancellation().await {
            self.emit_status_event().await;
            anyhow::bail!("任务仍在停止中；调度器已暂停，但不会在执行清空前重新启动");
        }
        self.emit_status().await
    }

    pub async fn set_project(&self, payload: &Value) -> anyhow::Result<Value> {
        let project_id = required_str(payload, "projectId")?;
        if payload.get("enabled").and_then(Value::as_bool) == Some(true) {
            anyhow::bail!("项目巡检执行器尚未接入；未保存 enabled=true，避免显示假运行状态");
        }
        let guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能修改项目设置");
        }
        let mut config = load_config(self.inner.context.db.clone()).await?;
        let previous = config.project_states.remove(&project_id);
        let enabled = payload
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let prompt = payload
            .get("prompt")
            .and_then(Value::as_str)
            .and_then(non_empty)
            .or_else(|| previous.as_ref().and_then(|item| item.prompt.clone()));
        config.project_states.insert(
            project_id.clone(),
            ProjectState {
                project_id,
                enabled,
                prompt,
                last_run_at: previous.as_ref().and_then(|item| item.last_run_at),
                last_result: previous.as_ref().and_then(|item| item.last_result.clone()),
                last_error: previous.as_ref().and_then(|item| item.last_error.clone()),
            },
        );
        save_config(
            self.inner.context.db.clone(),
            config,
            self.inner.context.clock.now_ms(),
        )
        .await?;
        drop(guard);
        self.emit_status().await
    }

    pub async fn add_scheduled(&self, payload: &Value) -> anyhow::Result<Value> {
        let mode = payload
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("interval")
            .trim()
            .to_lowercase();
        if !["interval", "daily", "weekly", "once"].contains(&mode.as_str()) {
            anyhow::bail!("mode must be one of interval/daily/weekly/once");
        }
        let prompt = required_str(payload, "prompt")?;
        validate_task_instruction(&prompt, "prompt")?;
        let (required_tools, allowed_tools, authorized_tools) = task_tool_policy(payload)?;
        let now = self.inner.context.clock.now_ms();
        let interval_minutes = (mode == "interval").then(|| {
            payload
                .get("intervalMinutes")
                .and_then(Value::as_i64)
                .unwrap_or(60)
                .clamp(1, MAX_TASK_INTERVAL_MINUTES)
        });
        let time = if mode == "daily" || mode == "weekly" {
            Some(sanitize_hhmm(payload.get("time").and_then(Value::as_str))?)
        } else {
            None
        };
        let weekdays = if mode == "weekly" {
            sanitize_weekdays(payload.get("weekdays"))
        } else {
            Vec::new()
        };
        let run_at = if mode == "once" {
            let value = payload
                .get("runAt")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("runAt is required for once task"))?;
            let parsed = parse_time_ms(value)
                .ok_or_else(|| anyhow::anyhow!("runAt must be an ISO-8601 datetime"))?;
            if parsed <= now {
                anyhow::bail!("runAt must be in the future");
            }
            Some(parsed)
        } else {
            None
        };
        let mut task = ScheduledTask {
            id: format!("sched_{}", uuid::Uuid::new_v4()),
            name: optional_str(payload, "name").unwrap_or_else(|| "定时任务".into()),
            enabled: payload
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            mode,
            prompt,
            project_id: optional_str(payload, "projectId"),
            work_item_id: optional_str(payload, "workItemId"),
            subagent_roles: string_array(payload.get("subagentRoles")),
            required_tools,
            allowed_tools,
            authorized_tools,
            interval_minutes,
            time,
            weekdays,
            run_at,
            created_at: now,
            updated_at: now,
            last_run_at: None,
            last_result: None,
            last_output: None,
            last_error: None,
            next_run_at: None,
            last_receipt_path: None,
        };
        task.next_run_at = task
            .enabled
            .then(|| compute_next_scheduled(&task, now))
            .flatten();
        insert_definition(
            self.inner.context.db.clone(),
            task.id.clone(),
            SCHEDULED_TASK_TYPE,
            if task.enabled { "scheduled" } else { "paused" },
            serde_json::to_value(&task)?,
            now,
        )
        .await?;
        self.inner.context.emitter.emit(
            "data:changed",
            json!({"scope":"redclaw-scheduled","action":"create","entityId":task.id}),
        );
        self.emit_status_event().await;
        Ok(json!({"success":true,"task":scheduled_json(&task)}))
    }

    pub async fn add_long_cycle(&self, payload: &Value) -> anyhow::Result<Value> {
        let objective = required_str(payload, "objective")?;
        let step_prompt = required_str(payload, "stepPrompt")?;
        validate_task_instruction(&objective, "objective")?;
        validate_task_instruction(&step_prompt, "stepPrompt")?;
        let (required_tools, allowed_tools, authorized_tools) = task_tool_policy(payload)?;
        let interval_minutes = payload
            .get("intervalMinutes")
            .and_then(Value::as_i64)
            .unwrap_or(60)
            .clamp(1, MAX_TASK_INTERVAL_MINUTES);
        let total_rounds = payload
            .get("totalRounds")
            .and_then(Value::as_i64)
            .unwrap_or(8)
            .clamp(1, 200);
        let now = self.inner.context.clock.now_ms();
        let enabled = payload
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let task = LongCycleTask {
            id: format!("long_{}", uuid::Uuid::new_v4()),
            name: optional_str(payload, "name").unwrap_or_else(|| "长周期任务".into()),
            enabled,
            status: if enabled { "running" } else { "paused" }.into(),
            objective,
            step_prompt,
            project_id: optional_str(payload, "projectId"),
            work_item_id: optional_str(payload, "workItemId"),
            subagent_roles: string_array(payload.get("subagentRoles")),
            required_tools,
            allowed_tools,
            authorized_tools,
            interval_minutes,
            total_rounds,
            completed_rounds: 0,
            created_at: now,
            updated_at: now,
            last_run_at: None,
            last_result: None,
            last_output: None,
            last_error: None,
            // 第一轮在一个完整间隔后触发，避免“新建即执行”与 UI 的下一次时间冲突。
            next_run_at: enabled.then_some(now + interval_minutes * 60_000),
            last_receipt_path: None,
        };
        insert_definition(
            self.inner.context.db.clone(),
            task.id.clone(),
            LONG_CYCLE_TASK_TYPE,
            if enabled { "scheduled" } else { "paused" },
            serde_json::to_value(&task)?,
            now,
        )
        .await?;
        self.inner.context.emitter.emit(
            "data:changed",
            json!({"scope":"redclaw-long-cycle","action":"create","entityId":task.id}),
        );
        self.emit_status_event().await;
        Ok(json!({"success":true,"task":long_cycle_json(&task)}))
    }

    pub async fn list_scheduled(&self) -> anyhow::Result<Value> {
        let tasks = load_scheduled(self.inner.context.db.clone()).await?;
        let states =
            load_runtime_states(self.inner.context.db.clone(), SCHEDULED_TASK_TYPE).await?;
        Ok(json!({
            "success": true,
            "tasks": tasks.iter().map(|task| {
                task_json_with_runtime(scheduled_json(task), states.get(&task.id))
            }).collect::<Vec<_>>()
        }))
    }

    pub async fn list_long_cycle(&self) -> anyhow::Result<Value> {
        let tasks = load_long_cycle(self.inner.context.db.clone()).await?;
        let states =
            load_runtime_states(self.inner.context.db.clone(), LONG_CYCLE_TASK_TYPE).await?;
        Ok(json!({
            "success": true,
            "tasks": tasks.iter().map(|task| {
                task_json_with_runtime(long_cycle_json(task), states.get(&task.id))
            }).collect::<Vec<_>>()
        }))
    }

    pub async fn set_scheduled_enabled(&self, payload: &Value) -> anyhow::Result<Value> {
        let id = required_str(payload, "taskId")?;
        let enabled = payload
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能修改任务");
        }
        let mut task = find_scheduled(self.inner.context.db.clone(), id.clone()).await?;
        let active = self.inner.context.active.lock().await.contains_key(&id);
        if enabled && active {
            if task.enabled {
                drop(guard);
                return Ok(json!({"success":true,"task":scheduled_json(&task)}));
            }
            anyhow::bail!("任务仍在停止中，请稍后再启用");
        }
        let now = self.inner.context.clock.now_ms();
        if enabled && task.mode == "once" && task.run_at.is_none_or(|run_at| run_at <= now) {
            anyhow::bail!("一次性任务时间已经过去，请新建任务并选择未来时间");
        }
        task.enabled = enabled;
        task.updated_at = now;
        task.next_run_at = enabled
            .then(|| compute_next_scheduled(&task, task.updated_at))
            .flatten();
        if !enabled && active {
            if !update_metadata_while_running(
                self.inner.context.db.clone(),
                id.clone(),
                TaskKind::Scheduled,
                serde_json::to_value(&task)?,
                now,
            )
            .await?
            {
                anyhow::bail!("任务执行状态已经变化，请刷新后重试");
            }
            self.cancel_task(&id).await;
            drop(guard);
            if !self.wait_for_task_cancellation(&id).await {
                anyhow::bail!("任务仍在停止中；已禁止后续调度，请稍后刷新状态");
            }
            let guard = self.inner.context.control.lock().await;
            let mut latest = find_scheduled(self.inner.context.db.clone(), id.clone()).await?;
            latest.enabled = false;
            latest.next_run_at = None;
            latest.updated_at = self.inner.context.clock.now_ms();
            save_definition(
                self.inner.context.db.clone(),
                id,
                "paused",
                serde_json::to_value(&latest)?,
                latest.last_error.clone(),
                latest.updated_at,
                latest.last_receipt_path.clone(),
            )
            .await?;
            drop(guard);
            self.emit_status_event().await;
            return Ok(json!({"success":true,"task":scheduled_json(&latest)}));
        }
        save_definition(
            self.inner.context.db.clone(),
            id,
            if enabled { "scheduled" } else { "paused" },
            serde_json::to_value(&task)?,
            task.last_error.clone(),
            task.updated_at,
            task.last_receipt_path.clone(),
        )
        .await?;
        drop(guard);
        self.emit_status_event().await;
        Ok(json!({"success":true,"task":scheduled_json(&task)}))
    }

    pub async fn set_long_cycle_enabled(&self, payload: &Value) -> anyhow::Result<Value> {
        let id = required_str(payload, "taskId")?;
        let enabled = payload
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能修改任务");
        }
        let mut task = find_long_cycle(self.inner.context.db.clone(), id.clone()).await?;
        if task.completed_rounds >= task.total_rounds && enabled {
            anyhow::bail!("Long cycle task is already completed");
        }
        let active = self.inner.context.active.lock().await.contains_key(&id);
        if enabled && active {
            if task.enabled {
                drop(guard);
                return Ok(json!({"success":true,"task":long_cycle_json(&task)}));
            }
            anyhow::bail!("任务仍在停止中，请稍后再启用");
        }
        task.enabled = enabled;
        task.status = if enabled { "running" } else { "paused" }.into();
        task.updated_at = self.inner.context.clock.now_ms();
        task.next_run_at = enabled.then_some(task.updated_at + task.interval_minutes * 60_000);
        if !enabled && active {
            if !update_metadata_while_running(
                self.inner.context.db.clone(),
                id.clone(),
                TaskKind::LongCycle,
                serde_json::to_value(&task)?,
                task.updated_at,
            )
            .await?
            {
                anyhow::bail!("任务执行状态已经变化，请刷新后重试");
            }
            self.cancel_task(&id).await;
            drop(guard);
            if !self.wait_for_task_cancellation(&id).await {
                anyhow::bail!("任务仍在停止中；已禁止后续调度，请稍后刷新状态");
            }
            let guard = self.inner.context.control.lock().await;
            let mut latest = find_long_cycle(self.inner.context.db.clone(), id.clone()).await?;
            latest.enabled = false;
            latest.status = "paused".into();
            latest.next_run_at = None;
            latest.updated_at = self.inner.context.clock.now_ms();
            save_definition(
                self.inner.context.db.clone(),
                id,
                "paused",
                serde_json::to_value(&latest)?,
                latest.last_error.clone(),
                latest.updated_at,
                latest.last_receipt_path.clone(),
            )
            .await?;
            drop(guard);
            self.emit_status_event().await;
            return Ok(json!({"success":true,"task":long_cycle_json(&latest)}));
        }
        save_definition(
            self.inner.context.db.clone(),
            id,
            if enabled { "scheduled" } else { "paused" },
            serde_json::to_value(&task)?,
            task.last_error.clone(),
            task.updated_at,
            task.last_receipt_path.clone(),
        )
        .await?;
        drop(guard);
        self.emit_status_event().await;
        Ok(json!({"success":true,"task":long_cycle_json(&task)}))
    }

    pub async fn remove_scheduled(&self, payload: &Value) -> anyhow::Result<Value> {
        self.remove_definition(payload, TaskKind::Scheduled).await
    }

    pub async fn remove_long_cycle(&self, payload: &Value) -> anyhow::Result<Value> {
        self.remove_definition(payload, TaskKind::LongCycle).await
    }

    async fn remove_definition(&self, payload: &Value, kind: TaskKind) -> anyhow::Result<Value> {
        let id = required_str(payload, "taskId")?;
        match kind {
            TaskKind::Scheduled => {
                self.set_scheduled_enabled(&json!({"taskId":id.clone(),"enabled":false}))
                    .await?;
            }
            TaskKind::LongCycle => {
                self.set_long_cycle_enabled(&json!({"taskId":id.clone(),"enabled":false}))
                    .await?;
            }
        }
        let guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能删除任务");
        }
        let db = self.inner.context.db.clone();
        let id_for_db = id.clone();
        let task_type = kind.task_type().to_string();
        let removed = blocking(move || {
            db.execute_json(
                "DELETE FROM agent_tasks WHERE id = ?1 AND task_type = ?2",
                &[json!(id_for_db), json!(task_type)],
            )
        })
        .await?;
        if removed == 0 {
            anyhow::bail!("Task not found");
        }
        drop(guard);
        self.inner.context.emitter.emit(
            "data:changed",
            json!({"scope":kind.receipt_label(),"action":"remove","entityId":id}),
        );
        let status = self.emit_status().await?;
        Ok(json!({"success":true,"status":status}))
    }

    pub async fn run_scheduled_now(&self, payload: &Value) -> anyhow::Result<Value> {
        let id = required_str(payload, "taskId")?;
        self.trigger_now(id, TaskKind::Scheduled).await?;
        Ok(json!({"success":true,"status":self.status().await?}))
    }

    pub async fn run_long_cycle_now(&self, payload: &Value) -> anyhow::Result<Value> {
        let id = required_str(payload, "taskId")?;
        self.trigger_now(id, TaskKind::LongCycle).await?;
        Ok(json!({"success":true,"status":self.status().await?}))
    }

    pub async fn run_due_now(&self) -> anyhow::Result<Value> {
        let triggered = self.inner.process_due().await?;
        let mut status = self.status().await?;
        if let Some(map) = status.as_object_mut() {
            map.insert("triggeredTaskIds".into(), json!(triggered));
        }
        Ok(status)
    }

    async fn trigger_now(&self, id: String, kind: TaskKind) -> anyhow::Result<()> {
        let _guard = self.inner.context.control.lock().await;
        if !self.inner.ensure_lease().await? {
            anyhow::bail!("另一个桌面实例持有调度租约，当前实例不能执行任务");
        }
        let config = load_config(self.inner.context.db.clone()).await?;
        if !config.enabled {
            anyhow::bail!("自动调度器已暂停；请先启动调度器再执行任务");
        }
        let active = self.inner.context.active.lock().await;
        if active.contains_key(&id) {
            anyhow::bail!("Task is already running");
        }
        if active.len() >= config.max_automation_per_tick.max(1) as usize {
            anyhow::bail!("已达到自动任务并发上限，请等待在途任务完成");
        }
        drop(active);
        match kind {
            TaskKind::Scheduled => {
                let task = find_scheduled(self.inner.context.db.clone(), id.clone()).await?;
                if !task.enabled {
                    anyhow::bail!("Task is disabled");
                }
            }
            TaskKind::LongCycle => {
                let task = find_long_cycle(self.inner.context.db.clone(), id.clone()).await?;
                if !task.enabled {
                    anyhow::bail!("Task is disabled");
                }
                if task.completed_rounds >= task.total_rounds {
                    anyhow::bail!("Long cycle task is already completed");
                }
            }
        }
        let now = self.inner.context.clock.now_ms();
        let claimed = claim_task(
            self.inner.context.db.clone(),
            id,
            kind,
            now,
            true,
            &self.inner.context.instance_id,
        )
        .await?;
        let Some(task) = claimed else {
            anyhow::bail!("Task is already running");
        };
        self.inner.spawn_execution(task).await;
        Ok(())
    }

    async fn cancel_task(&self, id: &str) {
        let token = self.inner.context.active.lock().await.get(id).cloned();
        if let Some(token) = token {
            token.cancel();
        }
    }

    async fn cancel_all(&self) {
        let tokens: Vec<CancellationToken> = self
            .inner
            .context
            .active
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        for token in tokens {
            token.cancel();
        }
    }

    async fn wait_for_cancellation(&self) -> bool {
        let deadline = tokio::time::Instant::now() + CANCELLATION_DRAIN_TIMEOUT;
        loop {
            if self.inner.context.active.lock().await.is_empty() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_for_task_cancellation(&self, id: &str) -> bool {
        let deadline = tokio::time::Instant::now() + CANCELLATION_DRAIN_TIMEOUT;
        loop {
            if !self.inner.context.active.lock().await.contains_key(id) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn emit_status_event(&self) {
        if let Ok(status) = self.status().await {
            self.inner
                .context
                .emitter
                .emit("redclaw:runner-status", status);
        }
    }

    async fn emit_status(&self) -> anyhow::Result<Value> {
        let status = self.status().await?;
        self.inner
            .context
            .emitter
            .emit("redclaw:runner-status", status.clone());
        Ok(status)
    }
}

impl Inner {
    async fn ensure_lease(&self) -> anyhow::Result<bool> {
        let now = self.context.clock.now_ms();
        if self.lease_owned.load(Ordering::SeqCst)
            && self.lease_valid_until.load(Ordering::SeqCst) - LEASE_RENEW_MARGIN_MS > now
        {
            return Ok(true);
        }
        let previous_lease = load_lease(self.context.db.clone()).await?;
        let lease_was_ours = previous_lease
            .as_ref()
            .is_some_and(|(owner, _)| owner == &self.context.instance_id);
        let valid_until = now + LEASE_TTL_MS;
        let acquired = acquire_or_renew_lease(
            self.context.db.clone(),
            self.context.instance_id.clone(),
            now,
            valid_until,
        )
        .await?;
        let previously_owned = self.lease_owned.swap(acquired, Ordering::SeqCst);
        self.lease_valid_until
            .store(if acquired { valid_until } else { 0 }, Ordering::SeqCst);
        if acquired && !previously_owned && !lease_was_ours {
            recover_interrupted_tasks(self.context.db.clone(), now).await?;
        } else if !acquired && previously_owned {
            let tokens: Vec<CancellationToken> =
                self.context.active.lock().await.values().cloned().collect();
            for token in tokens {
                token.cancel();
            }
        }
        Ok(acquired)
    }

    async fn process_due(&self) -> anyhow::Result<Vec<String>> {
        let _guard = self.context.control.lock().await;
        if !self.ensure_lease().await? {
            return Ok(Vec::new());
        }
        let active_ids: HashSet<String> =
            self.context.active.lock().await.keys().cloned().collect();
        recover_interrupted_tasks_excluding(
            self.context.db.clone(),
            self.context.clock.now_ms(),
            &active_ids,
        )
        .await?;
        let config = load_config(self.context.db.clone()).await?;
        if !config.enabled {
            return Ok(Vec::new());
        }
        let capacity =
            (config.max_automation_per_tick.max(1) as usize).saturating_sub(active_ids.len());
        if capacity == 0 {
            return Ok(Vec::new());
        }
        let now = self.context.clock.now_ms();
        let mut due = load_due(self.context.db.clone(), now).await?;
        due.truncate(capacity);
        let mut triggered = Vec::new();
        for task in due {
            if let Some(claimed) = claim_task(
                self.context.db.clone(),
                task.id.clone(),
                task.kind,
                now,
                false,
                &self.context.instance_id,
            )
            .await?
            {
                triggered.push(task.id);
                self.spawn_execution(claimed).await;
            }
        }
        Ok(triggered)
    }

    async fn spawn_execution(&self, task: ClaimedTask) {
        let cancel = self.shutdown.child_token();
        self.context
            .active
            .lock()
            .await
            .insert(task.id.clone(), cancel.clone());
        let context = self.context.clone();
        context.emitter.emit(
            "redclaw:task-event",
            json!({
                "taskId":task.id,
                "executionId":task.execution_id,
                "status":"running",
                "eventType":"started",
                "startedAt":task.started_at,
            }),
        );
        tokio::spawn(async move {
            let result = context.executor.execute(&task, cancel).await;
            if let Err(error) = finish_execution(context.clone(), task.clone(), result).await {
                tracing::error!(
                    "RedClaw task {} completion persistence failed: {error}",
                    task.id
                );
            }
            context.active.lock().await.remove(&task.id);
        });
    }
}

async fn finish_execution(
    context: RuntimeContext,
    task: ClaimedTask,
    result: Result<ExecutionOutput, ExecutionFailure>,
) -> anyhow::Result<()> {
    let completed_at = context.clock.now_ms();
    let (mut status, mut response, tools, mut error_text) = match result {
        Ok(output) => ("succeeded".to_string(), output.response, output.tools, None),
        Err(ExecutionFailure::Cancelled) => (
            "cancelled".to_string(),
            String::new(),
            Vec::new(),
            Some("任务已取消；未记录为成功".to_string()),
        ),
        Err(ExecutionFailure::Failed(error)) => {
            ("failed".to_string(), String::new(), Vec::new(), Some(error))
        }
        Err(ExecutionFailure::ToolFailed { error, tools }) => {
            ("failed".to_string(), String::new(), tools, Some(error))
        }
    };
    let mut receipt = sanitize_receipt_value(&json!({
        "schemaVersion": 1,
        "executionId": task.execution_id,
        "taskId": task.id,
        "taskKind": task.kind.receipt_label(),
        "status": status,
        "startedAt": task.started_at,
        "completedAt": completed_at,
        "durationMs": completed_at.saturating_sub(task.started_at),
        "prompt": task.prompt,
        "requiredTools": task.required_tools,
        "allowedTools": task.allowed_tools,
        "authorizedTools": task.authorized_tools,
        "response": response,
        "tools": tools,
        "error": error_text,
    }));
    let receipt_path = match write_receipt(
        &context.receipt_root,
        &task.id,
        &task.execution_id,
        &receipt,
    )
    .await
    {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(error) => {
            status = "failed".into();
            response.clear();
            error_text = Some(format!("执行回执持久化失败，任务未记录为成功：{error}"));
            if let Some(map) = receipt.as_object_mut() {
                map.insert("status".into(), json!(status));
                map.insert("response".into(), json!(""));
                map.insert("error".into(), json!(error_text));
            }
            None
        }
    };
    // 执行阶段不持锁；只有最终的 metadata 读改写与暂停/恢复操作串行，
    // 避免旧轮次覆盖用户刚保存的 enabled/轮次状态。
    let _control = context.control.lock().await;
    complete_task(
        context.db.clone(),
        task.clone(),
        &context.instance_id,
        &status,
        response.clone(),
        error_text.clone(),
        receipt_path.clone(),
        completed_at,
    )
    .await?;
    context.emitter.emit(
        "redclaw:task-event",
        json!({
            "taskId":task.id,
            "executionId":task.execution_id,
            "status":status,
            "eventType":if status == "succeeded" { "completed" } else { "failed" },
            "result":response,
            "error":error_text,
            "receiptPath":receipt_path,
            "completedAt":completed_at,
        }),
    );
    context.emitter.emit(
        "data:changed",
        json!({"scope":task.kind.receipt_label(),"action":"execute","entityId":task.id}),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn complete_task(
    db: Db,
    claimed: ClaimedTask,
    instance_id: &str,
    execution_status: &str,
    response: String,
    error_text: Option<String>,
    receipt_path: Option<String>,
    completed_at: i64,
) -> anyhow::Result<()> {
    match claimed.kind {
        TaskKind::Scheduled => {
            let mut task = find_scheduled(db.clone(), claimed.id.clone()).await?;
            task.last_run_at = Some(completed_at);
            task.last_result = Some(
                match execution_status {
                    "succeeded" => "success",
                    "cancelled" => "skipped",
                    _ => "error",
                }
                .into(),
            );
            task.last_output = (!response.is_empty()).then_some(response);
            task.last_error = error_text.clone();
            task.last_receipt_path = receipt_path.clone();
            task.updated_at = completed_at;
            let row_status = if execution_status == "succeeded" {
                if task.mode == "once" {
                    task.enabled = false;
                    task.next_run_at = None;
                    "completed"
                } else if task.enabled {
                    task.next_run_at = compute_next_scheduled(&task, completed_at);
                    "scheduled"
                } else {
                    task.next_run_at = None;
                    "paused"
                }
            } else if execution_status == "cancelled" && task.mode == "once" {
                // 一次性任务可能已经产生外部副作用，取消后不自动重放。
                task.enabled = false;
                task.next_run_at = None;
                "failed"
            } else if execution_status == "cancelled" || !task.enabled {
                task.next_run_at = None;
                "paused"
            } else if task.mode == "once" {
                task.enabled = false;
                task.next_run_at = None;
                "failed"
            } else {
                // 未知的部分副作用不能靠自动重试赌幂等；失败后暂停，由用户复核后恢复。
                task.enabled = false;
                task.next_run_at = None;
                "failed"
            };
            save_claimed_definition(
                db,
                claimed.id,
                claimed.execution_id,
                instance_id.to_string(),
                execution_status,
                row_status,
                serde_json::to_value(&task)?,
                error_text,
                completed_at,
                receipt_path,
            )
            .await
        }
        TaskKind::LongCycle => {
            let mut task = find_long_cycle(db.clone(), claimed.id.clone()).await?;
            task.last_run_at = Some(completed_at);
            task.last_result = Some(
                match execution_status {
                    "succeeded" => "success",
                    "cancelled" => "skipped",
                    _ => "error",
                }
                .into(),
            );
            task.last_output = (!response.is_empty()).then_some(response);
            task.last_error = error_text.clone();
            task.last_receipt_path = receipt_path.clone();
            task.updated_at = completed_at;
            if execution_status == "succeeded" {
                task.completed_rounds += 1;
            }
            let row_status = if task.completed_rounds >= task.total_rounds {
                task.enabled = false;
                task.status = "completed".into();
                task.next_run_at = None;
                "completed"
            } else if execution_status == "cancelled" || !task.enabled {
                task.status = "paused".into();
                task.next_run_at = None;
                "paused"
            } else if execution_status != "succeeded" {
                // 长周期步骤可能已经产生部分外部副作用；失败后不无限重试同一轮。
                task.enabled = false;
                task.status = "paused".into();
                task.next_run_at = None;
                "failed"
            } else {
                task.status = "running".into();
                task.next_run_at = Some(completed_at + task.interval_minutes * 60_000);
                "scheduled"
            };
            save_claimed_definition(
                db,
                claimed.id,
                claimed.execution_id,
                instance_id.to_string(),
                execution_status,
                row_status,
                serde_json::to_value(&task)?,
                error_text,
                completed_at,
                receipt_path,
            )
            .await
        }
    }
}

async fn claim_task(
    db: Db,
    id: String,
    kind: TaskKind,
    now: i64,
    force: bool,
    instance_id: &str,
) -> anyhow::Result<Option<ClaimedTask>> {
    let execution_id = format!("exec_{}", uuid::Uuid::new_v4());
    let (base_prompt, required_tools, allowed_tools, authorized_tools, project_id, work_item_id) =
        match kind {
            TaskKind::Scheduled => {
                let task = find_scheduled(db.clone(), id.clone()).await?;
                (
                    task.prompt,
                    task.required_tools,
                    task.allowed_tools,
                    task.authorized_tools,
                    task.project_id,
                    task.work_item_id,
                )
            }
            TaskKind::LongCycle => {
                let task = find_long_cycle(db.clone(), id.clone()).await?;
                let previous = task
                    .last_output
                    .as_deref()
                    .map(|output| truncate_chars(output, 8_000))
                    .unwrap_or_else(|| "（首轮，无上一轮结果）".into());
                (
                format!(
                    "长周期目标：{}\n当前轮次：{}/{}\n上一轮结果：{}\n本轮执行指令：{}\n\n必须真实完成本轮工作；若调用工具，请等待工具返回后再总结产物。",
                    task.objective,
                    task.completed_rounds + 1,
                    task.total_rounds,
                    previous,
                    task.step_prompt
                ),
                task.required_tools,
                task.allowed_tools,
                task.authorized_tools,
                task.project_id,
                task.work_item_id,
            )
            }
        };
    let prompt = format!(
        "调度执行 ID（也是工具支持时必须传入的 idempotency key）：{execution_id}\n项目 ID：{}\n工作项 ID：{}\n本次只允许调用：{}\n其中必须调用：{}\n已显式授权的付费/发布工具：{}\n禁止调用授权列表外的任何工具；每次工具调用必须等待真实结果，不得用文字冒充产物。\n\n不可变任务指令：\n{base_prompt}",
        project_id.as_deref().unwrap_or("（无）"),
        work_item_id.as_deref().unwrap_or("（无）"),
        allowed_tools.join(", "),
        required_tools.join(", "),
        if authorized_tools.is_empty() {
            "（无）".to_string()
        } else {
            authorized_tools.join(", ")
        },
    );
    let previous_status = load_task_status(db.clone(), id.clone(), kind).await?;
    if previous_status == "running" || (!force && previous_status != "scheduled") {
        return Ok(None);
    }
    let db_for_claim = db.clone();
    let id_for_claim = id.clone();
    let task_type = kind.task_type().to_string();
    let previous_status_for_claim = previous_status.clone();
    let execution_for_claim = execution_id.clone();
    let instance_for_claim = instance_id.to_string();
    let changed = blocking(move || {
        db_for_claim.execute_json(
            "UPDATE agent_tasks SET status='running', owner_session_id=?1, started_at=?2, updated_at=?2 \
             WHERE id=?3 AND task_type=?4 AND status=?5 \
             AND EXISTS (SELECT 1 FROM agent_tasks lease \
                 WHERE lease.id=?6 AND lease.task_type=?7 \
                   AND lease.owner_session_id=?8 AND lease.started_at>?2)",
            &[
                json!(execution_for_claim),
                json!(now),
                json!(id_for_claim),
                json!(task_type),
                json!(previous_status_for_claim),
                json!(CONFIG_ROW_ID),
                json!(CONFIG_TASK_TYPE),
                json!(instance_for_claim),
            ],
        )
    })
    .await?;
    if changed == 0 {
        return Ok(None);
    }
    let claimed = ClaimedTask {
        id,
        kind,
        execution_id,
        prompt,
        required_tools,
        allowed_tools,
        authorized_tools,
        started_at: now,
    };
    if let Err(error) = append_trace(
        db.clone(),
        &claimed,
        "execution_started",
        json!({"executionId":claimed.execution_id,"startedAt":now}),
        now,
    )
    .await
    {
        rollback_claim(
            db,
            claimed.id.clone(),
            claimed.execution_id.clone(),
            previous_status,
            kind,
        )
        .await?;
        return Err(error);
    }
    Ok(Some(claimed))
}

async fn load_task_status(db: Db, id: String, kind: TaskKind) -> anyhow::Result<String> {
    let task_type = kind.task_type().to_string();
    let row = blocking(move || {
        db.query_one_json(
            "SELECT status FROM agent_tasks WHERE id=?1 AND task_type=?2",
            &[json!(id), json!(task_type)],
        )
    })
    .await?
    .ok_or_else(|| anyhow::anyhow!("Task not found"))?;
    row.get("status")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("Task runtime status is missing"))
}

async fn rollback_claim(
    db: Db,
    id: String,
    execution_id: String,
    previous_status: String,
    kind: TaskKind,
) -> anyhow::Result<()> {
    let task_type = kind.task_type().to_string();
    blocking(move || {
        db.execute_json(
            "UPDATE agent_tasks SET status=?1, owner_session_id=NULL, started_at=NULL \
             WHERE id=?2 AND task_type=?3 AND status='running' AND owner_session_id=?4",
            &[
                json!(previous_status),
                json!(id),
                json!(task_type),
                json!(execution_id),
            ],
        )
        .map(|_| ())
    })
    .await
}

async fn update_metadata_while_running(
    db: Db,
    id: String,
    kind: TaskKind,
    metadata: Value,
    now: i64,
) -> anyhow::Result<bool> {
    let task_type = kind.task_type().to_string();
    let changed = blocking(move || {
        db.execute_json(
            "UPDATE agent_tasks SET metadata_json=?1, updated_at=?2 \
             WHERE id=?3 AND task_type=?4 AND status='running'",
            &[
                json!(metadata.to_string()),
                json!(now),
                json!(id),
                json!(task_type),
            ],
        )
    })
    .await?;
    Ok(changed == 1)
}

async fn load_due(db: Db, now: i64) -> anyhow::Result<Vec<DueTask>> {
    let scheduled = load_scheduled(db.clone()).await?;
    let long_cycle = load_long_cycle(db).await?;
    let mut due: Vec<(i64, DueTask)> = scheduled
        .into_iter()
        .filter(|task| task.enabled && task.next_run_at.is_some_and(|at| at <= now))
        .map(|task| {
            (
                task.next_run_at.unwrap_or(now),
                DueTask {
                    id: task.id,
                    kind: TaskKind::Scheduled,
                },
            )
        })
        .chain(
            long_cycle
                .into_iter()
                .filter(|task| {
                    task.enabled
                        && task.completed_rounds < task.total_rounds
                        && task.next_run_at.is_some_and(|at| at <= now)
                })
                .map(|task| {
                    (
                        task.next_run_at.unwrap_or(now),
                        DueTask {
                            id: task.id,
                            kind: TaskKind::LongCycle,
                        },
                    )
                }),
        )
        .collect();
    due.sort_by_key(|(at, _)| *at);
    Ok(due.into_iter().map(|(_, task)| task).collect())
}

async fn ensure_config_row(db: Db, now: i64) -> anyhow::Result<()> {
    let metadata = serde_json::to_value(RunnerConfig::default())?.to_string();
    blocking(move || {
        db.execute_json(
            "INSERT OR IGNORE INTO agent_tasks \
             (id,task_type,status,runtime_mode,owner_session_id,intent,role_id,goal,current_node,route_json,graph_json,artifacts_json,checkpoints_json,metadata_json,last_error,created_at,updated_at,started_at,completed_at) \
             VALUES (?1,?2,'active','redclaw',NULL,'redclaw-runner',NULL,NULL,NULL,'{}','[]','[]','[]',?3,NULL,?4,?4,NULL,NULL)",
            &[
                json!(CONFIG_ROW_ID),
                json!(CONFIG_TASK_TYPE),
                json!(metadata),
                json!(now),
            ],
        )
        .map(|_| ())
    })
    .await
}

async fn acquire_or_renew_lease(
    db: Db,
    instance_id: String,
    now: i64,
    valid_until: i64,
) -> anyhow::Result<bool> {
    let changed = blocking(move || {
        db.execute_json(
            "UPDATE agent_tasks SET owner_session_id=?1, started_at=?2, updated_at=?3 \
             WHERE id=?4 AND task_type=?5 \
               AND (owner_session_id IS NULL OR owner_session_id=?1 \
                    OR started_at IS NULL OR started_at<=?3)",
            &[
                json!(instance_id),
                json!(valid_until),
                json!(now),
                json!(CONFIG_ROW_ID),
                json!(CONFIG_TASK_TYPE),
            ],
        )
    })
    .await?;
    Ok(changed == 1)
}

async fn release_lease(db: Db, instance_id: String) -> anyhow::Result<()> {
    blocking(move || {
        db.execute_json(
            "UPDATE agent_tasks SET owner_session_id=NULL, started_at=NULL \
             WHERE id=?1 AND task_type=?2 AND owner_session_id=?3",
            &[
                json!(CONFIG_ROW_ID),
                json!(CONFIG_TASK_TYPE),
                json!(instance_id),
            ],
        )
        .map(|_| ())
    })
    .await
}

async fn load_lease(db: Db) -> anyhow::Result<Option<(String, i64)>> {
    let row = blocking(move || {
        db.query_one_json(
            "SELECT owner_session_id,started_at FROM agent_tasks WHERE id=?1 AND task_type=?2",
            &[json!(CONFIG_ROW_ID), json!(CONFIG_TASK_TYPE)],
        )
    })
    .await?;
    Ok(row.and_then(|row| {
        Some((
            row.get("owner_session_id")?.as_str()?.to_string(),
            row.get("started_at")?.as_i64()?,
        ))
    }))
}

async fn load_config(db: Db) -> anyhow::Result<RunnerConfig> {
    let db_for_query = db.clone();
    let row = blocking(move || {
        db_for_query.query_one_json(
            "SELECT metadata_json FROM agent_tasks WHERE id=?1 AND task_type=?2",
            &[json!(CONFIG_ROW_ID), json!(CONFIG_TASK_TYPE)],
        )
    })
    .await?;
    let mut config = match row {
        Some(row) => parse_metadata(&row)?,
        None => RunnerConfig::default(),
    };
    // 旧占位实现曾默认显示为启用，但没有执行器；读取时强制归一为真实能力。
    config.keep_alive_when_no_window = false;
    config.heartbeat.enabled = false;
    for project in config.project_states.values_mut() {
        project.enabled = false;
    }
    Ok(config)
}

async fn save_config(db: Db, config: RunnerConfig, now: i64) -> anyhow::Result<()> {
    let status = if config.enabled { "active" } else { "paused" };
    save_definition(
        db,
        CONFIG_ROW_ID.into(),
        status,
        serde_json::to_value(config)?,
        None,
        now,
        None,
    )
    .await
}

async fn insert_definition(
    db: Db,
    id: String,
    task_type: &str,
    status: &str,
    metadata: Value,
    now: i64,
) -> anyhow::Result<()> {
    let task_type = task_type.to_string();
    let status = status.to_string();
    blocking(move || {
        db.execute_json(
            "INSERT INTO agent_tasks \
             (id,task_type,status,runtime_mode,owner_session_id,intent,role_id,goal,current_node,route_json,graph_json,artifacts_json,checkpoints_json,metadata_json,last_error,created_at,updated_at,started_at,completed_at) \
             VALUES (?1,?2,?3,'redclaw',NULL,'redclaw-runner',NULL,NULL,NULL,'{}','[]','[]','[]',?4,NULL,?5,?5,NULL,NULL)",
            &[
                json!(id),
                json!(task_type),
                json!(status),
                json!(metadata.to_string()),
                json!(now),
            ],
        )
        .map(|_| ())
    })
    .await
}

async fn save_definition(
    db: Db,
    id: String,
    status: &str,
    metadata: Value,
    last_error: Option<String>,
    now: i64,
    receipt_path: Option<String>,
) -> anyhow::Result<()> {
    let status = status.to_string();
    let completed_at = matches!(status.as_str(), "completed" | "failed").then_some(now);
    let artifacts = receipt_path
        .as_ref()
        .map(|path| json!([{"type":"execution-receipt","path":path}]).to_string());
    let changed = blocking(move || {
        db.execute_json(
            "UPDATE agent_tasks SET status=?1, metadata_json=?2, last_error=?3, updated_at=?4, \
             artifacts_json=COALESCE(?5,artifacts_json), completed_at=?6, \
             owner_session_id=CASE WHEN id='redclaw-runner-config' THEN owner_session_id ELSE NULL END, \
             started_at=CASE WHEN id='redclaw-runner-config' THEN started_at ELSE NULL END \
             WHERE id=?7",
            &[
                json!(status),
                json!(metadata.to_string()),
                last_error.map_or(Value::Null, Value::String),
                json!(now),
                artifacts.map_or(Value::Null, Value::String),
                completed_at.map_or(Value::Null, |value| json!(value)),
                json!(id),
            ],
        )
    })
    .await?;
    if changed == 0 {
        anyhow::bail!("Task was removed while execution was completing");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn save_claimed_definition(
    db: Db,
    id: String,
    execution_id: String,
    instance_id: String,
    execution_status: &str,
    status: &str,
    metadata: Value,
    last_error: Option<String>,
    now: i64,
    receipt_path: Option<String>,
) -> anyhow::Result<()> {
    let execution_status = execution_status.to_string();
    let status = status.to_string();
    let completed_at = matches!(status.as_str(), "completed" | "failed").then_some(now);
    let artifacts = receipt_path
        .as_ref()
        .map(|path| json!([{"type":"execution-receipt","path":path}]).to_string());
    let trace_id = format!("trace_{}", uuid::Uuid::new_v4());
    let trace_event_type = if execution_status == "succeeded" {
        "execution_completed"
    } else {
        "execution_failed"
    }
    .to_string();
    let trace_payload = json!({
        "executionId":execution_id.clone(),
        "status":execution_status.clone(),
        "receiptPath":receipt_path.clone(),
        "error":last_error.clone(),
        "completedAt":now,
    })
    .to_string();
    let update_params = vec![
        json!(status),
        json!(metadata.to_string()),
        last_error.clone().map_or(Value::Null, Value::String),
        json!(now),
        artifacts.map_or(Value::Null, Value::String),
        completed_at.map_or(Value::Null, |value| json!(value)),
        json!(id.clone()),
        json!(execution_id.clone()),
        json!(CONFIG_ROW_ID),
        json!(CONFIG_TASK_TYPE),
        json!(instance_id),
    ];
    let trace_params = vec![
        json!(trace_id),
        json!(id),
        json!("scheduler"),
        json!(trace_event_type),
        json!(trace_payload),
        json!(now),
    ];
    let affected = blocking(move || {
        db.execute_transaction_json(&[
            (
                "UPDATE agent_tasks SET status=?1, metadata_json=?2, last_error=?3, updated_at=?4, \
             artifacts_json=COALESCE(?5,artifacts_json), completed_at=?6, \
             owner_session_id=NULL, started_at=NULL \
             WHERE id=?7 AND status='running' AND owner_session_id=?8 \
             AND EXISTS (SELECT 1 FROM agent_tasks lease \
                 WHERE lease.id=?9 AND lease.task_type=?10 \
                   AND lease.owner_session_id=?11 AND lease.started_at>?4)"
                    .to_string(),
                update_params,
            ),
            (
                "INSERT INTO agent_task_traces (id,task_id,node_id,event_type,payload_json,created_at) \
                 SELECT ?1,?2,?3,?4,?5,?6 WHERE changes()=1"
                    .to_string(),
                trace_params,
            ),
        ])
    })
    .await?;
    if affected.first().copied().unwrap_or(0) == 0 {
        anyhow::bail!(
            "Stale execution completion was rejected because its claim or scheduler lease is no longer current"
        );
    }
    Ok(())
}

async fn load_scheduled(db: Db) -> anyhow::Result<Vec<ScheduledTask>> {
    load_typed(db, SCHEDULED_TASK_TYPE).await
}

async fn load_long_cycle(db: Db) -> anyhow::Result<Vec<LongCycleTask>> {
    load_typed(db, LONG_CYCLE_TASK_TYPE).await
}

async fn load_typed<T>(db: Db, task_type: &str) -> anyhow::Result<Vec<T>>
where
    T: for<'de> Deserialize<'de> + Send + 'static,
{
    let task_type = task_type.to_string();
    let rows = blocking(move || {
        db.query_all_json(
            "SELECT metadata_json FROM agent_tasks WHERE task_type=?1 ORDER BY updated_at DESC",
            &[json!(task_type)],
        )
    })
    .await?;
    rows.iter().map(parse_metadata).collect()
}

async fn load_runtime_states(
    db: Db,
    task_type: &str,
) -> anyhow::Result<HashMap<String, RuntimeState>> {
    let task_type = task_type.to_string();
    let rows = blocking(move || {
        db.query_all_json(
            "SELECT id,status,started_at FROM agent_tasks WHERE task_type=?1",
            &[json!(task_type)],
        )
    })
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some((
                row.get("id")?.as_str()?.to_string(),
                RuntimeState {
                    status: row.get("status")?.as_str()?.to_string(),
                    started_at: row.get("started_at").and_then(Value::as_i64),
                },
            ))
        })
        .collect())
}

async fn find_scheduled(db: Db, id: String) -> anyhow::Result<ScheduledTask> {
    find_typed(db, id, SCHEDULED_TASK_TYPE).await
}

async fn find_long_cycle(db: Db, id: String) -> anyhow::Result<LongCycleTask> {
    find_typed(db, id, LONG_CYCLE_TASK_TYPE).await
}

async fn find_typed<T>(db: Db, id: String, task_type: &str) -> anyhow::Result<T>
where
    T: for<'de> Deserialize<'de> + Send + 'static,
{
    let task_type = task_type.to_string();
    let row = blocking(move || {
        db.query_one_json(
            "SELECT metadata_json FROM agent_tasks WHERE id=?1 AND task_type=?2",
            &[json!(id), json!(task_type)],
        )
    })
    .await?
    .ok_or_else(|| anyhow::anyhow!("Task not found"))?;
    parse_metadata(&row)
}

fn parse_metadata<T>(row: &Value) -> anyhow::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let raw = row
        .get("metadata_json")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Task metadata is missing"))?;
    Ok(serde_json::from_str(raw)?)
}

async fn append_trace(
    db: Db,
    task: &ClaimedTask,
    event_type: &str,
    payload: Value,
    now: i64,
) -> anyhow::Result<()> {
    let trace_id = format!("trace_{}", uuid::Uuid::new_v4());
    let task_id = task.id.clone();
    let event_type = event_type.to_string();
    blocking(move || {
        db.execute_json(
            "INSERT INTO agent_task_traces (id,task_id,node_id,event_type,payload_json,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            &[
                json!(trace_id),
                json!(task_id),
                json!("scheduler"),
                json!(event_type),
                json!(payload.to_string()),
                json!(now),
            ],
        )
        .map(|_| ())
    })
    .await
}

async fn recover_interrupted_tasks(db: Db, now: i64) -> anyhow::Result<()> {
    recover_interrupted_tasks_excluding(db, now, &HashSet::new()).await
}

async fn recover_interrupted_tasks_excluding(
    db: Db,
    now: i64,
    active_ids: &HashSet<String>,
) -> anyhow::Result<()> {
    let rows = {
        let db = db.clone();
        blocking(move || {
            db.query_all_json(
                "SELECT id,task_type,metadata_json FROM agent_tasks WHERE status='running' AND task_type IN (?1,?2)",
                &[json!(SCHEDULED_TASK_TYPE), json!(LONG_CYCLE_TASK_TYPE)],
            )
        })
        .await?
    };
    for row in rows {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if active_ids.contains(&id) {
            continue;
        }
        let task_type = row
            .get("task_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message =
            "上次执行因应用退出、租约丢失或完成记录持久化失败而中断；为避免重复外部副作用，未自动重放，已暂停并需人工确认后再启用"
                .to_string();
        if task_type == SCHEDULED_TASK_TYPE {
            let mut task: ScheduledTask = parse_metadata(&row)?;
            task.last_error = Some(message.clone());
            task.last_result = Some("error".into());
            task.updated_at = now;
            task.enabled = false;
            task.next_run_at = None;
            save_definition(
                db.clone(),
                id,
                "failed",
                serde_json::to_value(task)?,
                Some(message),
                now,
                None,
            )
            .await?;
        } else if task_type == LONG_CYCLE_TASK_TYPE {
            let mut task: LongCycleTask = parse_metadata(&row)?;
            task.last_error = Some(message.clone());
            task.last_result = Some("error".into());
            task.updated_at = now;
            task.enabled = false;
            task.status = "paused".into();
            task.next_run_at = None;
            save_definition(
                db.clone(),
                id,
                "failed",
                serde_json::to_value(task)?,
                Some(message),
                now,
                None,
            )
            .await?;
        }
    }
    Ok(())
}

async fn resume_enabled_definitions(db: Db, now: i64) -> anyhow::Result<()> {
    let scheduled_states = load_runtime_states(db.clone(), SCHEDULED_TASK_TYPE).await?;
    for mut task in load_scheduled(db.clone()).await? {
        if !task.enabled {
            continue;
        }
        let runtime_status = scheduled_states
            .get(&task.id)
            .map(|state| state.status.as_str());
        if runtime_status == Some("running") {
            continue;
        }
        if runtime_status == Some("scheduled") && task.next_run_at.is_some() {
            continue;
        }
        if task.next_run_at.is_none() && task.mode != "once" {
            task.next_run_at = compute_next_scheduled(&task, now);
        }
        if task.next_run_at.is_none() {
            continue;
        }
        task.updated_at = now;
        save_definition(
            db.clone(),
            task.id.clone(),
            "scheduled",
            serde_json::to_value(&task)?,
            task.last_error.clone(),
            now,
            task.last_receipt_path.clone(),
        )
        .await?;
    }
    let long_states = load_runtime_states(db.clone(), LONG_CYCLE_TASK_TYPE).await?;
    for mut task in load_long_cycle(db.clone()).await? {
        if !task.enabled || task.completed_rounds >= task.total_rounds {
            continue;
        }
        let runtime_status = long_states.get(&task.id).map(|state| state.status.as_str());
        if runtime_status == Some("running") {
            continue;
        }
        if runtime_status == Some("scheduled") && task.next_run_at.is_some() {
            continue;
        }
        if task.next_run_at.is_none() {
            task.next_run_at = Some(now + task.interval_minutes * 60_000);
        }
        task.status = "running".into();
        task.updated_at = now;
        save_definition(
            db.clone(),
            task.id.clone(),
            "scheduled",
            serde_json::to_value(&task)?,
            task.last_error.clone(),
            now,
            task.last_receipt_path.clone(),
        )
        .await?;
    }
    Ok(())
}

async fn write_receipt(
    root: &Path,
    task_id: &str,
    execution_id: &str,
    receipt: &Value,
) -> anyhow::Result<PathBuf> {
    let directory = root.join(safe_component(task_id));
    tokio::fs::create_dir_all(&directory).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let path = directory.join(format!("{}.json", safe_component(execution_id)));
    let temporary = directory.join(format!(".{}.tmp", safe_component(execution_id)));
    let bytes = serde_json::to_vec_pretty(receipt)?;
    if bytes.len() > MAX_RECEIPT_BYTES {
        anyhow::bail!(
            "execution receipt exceeds {} byte safety limit",
            MAX_RECEIPT_BYTES
        );
    }
    let mut file = tokio::fs::File::create(&temporary).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).await?;
    }
    use tokio::io::AsyncWriteExt;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temporary, &path).await?;
    Ok(path)
}

fn sanitize_receipt_value(value: &Value) -> Value {
    fn visit(value: &Value, depth: usize) -> Value {
        if depth > 16 {
            return json!("[truncated: nesting limit]");
        }
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| {
                        let normalized = key
                            .chars()
                            .filter(|character| character.is_ascii_alphanumeric())
                            .flat_map(char::to_lowercase)
                            .collect::<String>();
                        let sensitive = [
                            "apikey",
                            "authorization",
                            "cookie",
                            "password",
                            "secret",
                            "token",
                        ]
                        .iter()
                        .any(|needle| normalized.contains(needle));
                        (
                            key.clone(),
                            if sensitive {
                                json!("[redacted]")
                            } else {
                                visit(value, depth + 1)
                            },
                        )
                    })
                    .collect(),
            ),
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .take(MAX_TOOL_CALLS_PER_EXECUTION * 4)
                    .map(|value| visit(value, depth + 1))
                    .collect(),
            ),
            Value::String(text) => {
                if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                    if parsed.is_object() || parsed.is_array() {
                        return Value::String(visit(&parsed, depth + 1).to_string());
                    }
                }
                Value::String(truncate_chars(text, MAX_RECEIPT_STRING_CHARS))
            }
            _ => value.clone(),
        }
    }
    visit(value, 0)
}

fn execution_receipt_root(db: &Db) -> PathBuf {
    if let Ok(value) = std::env::var("YUNYING_REDCLAW_RECEIPT_DIR") {
        let value = value.trim();
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    let settings = db.settings().get().unwrap_or_else(|_| json!({}));
    let workspace = settings
        .get("workspace_dir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let active_space = settings
        .get("active_space_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default");
    match workspace {
        Some(workspace) => workspace
            .join(active_space)
            .join("redclaw")
            .join("executions"),
        None => std::env::var_os("YUNYING_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("redclaw")
            .join("executions"),
    }
}

fn compute_next_scheduled(task: &ScheduledTask, from_ms: i64) -> Option<i64> {
    match task.mode.as_str() {
        "interval" => Some(from_ms + task.interval_minutes.unwrap_or(60) * 60_000),
        "daily" => next_local_time(from_ms, task.time.as_deref()?, None),
        "weekly" => next_local_time(from_ms, task.time.as_deref()?, Some(&task.weekdays)),
        "once" => task.run_at.filter(|at| *at > from_ms),
        _ => None,
    }
}

fn next_local_time(from_ms: i64, hhmm: &str, weekdays: Option<&[i64]>) -> Option<i64> {
    let (hour, minute) = parse_hhmm(hhmm)?;
    let now = Local.timestamp_millis_opt(from_ms).single()?;
    for offset in 0..=8 {
        let date: NaiveDate = now
            .date_naive()
            .checked_add_days(chrono::Days::new(offset))?;
        if let Some(days) = weekdays {
            let weekday = i64::from(date.weekday().num_days_from_sunday());
            if !days.contains(&weekday) {
                continue;
            }
        }
        let candidate =
            match Local.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0) {
                LocalResult::Single(value) => value,
                // DST 回拨时选择较早的时刻；如果已经过去，循环会继续。
                LocalResult::Ambiguous(first, _) => first,
                LocalResult::None => continue,
            };
        if candidate.timestamp_millis() > from_ms {
            return Some(candidate.timestamp_millis());
        }
    }
    None
}

fn scheduled_json(task: &ScheduledTask) -> Value {
    json!({
        "id":task.id,
        "name":task.name,
        "enabled":task.enabled,
        "mode":task.mode,
        "prompt":task.prompt,
        "projectId":task.project_id,
        "workItemId":task.work_item_id,
        "subagentRoles":task.subagent_roles,
        "requiredTools":task.required_tools,
        "allowedTools":task.allowed_tools,
        "authorizedTools":task.authorized_tools,
        "intervalMinutes":task.interval_minutes,
        "time":task.time,
        "weekdays":task.weekdays,
        "runAt":timestamp_value(task.run_at),
        "createdAt":task.created_at,
        "updatedAt":task.updated_at,
        "lastRunAt":timestamp_value(task.last_run_at),
        "lastResult":task.last_result,
        "lastOutput":task.last_output,
        "lastError":task.last_error,
        "nextRunAt":timestamp_value(task.next_run_at),
        "lastReceiptPath":task.last_receipt_path,
    })
}

fn long_cycle_json(task: &LongCycleTask) -> Value {
    json!({
        "id":task.id,
        "name":task.name,
        "enabled":task.enabled,
        "status":task.status,
        "objective":task.objective,
        "stepPrompt":task.step_prompt,
        "projectId":task.project_id,
        "workItemId":task.work_item_id,
        "subagentRoles":task.subagent_roles,
        "requiredTools":task.required_tools,
        "allowedTools":task.allowed_tools,
        "authorizedTools":task.authorized_tools,
        "intervalMinutes":task.interval_minutes,
        "totalRounds":task.total_rounds,
        "completedRounds":task.completed_rounds,
        "createdAt":task.created_at,
        "updatedAt":task.updated_at,
        "lastRunAt":timestamp_value(task.last_run_at),
        "lastResult":task.last_result,
        "lastOutput":task.last_output,
        "lastError":task.last_error,
        "nextRunAt":timestamp_value(task.next_run_at),
        "lastReceiptPath":task.last_receipt_path,
    })
}

fn task_json_with_runtime(mut task: Value, state: Option<&RuntimeState>) -> Value {
    if let Some(map) = task.as_object_mut() {
        if state.is_some_and(|state| state.status != "scheduled") {
            map.insert("nextRunAt".into(), Value::Null);
        }
        map.insert(
            "executionStatus".into(),
            state
                .map(|state| json!(state.status))
                .unwrap_or(Value::Null),
        );
        map.insert(
            "startedAt".into(),
            timestamp_value(state.and_then(|state| state.started_at)),
        );
    }
    task
}

fn project_state_json(state: &ProjectState) -> Value {
    json!({
        "projectId":state.project_id,
        "enabled":state.enabled,
        "prompt":state.prompt,
        "lastRunAt":timestamp_value(state.last_run_at),
        "lastResult":state.last_result,
        "lastError":state.last_error,
    })
}

fn heartbeat_json(config: &HeartbeatConfig) -> Value {
    json!({
        "enabled":config.enabled,
        "intervalMinutes":config.interval_minutes,
        "suppressEmptyReport":config.suppress_empty_report,
        "reportToMainSession":config.report_to_main_session,
        "prompt":config.prompt,
        "lastRunAt":timestamp_value(config.last_run_at),
        "nextRunAt":timestamp_value(config.next_run_at),
        "lastDigest":config.last_digest,
    })
}

fn timestamp_value(value: Option<i64>) -> Value {
    value
        .map(|value| json!(value.to_string()))
        .unwrap_or(Value::Null)
}

fn required_str(payload: &Value, key: &str) -> anyhow::Result<String> {
    optional_str(payload, key).ok_or_else(|| anyhow::anyhow!("{key} is required"))
}

fn optional_str(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).and_then(non_empty)
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    let mut values: Vec<String> = value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter_map(non_empty)
                .collect()
        })
        .unwrap_or_default();
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
    values
}

fn task_tool_policy(payload: &Value) -> anyhow::Result<(Vec<String>, Vec<String>, Vec<String>)> {
    let required = string_array(payload.get("requiredTools"));
    let allowed = string_array(payload.get("allowedTools"));
    let authorized = string_array(payload.get("authorizedTools"));
    if required.is_empty() {
        anyhow::bail!("requiredTools must contain at least one verifiable tool");
    }
    if allowed.is_empty() {
        anyhow::bail!(
            "allowedTools must be explicit and non-empty; default policy denies all tools"
        );
    }
    for name in required.iter().chain(&allowed).chain(&authorized) {
        if name.len() > 128
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            anyhow::bail!("Invalid tool name in authorization snapshot: {name}");
        }
    }
    let missing_allowed: Vec<&str> = required
        .iter()
        .filter(|required| {
            !allowed
                .iter()
                .any(|allowed| tool_name_matches(allowed, required))
        })
        .map(String::as_str)
        .collect();
    if !missing_allowed.is_empty() {
        anyhow::bail!(
            "requiredTools must be a subset of allowedTools: {}",
            missing_allowed.join(", ")
        );
    }
    let unauthorized_names: Vec<&str> = authorized
        .iter()
        .filter(|authorized| {
            !allowed
                .iter()
                .any(|allowed| tool_name_matches(allowed, authorized))
        })
        .map(String::as_str)
        .collect();
    if !unauthorized_names.is_empty() {
        anyhow::bail!(
            "authorizedTools must be a subset of allowedTools: {}",
            unauthorized_names.join(", ")
        );
    }
    let missing_consent: Vec<&str> = allowed
        .iter()
        .filter(|tool| requires_explicit_authorization(tool))
        .filter(|tool| {
            !authorized
                .iter()
                .any(|authorized| tool_name_matches(authorized, tool))
        })
        .map(String::as_str)
        .collect();
    if !missing_consent.is_empty() {
        anyhow::bail!(
            "付费生成或发布工具必须在 authorizedTools 中显式确认：{}",
            missing_consent.join(", ")
        );
    }
    Ok((required, allowed, authorized))
}

fn validate_task_instruction(value: &str, field: &str) -> anyhow::Result<()> {
    if value.contains('〔') && value.contains("请填写") {
        anyhow::bail!("{field} contains unfilled placeholders");
    }
    if value.chars().count() > 64_000 {
        anyhow::bail!("{field} exceeds 64000 character limit");
    }
    Ok(())
}

fn requires_explicit_authorization(name: &str) -> bool {
    let basename = tool_basename(name).to_ascii_lowercase();
    matches!(
        basename.as_str(),
        "generate_image"
            | "image_gen"
            | "image_selector"
            | "google_imagen"
            | "seedance_video"
            | "video_selector"
            | "video_stitch"
            | "talking_head"
            | "lip_sync"
            | "tts_selector"
            | "music_gen"
            | "upload_video"
            | "upload_note"
    ) || basename.starts_with("publish_")
        || basename.ends_with("_publish")
        || basename.starts_with("upload_")
        || basename.starts_with("generate_")
        || basename.ends_with("_image")
        || basename.ends_with("_video")
        || basename.ends_with("_tts")
        || basename.ends_with("_music")
}

fn sanitize_weekdays(value: Option<&Value>) -> Vec<i64> {
    let mut values: Vec<i64> = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .filter(|value| (0..=6).contains(value))
        .collect();
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        values.push(1);
    }
    values
}

fn sanitize_hhmm(value: Option<&str>) -> anyhow::Result<String> {
    let value = value
        .and_then(non_empty)
        .ok_or_else(|| anyhow::anyhow!("time is required for daily/weekly task"))?;
    let (hour, minute) =
        parse_hhmm(&value).ok_or_else(|| anyhow::anyhow!("time must use 24-hour HH:mm format"))?;
    Ok(format!("{hour:02}:{minute:02}"))
}

fn parse_hhmm(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

fn parse_time_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp_millis())
        .or_else(|| value.trim().parse::<i64>().ok())
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect()
}

fn contains_explicit_tool_failure(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.iter().any(|(key, value)| {
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                matches!(
                    normalized.as_str(),
                    "success" | "persisted" | "executed" | "ok"
                ) && value.as_bool() == Some(false)
                    || matches!(normalized.as_str(), "iserror" | "dryrun")
                        && value.as_bool() == Some(true)
                    || normalized == "status"
                        && value.as_str().is_some_and(|status| {
                            matches!(
                                status.trim().to_ascii_lowercase().as_str(),
                                "failed" | "failure" | "error" | "cancelled" | "canceled"
                            )
                        })
            }) || map.values().any(contains_explicit_tool_failure)
        }
        Value::Array(values) => values.iter().any(contains_explicit_tool_failure),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|value| contains_explicit_tool_failure(&value)),
        _ => false,
    }
}

fn contains_disallowed_tool_input(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            let normalized = key
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>();
            normalized == "dryrun" && value.as_bool() == Some(true)
                || contains_disallowed_tool_input(value)
        }),
        Value::Array(values) => values.iter().any(contains_disallowed_tool_input),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|value| contains_disallowed_tool_input(&value)),
        _ => false,
    }
}

fn tool_basename(name: &str) -> &str {
    name.rsplit_once("__")
        .map(|(_, basename)| basename)
        .unwrap_or(name)
}

fn tool_name_matches(actual: &str, required: &str) -> bool {
    actual == required || (!required.contains("__") && tool_basename(actual) == required)
}

fn missing_required_tools<'a>(required: &'a [String], tools: &[ToolEvidence]) -> Vec<&'a str> {
    required
        .iter()
        .filter(|required_name| {
            !tools
                .iter()
                .any(|tool| tool_name_matches(&tool.name, required_name))
        })
        .map(String::as_str)
        .collect()
}

fn value_contains_bool(value: &Value, key: &str, expected: bool) -> bool {
    match value {
        Value::Object(map) => {
            map.get(key).and_then(Value::as_bool) == Some(expected)
                || map
                    .values()
                    .any(|value| value_contains_bool(value, key, expected))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_bool(value, key, expected)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|value| value_contains_bool(&value, key, expected)),
        _ => false,
    }
}

fn value_has_nonempty_key(value: &Value, keys: &[&str]) -> bool {
    match value {
        Value::Object(map) => {
            map.iter().any(|(key, value)| {
                keys.contains(&key.as_str())
                    && match value {
                        Value::Null => false,
                        Value::String(value) => !value.trim().is_empty(),
                        Value::Array(values) => !values.is_empty(),
                        Value::Object(values) => !values.is_empty(),
                        _ => true,
                    }
            }) || map
                .values()
                .any(|value| value_has_nonempty_key(value, keys))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| value_has_nonempty_key(value, keys)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|value| value_has_nonempty_key(&value, keys)),
        _ => false,
    }
}

fn unverified_required_tools<'a>(required: &'a [String], tools: &[ToolEvidence]) -> Vec<&'a str> {
    required
        .iter()
        .filter(|required_name| {
            let Some(tool) = tools
                .iter()
                .find(|tool| tool_name_matches(&tool.name, required_name))
            else {
                return true;
            };
            let Some(output) = tool.output.as_ref() else {
                return true;
            };
            let verified = match tool_basename(required_name) {
                "create_note" => value_contains_bool(output, "persisted", true),
                "generate_image" => {
                    value_contains_bool(output, "executed", true)
                        && value_has_nonempty_key(output, &["assets"])
                }
                "seedance_video" => {
                    (value_contains_bool(output, "success", true)
                        || value_contains_bool(output, "ok", true))
                        && value_has_nonempty_key(
                            output,
                            &["artifacts", "output", "output_path", "path", "file"],
                        )
                }
                _ => {
                    value_contains_bool(output, "success", true)
                        || value_contains_bool(output, "ok", true)
                        || value_contains_bool(output, "executed", true)
                        || value_contains_bool(output, "persisted", true)
                }
            };
            !verified
        })
        .map(String::as_str)
        .collect()
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

async fn blocking<F, R>(operation: F) -> anyhow::Result<R>
where
    F: FnOnce() -> anyhow::Result<R> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| anyhow::anyhow!("blocking database task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct ManualClock(AtomicI64);

    impl Clock for ManualClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct MockExecutor {
        calls: AtomicUsize,
    }

    struct CancellationExecutor {
        started: Notify,
        cancelled: AtomicUsize,
    }

    #[async_trait]
    impl TaskExecutor for CancellationExecutor {
        async fn execute(
            &self,
            _task: &ClaimedTask,
            cancel: CancellationToken,
        ) -> Result<ExecutionOutput, ExecutionFailure> {
            self.started.notify_one();
            cancel.cancelled().await;
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            Err(ExecutionFailure::Cancelled)
        }
    }

    #[async_trait]
    impl TaskExecutor for MockExecutor {
        async fn execute(
            &self,
            task: &ClaimedTask,
            _cancel: CancellationToken,
        ) -> Result<ExecutionOutput, ExecutionFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ExecutionOutput {
                response: format!("真实 mock 执行：{}", task.prompt),
                tools: vec![ToolEvidence {
                    call_id: "mock-call".into(),
                    name: "create_note".into(),
                    input: json!({"title":"test"}),
                    output: Some(json!({"persisted":true})),
                }],
            })
        }
    }

    #[tokio::test]
    async fn one_minute_and_long_cycle_are_time_controlled_and_not_duplicated() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("test.db")).unwrap();
        db.settings()
            .save(&json!({"workspace_dir":temp.path().to_string_lossy()}))
            .unwrap();
        let base = 1_800_000_000_000i64;
        let clock = Arc::new(ManualClock(AtomicI64::new(base)));
        let executor = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
        });
        let scheduler = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            executor.clone(),
            clock.clone(),
            temp.path().join("receipts"),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        let run_at = chrono::Utc
            .timestamp_millis_opt(base + 60_000)
            .single()
            .unwrap()
            .to_rfc3339();
        let scheduled = scheduler
            .add_scheduled(&json!({
                "mode":"once","name":"+1 minute","prompt":"write receipt","runAt":run_at,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let scheduled_id = scheduled["task"]["id"].as_str().unwrap().to_string();
        let long = scheduler
            .add_long_cycle(&json!({
                "name":"+5 minute","objective":"verify long cycle","stepPrompt":"write receipt",
                "intervalMinutes":5,"totalRounds":1,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let long_id = long["task"]["id"].as_str().unwrap().to_string();

        clock.0.store(base + 59_000, Ordering::SeqCst);
        assert!(scheduler.inner.process_due().await.unwrap().is_empty());
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);

        clock.0.store(base + 60_000, Ordering::SeqCst);
        assert_eq!(
            scheduler.inner.process_due().await.unwrap(),
            vec![scheduled_id.clone()]
        );
        for _ in 0..100 {
            if executor.calls.load(Ordering::SeqCst) == 1
                && !scheduler
                    .inner
                    .context
                    .active
                    .lock()
                    .await
                    .contains_key(&scheduled_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        let one = scheduler.list_scheduled().await.unwrap();
        let task = one["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == scheduled_id)
            .unwrap();
        assert_eq!(task["enabled"], json!(false));
        assert_eq!(task["lastResult"], json!("success"));
        assert!(task["lastOutput"]
            .as_str()
            .unwrap()
            .contains("真实 mock 执行"));
        assert!(task["lastReceiptPath"].as_str().unwrap().ends_with(".json"));

        // 多次 tick 不能重复认领已完成的一次性任务。
        assert!(scheduler.inner.process_due().await.unwrap().is_empty());
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);

        clock.0.store(base + 300_000, Ordering::SeqCst);
        assert_eq!(
            scheduler.inner.process_due().await.unwrap(),
            vec![long_id.clone()]
        );
        for _ in 0..100 {
            if executor.calls.load(Ordering::SeqCst) == 2
                && !scheduler
                    .inner
                    .context
                    .active
                    .lock()
                    .await
                    .contains_key(&long_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
        let long_tasks = scheduler.list_long_cycle().await.unwrap();
        let task = long_tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == long_id)
            .unwrap();
        assert_eq!(task["completedRounds"], json!(1));
        assert_eq!(task["status"], json!("completed"));
        assert_eq!(task["lastResult"], json!("success"));
    }

    #[tokio::test]
    async fn definitions_survive_restart_but_interrupted_once_is_not_replayed() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("restart.db");
        let db = Db::open(&db_path).unwrap();
        let base = 1_800_000_000_000i64;
        let clock = Arc::new(ManualClock(AtomicI64::new(base)));
        let executor = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
        });
        let scheduler = RedClawScheduler::start_with(
            db.clone(),
            Arc::new(NoopEmitter),
            executor.clone(),
            clock.clone(),
            temp.path().join("receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let run_at = chrono::Utc
            .timestamp_millis_opt(base + 60_000)
            .single()
            .unwrap()
            .to_rfc3339();
        let created = scheduler
            .add_scheduled(&json!({
                "mode":"once","prompt":"do not duplicate","runAt":run_at,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let id = created["task"]["id"].as_str().unwrap().to_string();
        let long = scheduler
            .add_long_cycle(&json!({
                "objective":"do not repeat a partial round","stepPrompt":"write","intervalMinutes":5,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let long_id = long["task"]["id"].as_str().unwrap().to_string();

        // 模拟进程在外部副作用可能已经发生后崩溃：DB 留下 running claim。
        db.execute_json(
            "UPDATE agent_tasks SET status='running', started_at=?1 WHERE id IN (?2,?3)",
            &[json!(base + 60_000), json!(id), json!(long_id)],
        )
        .unwrap();
        drop(scheduler);

        clock.0.store(base + 120_000, Ordering::SeqCst);
        let restarted = RedClawScheduler::start_with(
            Db::open(&db_path).unwrap(),
            Arc::new(NoopEmitter),
            executor.clone(),
            clock,
            temp.path().join("receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let tasks = restarted.list_scheduled().await.unwrap();
        let task = tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        assert_eq!(task["enabled"], json!(false));
        assert!(task["lastError"].as_str().unwrap().contains("未自动重放"));
        let long_tasks = restarted.list_long_cycle().await.unwrap();
        let long_task = long_tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == long_id)
            .unwrap();
        assert_eq!(long_task["enabled"], json!(false));
        assert_eq!(long_task["status"], json!("paused"));
        assert_eq!(long_task["completedRounds"], json!(0));
        assert!(long_task["lastError"]
            .as_str()
            .unwrap()
            .contains("人工确认"));
        assert!(restarted.inner.process_due().await.unwrap().is_empty());
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disabling_a_running_long_cycle_cancels_without_counting_a_round() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("cancel.db")).unwrap();
        let base = 1_800_000_000_000i64;
        let clock = Arc::new(ManualClock(AtomicI64::new(base)));
        let executor = Arc::new(CancellationExecutor {
            started: Notify::new(),
            cancelled: AtomicUsize::new(0),
        });
        let scheduler = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            executor.clone(),
            clock,
            temp.path().join("receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let created = scheduler
            .add_long_cycle(&json!({
                "objective":"cancel safely","stepPrompt":"wait","intervalMinutes":5,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let id = created["task"]["id"].as_str().unwrap().to_string();
        scheduler
            .run_long_cycle_now(&json!({"taskId":id}))
            .await
            .unwrap();
        executor.started.notified().await;
        let running = scheduler.list_long_cycle().await.unwrap();
        let running_task = running["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        assert_eq!(running_task["executionStatus"], json!("running"));
        assert!(running_task["startedAt"].is_string());
        scheduler
            .set_long_cycle_enabled(&json!({"taskId":id,"enabled":false}))
            .await
            .unwrap();
        for _ in 0..100 {
            if !scheduler
                .inner
                .context
                .active
                .lock()
                .await
                .contains_key(&id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(executor.cancelled.load(Ordering::SeqCst), 1);
        let tasks = scheduler.list_long_cycle().await.unwrap();
        let task = tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        assert_eq!(task["completedRounds"], json!(0));
        assert_eq!(task["status"], json!("paused"));
        assert_eq!(task["lastResult"], json!("skipped"));
        assert!(task["lastError"].as_str().unwrap().contains("已取消"));
    }

    #[tokio::test]
    async fn receipt_io_failure_is_recorded_as_failure_not_left_running() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("receipt-failure.db")).unwrap();
        let invalid_root = temp.path().join("not-a-directory");
        std::fs::write(&invalid_root, b"file").unwrap();
        let base = 1_800_000_000_000i64;
        let clock = Arc::new(ManualClock(AtomicI64::new(base)));
        let scheduler = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            Arc::new(MockExecutor {
                calls: AtomicUsize::new(0),
            }),
            clock.clone(),
            invalid_root,
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let run_at = chrono::Utc
            .timestamp_millis_opt(base + 60_000)
            .single()
            .unwrap()
            .to_rfc3339();
        let created = scheduler
            .add_scheduled(&json!({
                "mode":"once","prompt":"x","runAt":run_at,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let id = created["task"]["id"].as_str().unwrap().to_string();
        clock.0.store(base + 60_000, Ordering::SeqCst);
        assert_eq!(
            scheduler.inner.process_due().await.unwrap(),
            vec![id.clone()]
        );
        for _ in 0..100 {
            if !scheduler
                .inner
                .context
                .active
                .lock()
                .await
                .contains_key(&id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let tasks = scheduler.list_scheduled().await.unwrap();
        let task = tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        assert_eq!(task["executionStatus"], json!("failed"));
        assert_eq!(task["lastResult"], json!("error"));
        assert!(task["lastError"]
            .as_str()
            .unwrap()
            .contains("回执持久化失败"));
        assert!(task["lastReceiptPath"].is_null());
    }

    #[tokio::test]
    async fn global_stop_cancels_then_start_reschedules_enabled_long_cycle() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("global-stop.db")).unwrap();
        let executor = Arc::new(CancellationExecutor {
            started: Notify::new(),
            cancelled: AtomicUsize::new(0),
        });
        let scheduler = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            executor.clone(),
            Arc::new(ManualClock(AtomicI64::new(1_800_000_000_000))),
            temp.path().join("receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let created = scheduler
            .add_long_cycle(&json!({
                "objective":"global pause","stepPrompt":"wait","intervalMinutes":5,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let id = created["task"]["id"].as_str().unwrap().to_string();
        scheduler
            .run_long_cycle_now(&json!({"taskId":id}))
            .await
            .unwrap();
        executor.started.notified().await;
        scheduler.stop_runner().await.unwrap();
        assert_eq!(executor.cancelled.load(Ordering::SeqCst), 1);
        scheduler.start_runner().await.unwrap();
        let tasks = scheduler.list_long_cycle().await.unwrap();
        let task = tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        assert_eq!(task["enabled"], json!(true));
        assert_eq!(task["status"], json!("running"));
        assert_eq!(task["executionStatus"], json!("scheduled"));
        assert!(task["nextRunAt"].is_string());
        assert_eq!(task["completedRounds"], json!(0));
    }

    #[tokio::test]
    async fn config_stop_is_atomic_and_concurrency_limit_includes_active_tasks() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("config-stop.db")).unwrap();
        let base = 1_800_000_000_000i64;
        let clock = Arc::new(ManualClock(AtomicI64::new(base)));
        let executor = Arc::new(CancellationExecutor {
            started: Notify::new(),
            cancelled: AtomicUsize::new(0),
        });
        let scheduler = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            executor.clone(),
            clock.clone(),
            temp.path().join("receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        scheduler
            .set_config(&json!({"maxAutomationPerTick":1}))
            .await
            .unwrap();
        let first = scheduler
            .add_long_cycle(&json!({
                "objective":"first","stepPrompt":"wait","intervalMinutes":1,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let second = scheduler
            .add_long_cycle(&json!({
                "objective":"second","stepPrompt":"wait","intervalMinutes":1,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let first_id = first["task"]["id"].as_str().unwrap().to_string();
        let second_id = second["task"]["id"].as_str().unwrap().to_string();

        scheduler
            .run_long_cycle_now(&json!({"taskId":first_id}))
            .await
            .unwrap();
        executor.started.notified().await;
        let running = scheduler.list_long_cycle().await.unwrap();
        let running_task = running["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == first_id)
            .unwrap();
        assert_eq!(running_task["executionStatus"], json!("running"));
        assert!(running_task["nextRunAt"].is_null());

        clock.0.store(base + 60_000, Ordering::SeqCst);
        assert!(scheduler.inner.process_due().await.unwrap().is_empty());
        assert!(scheduler
            .run_long_cycle_now(&json!({"taskId":second_id}))
            .await
            .unwrap_err()
            .to_string()
            .contains("并发上限"));

        scheduler
            .set_config(&json!({"enabled":false}))
            .await
            .unwrap();
        assert_eq!(executor.cancelled.load(Ordering::SeqCst), 1);
        let stopped = scheduler.status().await.unwrap();
        assert_eq!(stopped["enabled"], json!(false));
        assert!(scheduler.inner.process_due().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn second_scheduler_is_passive_and_cannot_recover_or_reclaim_live_work() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("lease.db")).unwrap();
        let clock = Arc::new(ManualClock(AtomicI64::new(1_800_000_000_000)));
        let first_executor = Arc::new(CancellationExecutor {
            started: Notify::new(),
            cancelled: AtomicUsize::new(0),
        });
        let first = RedClawScheduler::start_with(
            db.clone(),
            Arc::new(NoopEmitter),
            first_executor.clone(),
            clock.clone(),
            temp.path().join("first-receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let created = first
            .add_long_cycle(&json!({
                "objective":"single owner","stepPrompt":"wait","intervalMinutes":5,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let id = created["task"]["id"].as_str().unwrap().to_string();
        first
            .run_long_cycle_now(&json!({"taskId":id}))
            .await
            .unwrap();
        first_executor.started.notified().await;

        let second_executor = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
        });
        let second = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            second_executor.clone(),
            clock,
            temp.path().join("second-receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let status = second.status().await.unwrap();
        assert_eq!(status["lockState"], json!("passive"));
        assert!(!status["blockedBy"].is_null());
        let tasks = second.list_long_cycle().await.unwrap();
        let task = tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        assert_eq!(task["executionStatus"], json!("running"));
        assert!(second
            .run_long_cycle_now(&json!({"taskId":id}))
            .await
            .is_err());
        assert!(second.inner.process_due().await.unwrap().is_empty());
        assert_eq!(second_executor.calls.load(Ordering::SeqCst), 0);

        // 重复启动 owner 不能把 running 行写回 scheduled。
        first.start_runner().await.unwrap();
        let still_running = first.list_long_cycle().await.unwrap();
        assert_eq!(
            still_running["tasks"][0]["executionStatus"],
            json!("running")
        );
        first
            .set_long_cycle_enabled(&json!({"taskId":id,"enabled":false}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn status_read_cannot_suppress_active_cancellation_after_lease_takeover() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("lease-takeover.db")).unwrap();
        let base = 1_800_000_000_000i64;
        let clock = Arc::new(ManualClock(AtomicI64::new(base)));
        let executor = Arc::new(CancellationExecutor {
            started: Notify::new(),
            cancelled: AtomicUsize::new(0),
        });
        let first = RedClawScheduler::start_with(
            db.clone(),
            Arc::new(NoopEmitter),
            executor.clone(),
            clock.clone(),
            temp.path().join("first"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let created = first
            .add_long_cycle(&json!({
                "objective":"lease takeover","stepPrompt":"wait","intervalMinutes":5,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let id = created["task"]["id"].as_str().unwrap().to_string();
        first
            .run_long_cycle_now(&json!({"taskId":id}))
            .await
            .unwrap();
        executor.started.notified().await;

        clock.0.store(base + LEASE_TTL_MS + 1, Ordering::SeqCst);
        let second = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            Arc::new(MockExecutor {
                calls: AtomicUsize::new(0),
            }),
            clock,
            temp.path().join("second"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        assert_eq!(second.status().await.unwrap()["lockState"], json!("owner"));
        assert_eq!(first.status().await.unwrap()["lockState"], json!("passive"));

        // status() is read-only: ensure_lease still observes the owned->lost edge
        // and cancels the old Agent instead of leaving two external executions alive.
        assert!(first.inner.process_due().await.unwrap().is_empty());
        for _ in 0..100 {
            if executor.cancelled.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(executor.cancelled.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_execution_completion_cannot_overwrite_a_new_claim_owner() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("stale-completion.db")).unwrap();
        let base = 1_800_000_000_000i64;
        let clock = Arc::new(ManualClock(AtomicI64::new(base)));
        let scheduler = RedClawScheduler::start_with(
            db.clone(),
            Arc::new(NoopEmitter),
            Arc::new(MockExecutor {
                calls: AtomicUsize::new(0),
            }),
            clock,
            temp.path().join("receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let created = scheduler
            .add_long_cycle(&json!({
                "objective":"CAS","stepPrompt":"write","intervalMinutes":5,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        let id = created["task"]["id"].as_str().unwrap().to_string();
        // Production claims are always protected until the task is registered as active.
        // Keep this synthetic, unregistered claim hidden from the orphan-recovery scan too.
        let _control = scheduler.inner.context.control.lock().await;
        let claimed = claim_task(
            db.clone(),
            id.clone(),
            TaskKind::LongCycle,
            base,
            true,
            &scheduler.inner.context.instance_id,
        )
        .await
        .unwrap()
        .unwrap();
        db.execute_json(
            "UPDATE agent_tasks SET owner_session_id='new-execution' WHERE id=?1",
            &[json!(id)],
        )
        .unwrap();
        let result = complete_task(
            db,
            claimed,
            &scheduler.inner.context.instance_id,
            "succeeded",
            "old result".into(),
            None,
            None,
            base,
        )
        .await;
        assert!(result.is_err());
        let tasks = scheduler.list_long_cycle().await.unwrap();
        assert_eq!(tasks["tasks"][0]["completedRounds"], json!(0));
        assert!(tasks["tasks"][0]["lastResult"].is_null());
    }

    #[test]
    fn local_daily_and_weekly_next_run_are_strictly_future() {
        let now = system_now_ms();
        let task = ScheduledTask {
            id: "t".into(),
            name: "daily".into(),
            enabled: true,
            mode: "daily".into(),
            prompt: "x".into(),
            project_id: None,
            work_item_id: None,
            subagent_roles: Vec::new(),
            required_tools: Vec::new(),
            allowed_tools: Vec::new(),
            authorized_tools: Vec::new(),
            interval_minutes: None,
            time: Some("09:00".into()),
            weekdays: Vec::new(),
            run_at: None,
            created_at: now,
            updated_at: now,
            last_run_at: None,
            last_result: None,
            last_output: None,
            last_error: None,
            next_run_at: None,
            last_receipt_path: None,
        };
        assert!(compute_next_scheduled(&task, now).unwrap() > now);
        let mut weekly = task;
        weekly.mode = "weekly".into();
        weekly.weekdays = vec![0, 6];
        assert!(compute_next_scheduled(&weekly, now).unwrap() > now);
    }

    #[test]
    fn nested_tool_failures_cannot_be_recorded_as_success() {
        assert!(contains_explicit_tool_failure(&json!({
            "content":[{"type":"text","text":"{\"persisted\":false}"}]
        })));
        assert!(!contains_explicit_tool_failure(&json!({
            "content":[{"type":"text","text":"{\"persisted\":true}"}]
        })));
        assert!(contains_explicit_tool_failure(&json!({
            "content":"{\"dry_run\":true,\"plannedAction\":{}}"
        })));
        assert!(contains_explicit_tool_failure(&json!({"isError":true})));
        assert!(contains_explicit_tool_failure(&json!({"ok":false})));
        assert!(contains_explicit_tool_failure(&json!({"status":"failed"})));
        assert!(contains_disallowed_tool_input(&json!({"dry_run":true})));
        let tools = vec![ToolEvidence {
            call_id: "1".into(),
            name: "yunying-ops__create_note".into(),
            input: Value::Null,
            output: Some(json!({"persisted":true})),
        }];
        assert!(missing_required_tools(&["create_note".into()], &tools).is_empty());
        assert_eq!(
            missing_required_tools(&["generate_image".into()], &tools),
            vec!["generate_image"]
        );
        assert_eq!(
            missing_required_tools(&["not_create_note".into()], &tools),
            vec!["not_create_note"]
        );
        let impersonating = vec![ToolEvidence {
            call_id: "2".into(),
            name: "not_create_note".into(),
            input: Value::Null,
            output: Some(json!({"persisted":true})),
        }];
        assert_eq!(
            missing_required_tools(&["create_note".into()], &impersonating),
            vec!["create_note"]
        );
        assert!(unverified_required_tools(&["create_note".into()], &tools).is_empty());
        for name in [
            "openai_image",
            "google_imagen",
            "recraft_image",
            "elevenlabs_tts",
            "suno_music",
            "talking_head",
            "video_selector",
        ] {
            assert!(
                requires_explicit_authorization(name),
                "{name} must require explicit authorization"
            );
        }
        assert!(!requires_explicit_authorization("create_note"));
    }

    #[tokio::test]
    async fn status_never_claims_unimplemented_heartbeat_or_process_exit_execution() {
        let db = Db::open_in_memory().unwrap();
        let scheduler = RedClawScheduler::start_with(
            db,
            Arc::new(NoopEmitter),
            Arc::new(MockExecutor {
                calls: AtomicUsize::new(0),
            }),
            Arc::new(ManualClock(AtomicI64::new(1_800_000_000_000))),
            tempfile::tempdir().unwrap().path().join("receipts"),
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
        let status = scheduler.status().await.unwrap();
        assert_eq!(status["capabilities"]["automaticExecution"], json!(true));
        assert_eq!(status["capabilities"]["heartbeatExecution"], json!(false));
        assert_eq!(status["capabilities"]["survivesAppExit"], json!(false));
        assert_eq!(status["heartbeat"]["enabled"], json!(false));
        assert_eq!(status["keepAliveWhenNoWindow"], json!(false));
        assert!(scheduler
            .set_config(&json!({"heartbeatEnabled":true}))
            .await
            .is_err());
        assert!(scheduler
            .set_config(&json!({"keepAliveWhenNoWindow":true}))
            .await
            .is_err());
        let daily_long = scheduler
            .add_long_cycle(&json!({
                "objective":"daily","stepPrompt":"run","intervalMinutes":1440,"totalRounds":2,
                "requiredTools":["create_note"],"allowedTools":["create_note"]
            }))
            .await
            .unwrap();
        assert_eq!(daily_long["task"]["intervalMinutes"], json!(1440));
    }
}
