//! `devtools` 命名空间 IPC 通道：开发工具（技能管理、工具诊断、运行时钩子、浏览器扩展、CLI 运行时）。
//!
//! 对应 Beav `desktop/electron/main.ts` 中前缀为 `skills:` / `tools:diagnostics:` /
//! `tools:hooks:` / `plugin:` / `cli-runtime:` 的 `ipcMain.handle`。由 [`super::dispatch_invoke`]
//! 按命名空间前缀路由到这里（需在 `mod.rs` 把这些前缀加到 `devtools::invoke` 分支）。
//!
//! 设计取舍：
//! - **skills** → 文件系统。根目录 = `settings.workspace_dir/spaces/<active_space_id>/skills`
//!   （对齐 Beav `getWorkspacePaths().skills`，缺省 `~/.redconvert/spaces/default/skills`）。
//!   扫描顶层 `*.md` 与子目录 `*/SKILL.md`（SKILL.md 格式）；frontmatter 解析 `name`/`description`/`disabled`；
//!   enable/disable 改写 frontmatter 的 `disabled` 字段（非 Beav 的 `~/.redconvert/skill-settings.json`，
//!   以 frontmatter 为单一事实来源，便于随技能文件迁移）。
//! - **market**（market-search / market-install / install-from-github）→ 真实实现走 `clawhub.ai` 网络。
//!   Rust 壳未引入 HTTP 客户端（`reqwest`），当前为结构完整的占位（空结果 / stub），列在
//!   [`super`] 的 `stub_channels` 说明里，待接入网络层。
//! - **tools:diagnostics** → 内存态占位。真实诊断需工具注册表（`toolRegistry` + builtin tools），
//!   Rust 壳尚未移植；`list` 返回空数组，`run-direct`/`run-ai` 返回结构完整的 stub 结果。
//! - **tools:hooks** → 内存 `Vec` 占位。真实钩子是 agent 工具循环拦截（`executeRuntimeHooks`），
//!   本命名空间仅做 CRUD 存储；执行未接入（标 stub）。
//! - **plugin** → 文件系统。`browser-extension-status` 探测打包/导出目录；`prepare` 拷贝打包目录到
//!   导出目录；`open` 用 `webbrowser` 打开导出目录。Rust 壳未打包浏览器扩展，故 `bundled` 通常为 false。
//! - **cli-runtime** → 内存态占位。真实实现需 PTY 子进程（`portable-pty`），未接入；全部返回 stub，
//!   依赖子进程的通道列在 `stub_channels`。
//! - 写/真实副作用默认尊重 `payload.dryRun`（=`true`）或 `payload.confirm`（=`false`）。
//! - 时间戳统一 `std::time::SystemTime` 毫秒。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::AppState;

// ---------------------------------------------------------------------------
// 内存态占位（hooks / cli-runtime）
// ---------------------------------------------------------------------------

/// `std::sync::MutexGuard` 别名（统一生命周期标注）。
type MutexGuard<'a, T> = std::sync::MutexGuard<'a, T>;

/// 运行时钩子内存存储（对齐 Beav `core/runtimeHooks.ts` 的 `hooks: Map`，简化为 `Vec`）。
static HOOKS: Mutex<Option<Vec<Value>>> = Mutex::new(None);

/// CLI 运行时环境内存存储（stub）。
static CLI_ENVS: Mutex<Option<Vec<Value>>> = Mutex::new(None);

/// CLI 运行时执行记录内存存储（stub）。
static CLI_RUNS: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);

fn hooks_store() -> MutexGuard<'static, Option<Vec<Value>>> {
    let mut g = HOOKS.lock().unwrap();
    if g.is_none() {
        *g = Some(Vec::new());
    }
    g
}

fn cli_envs_store() -> MutexGuard<'static, Option<Vec<Value>>> {
    let mut g = CLI_ENVS.lock().unwrap();
    if g.is_none() {
        *g = Some(Vec::new());
    }
    g
}

fn cli_runs_store() -> MutexGuard<'static, Option<HashMap<String, Value>>> {
    let mut g = CLI_RUNS.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

// ---------------------------------------------------------------------------
// invoke 分发
// ---------------------------------------------------------------------------

/// `devtools` 命名空间的双向通道。按通道全名 `match`；未知通道返回 `Err`。
///
/// 本命名空间无单向（`send`）通道，故不提供 `send`。触发类（打开扩展目录）也走 `invoke`，
/// 与 Beav 一致。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        // ===================== skills =====================
        "skills:list" => {
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let dir = skills_dir(&settings);
            Ok(json!(discover_skills_with_plugins(&dir)))
        }

        "skills:import-local" => {
            let source = payload
                .get("sourcePath")
                .or_else(|| payload.get("source_path"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            if source.is_empty() {
                return Ok(json!({
                    "success": false,
                    "reason": "native_dialog_required",
                    "error": "Tauri import requires sourcePath until the native directory dialog is wired",
                }));
            }
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let destination_root = skills_dir(&settings);
            match import_skill_directory(Path::new(source), &destination_root) {
                Ok((location, generated_entry)) => Ok(json!({
                    "success": true,
                    "canceled": false,
                    "location": location.to_string_lossy(),
                    "generatedEntry": generated_entry,
                })),
                Err(error) => Ok(json!({ "success": false, "error": error.to_string() })),
            }
        }

        "skills:save" => {
            let location = payload
                .get("location")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let content = payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if location.is_empty() {
                return Ok(json!({ "success": false, "error": "location is required" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "location": location }));
            }
            match write_skill_file(&location, &content) {
                Ok(()) => Ok(json!({ "success": true })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        "skills:create" => {
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let dir = skills_dir(&settings);
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Ok(json!({ "success": false, "error": "name is required" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({
                    "success": true,
                    "dryRun": true,
                    "location": dir.join(sanitize_skill_name(&name)).join("SKILL.md").to_string_lossy(),
                }));
            }
            match create_skill(&dir, &name) {
                Ok(loc) => Ok(json!({ "success": true, "location": loc.to_string_lossy() })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        "skills:enable" | "skills:disable" => {
            let enable = channel == "skills:enable";
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Ok(json!({ "success": false, "error": "name is required" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "changed": false }));
            }
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let dir = skills_dir(&settings);
            match toggle_skill(&dir, &name, enable) {
                Ok(changed) => Ok(json!({ "success": true, "changed": changed })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        // ===================== installable plugins =====================
        "plugins:list" => match crate::plugins::list_installed_plugins() {
            Ok(plugins) => Ok(json!({
                "success": true,
                "plugins": plugins,
                "pluginHome": crate::plugins::plugin_install_root(),
            })),
            Err(error) => {
                Ok(json!({ "success": false, "plugins": [], "error": error.to_string() }))
            }
        },

        "plugins:import-local" => {
            let source = payload
                .get("sourcePath")
                .or_else(|| payload.get("source_path"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if source.is_empty() {
                return Ok(json!({ "success": false, "error": "sourcePath is required" }));
            }
            match crate::plugins::install_plugin_directory(Path::new(source), "installed") {
                Ok(result) => Ok(json!({ "success": true, "canceled": false, "plugin": result })),
                Err(error) => Ok(json!({ "success": false, "error": error.to_string() })),
            }
        }

        "plugins:sync-builtins" => match crate::plugins::sync_builtin_plugins() {
            Ok(results) => Ok(json!({
                "success": true,
                "plugins": results,
                "pluginHome": crate::plugins::plugin_install_root(),
            })),
            Err(error) => Ok(json!({ "success": false, "error": error.to_string() })),
        },

        "plugins:remove" => {
            let name = payload
                .get("name")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                return Ok(json!({ "success": false, "error": "name is required" }));
            }
            match crate::plugins::remove_installed_plugin(name) {
                Ok(removed) => Ok(json!({ "success": true, "removed": removed })),
                Err(error) => Ok(json!({ "success": false, "error": error.to_string() })),
            }
        }

        "plugins:get-settings" => {
            let name = payload
                .get("name")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                return Ok(json!({ "success": false, "error": "id is required" }));
            }
            match crate::plugins::read_plugin_settings(name) {
                Ok(settings) => Ok(json!({ "success": true, "settings": settings })),
                Err(error) => Ok(json!({ "success": false, "error": error.to_string() })),
            }
        }

        "plugins:save-settings" => {
            let name = payload
                .get("name")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                return Ok(json!({ "success": false, "error": "id is required" }));
            }
            let values = payload.get("values").cloned().unwrap_or_else(|| json!({}));
            let clear_secret_keys = payload
                .get("clearSecretKeys")
                .or_else(|| payload.get("clear_secret_keys"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            match crate::plugins::save_plugin_settings(name, &values, &clear_secret_keys) {
                Ok(settings) => Ok(json!({ "success": true, "settings": settings })),
                Err(error) => Ok(json!({ "success": false, "error": error.to_string() })),
            }
        }

        "plugins:get-root" | "plugins:open-root" => {
            let root = crate::plugins::plugin_install_root();
            if let Err(error) = std::fs::create_dir_all(&root) {
                return Ok(json!({ "success": false, "error": error.to_string() }));
            }
            Ok(json!({ "success": true, "path": root }))
        }

        // market-* / install-from-github → 网络占位（clawhub.ai），待接入 HTTP 客户端。
        "skills:market-search" => Ok(json!([])),
        "skills:market-install" | "skills:install-from-github" => Ok(json!({
            "success": false,
            "reason": "skill_market_network_pending",
            "error": "skill market install requires HTTP client (clawhub.ai); not wired",
        })),

        // ===================== tools:diagnostics =====================
        // 内存态占位：真实诊断需工具注册表（builtin tools + ToolRegistry），Rust 壳尚未移植。
        "tools:diagnostics:list" => Ok(json!([])),

        "tools:diagnostics:run-direct" => {
            let tool_name = payload
                .get("toolName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if tool_name.is_empty() {
                return Ok(json!({
                    "success": false, "mode": "direct", "toolName": "",
                    "error": "toolName is required"
                }));
            }
            Ok(json!({
                "success": false, "mode": "direct", "toolName": tool_name,
                "request": Value::Null,
                "error": "tool diagnostics registry not ported (stub)",
                "executionSucceeded": false
            }))
        }

        "tools:diagnostics:run-ai" => {
            let tool_name = payload
                .get("toolName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if tool_name.is_empty() {
                return Ok(json!({
                    "success": false, "mode": "ai", "toolName": "",
                    "error": "toolName is required"
                }));
            }
            Ok(json!({
                "success": false, "mode": "ai", "toolName": tool_name,
                "request": Value::Null,
                "error": "tool diagnostics registry + LLM call not wired (stub)",
                "toolCallReturned": false,
                "toolNameMatched": false,
                "argumentsParsed": false,
                "executionSucceeded": false
            }))
        }

        // ===================== tools:hooks =====================
        // 内存 Vec 占位：CRUD 功能可用，真实钩子执行（agent 工具循环拦截）未接入。
        "tools:hooks:list" => {
            let list = hooks_store().as_ref().unwrap().clone();
            Ok(json!(list))
        }

        "tools:hooks:register" => {
            let id = payload
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("hook_{}", now_ts()));
            let event = payload
                .get("event")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let hook_type = payload
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if event.is_empty() || hook_type.is_empty() {
                return Ok(json!({ "success": false, "error": "id, event and type are required" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "hook": { "id": id } }));
            }
            let hook = build_hook(&payload, &id, &event, &hook_type);
            let mut store = hooks_store();
            let vec = store.as_mut().unwrap();
            if let Some(existing) = vec
                .iter_mut()
                .find(|h| h.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
            {
                *existing = hook.clone();
            } else {
                vec.push(hook.clone());
            }
            Ok(json!({ "success": true, "hook": hook }))
        }

        "tools:hooks:remove" => {
            let id = payload
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if id.is_empty() {
                return Ok(json!({ "success": false, "error": "id is required" }));
            }
            let mut store = hooks_store();
            let vec = store.as_mut().unwrap();
            let before = vec.len();
            vec.retain(|h| h.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
            Ok(json!({ "success": true, "removed": vec.len() < before }))
        }

        // ===================== plugin =====================
        "plugin:browser-extension-status" => {
            let bundled = bundled_plugin_dir();
            let export_path = exported_plugin_dir();
            Ok(json!({
                "success": true,
                "bundled": bundled.is_some(),
                "bundledPath": bundled
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "exportPath": export_path.to_string_lossy(),
                "exported": export_path.is_dir(),
            }))
        }

        "plugin:prepare-browser-extension" => {
            let export_path = exported_plugin_dir();
            if is_dry_run(&payload) {
                return Ok(json!({
                    "success": true,
                    "dryRun": true,
                    "path": export_path.to_string_lossy(),
                }));
            }
            // 真实实现：拷贝打包目录 → 导出目录。Rust 壳未打包浏览器扩展资源。
            match bundled_plugin_dir() {
                Some(src) => match copy_dir_recursive(&src, &export_path) {
                    Ok(()) => Ok(json!({
                        "success": true,
                        "path": export_path.to_string_lossy(),
                        "alreadyPrepared": false,
                    })),
                    Err(e) => Ok(json!({
                        "success": false,
                        "path": export_path.to_string_lossy(),
                        "error": e.to_string(),
                    })),
                },
                None => Ok(json!({
                    "success": false,
                    "path": "",
                    "reason": "bundled_plugin_missing",
                    "error": "内置插件资源不存在（Rust 壳未打包浏览器扩展）",
                })),
            }
        }

        "plugin:open-browser-extension-dir" => {
            let export_path = exported_plugin_dir();
            if is_dry_run(&payload) {
                return Ok(json!({
                    "success": true,
                    "dryRun": true,
                    "path": export_path.to_string_lossy(),
                }));
            }
            let _ = std::fs::create_dir_all(&export_path);
            match webbrowser::open(&export_path.to_string_lossy()) {
                Ok(()) => Ok(json!({ "success": true, "path": export_path.to_string_lossy() })),
                Err(e) => Ok(json!({
                    "success": false,
                    "path": export_path.to_string_lossy(),
                    "error": e.to_string(),
                })),
            }
        }

        // ===================== cli-runtime =====================
        // 内存态占位：真实实现需 PTY 子进程（portable-pty），未接入。全部 stub。
        "cli-runtime:detect" => Ok(json!({
            "success": true,
            "detected": false,
            "runtimes": [],
            "reason": "pty_subprocess_not_wired",
        })),

        "cli-runtime:discover" => Ok(json!({ "success": true, "entries": [] })),

        "cli-runtime:list-tools" => {
            let env_id = payload
                .get("environmentId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({ "success": true, "environmentId": env_id, "tools": [] }))
        }

        "cli-runtime:inspect" => {
            let env_id = payload
                .get("environmentId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({
                "success": false,
                "environmentId": env_id,
                "reason": "pty_subprocess_not_wired",
                "error": "inspect requires real PTY subprocess",
                "info": Value::Null,
            }))
        }

        "cli-runtime:list-environments" => {
            let envs = cli_envs_store().as_ref().unwrap().clone();
            Ok(json!({ "success": true, "environments": envs }))
        }

        "cli-runtime:create-environment" => {
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Ok(json!({ "success": false, "error": "name is required" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({
                    "success": true,
                    "dryRun": true,
                    "environmentId": format!("env-{}", now_ts()),
                    "name": name,
                }));
            }
            let id = format!("env-{}", now_ts());
            let env = json!({
                "id": id,
                "name": name,
                "createdAt": now_ts(),
                "status": "created-stub",
            });
            cli_envs_store().as_mut().unwrap().push(env.clone());
            Ok(json!({ "success": true, "environmentId": id, "environment": env }))
        }

        "cli-runtime:install" => {
            let env_id = payload
                .get("environmentId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({
                "success": false,
                "environmentId": env_id,
                "reason": "pty_subprocess_not_wired",
                "error": "install requires real PTY subprocess",
            }))
        }

        "cli-runtime:execute" => {
            let env_id = payload
                .get("environmentId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let command = payload
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if is_dry_run(&payload) {
                return Ok(json!({
                    "success": true,
                    "dryRun": true,
                    "environmentId": env_id,
                    "executionId": format!("exec-{}", now_ts()),
                }));
            }
            let exec_id = format!("exec-{}", now_ts());
            let run = json!({
                "id": exec_id,
                "environmentId": env_id,
                "command": command,
                "status": "running-stub",
                "startedAt": now_ts(),
                "finishedAt": Value::Null,
                "stdout": "",
                "stderr": "",
                "exitCode": Value::Null,
            });
            cli_runs_store()
                .as_mut()
                .unwrap()
                .insert(exec_id.clone(), run.clone());
            Ok(json!({
                "success": true,
                "executionId": exec_id,
                "run": run,
                "reason": "pty_subprocess_not_wired",
            }))
        }

        "cli-runtime:cancel-execution" => {
            let exec_id = payload
                .get("executionId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            {
                let mut store = cli_runs_store();
                if let Some(run) = store.as_mut().unwrap().get_mut(&exec_id) {
                    run["status"] = json!("cancelled");
                    run["finishedAt"] = json!(now_ts());
                }
            }
            Ok(json!({ "success": true, "executionId": exec_id }))
        }

        "cli-runtime:poll-execution" => {
            let exec_id = payload
                .get("executionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let run = cli_runs_store()
                .as_ref()
                .unwrap()
                .get(exec_id)
                .cloned()
                .unwrap_or_else(|| json!({ "id": exec_id, "status": "unknown" }));
            Ok(json!({ "success": true, "run": run }))
        }

        "cli-runtime:verify" => {
            let env_id = payload
                .get("environmentId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({
                "success": false,
                "environmentId": env_id,
                "ok": false,
                "reason": "pty_subprocess_not_wired",
            }))
        }

        "cli-runtime:approve-escalation" | "cli-runtime:deny-escalation" => {
            let approved = channel.ends_with("approve-escalation");
            let exec_id = payload
                .get("executionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({
                "success": true,
                "executionId": exec_id,
                "approved": approved,
                "reason": "stub",
            }))
        }

        other => Err(anyhow::anyhow!("devtools 通道未实现: {other}")),
    }
}

// ---------------------------------------------------------------------------
// skills：文件系统 + frontmatter
// ---------------------------------------------------------------------------

/// 工作区根目录 = `settings.workspace_dir`（缺省 `~/.redconvert`）。
fn workspace_root(settings: &Value) -> PathBuf {
    settings
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".redconvert"))
}

/// 当前激活空间 id（缺省 `default`，对齐 Beav `DEFAULT_SPACE_ID`）。
fn active_space_id(settings: &Value) -> String {
    settings
        .get("active_space_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("default")
        .to_string()
}

/// 当前空间基目录 = `<workspace_root>/spaces/<active_space_id>`（简化版 `resolveSpaceBaseDir`，
/// 不处理 default 空间的 legacy 回退）。
fn space_base(settings: &Value) -> PathBuf {
    workspace_root(settings)
        .join("spaces")
        .join(active_space_id(settings))
}

/// 技能目录 = `<space_base>/skills`（对齐 Beav `getWorkspacePaths().skills`）。
fn skills_dir(settings: &Value) -> PathBuf {
    space_base(settings).join("skills")
}

/// 发现技能目录下所有技能：顶层 `*.md` 与子目录 `*/SKILL.md`。返回按 name 排序的数组。
/// 每项含 `name`/`description`/`location`/`baseDir`/`body`/`sourceScope`/`disabled`（对齐 `SkillDefinition`）。
fn discover_skills(dir: &Path) -> Vec<Value> {
    discover_skills_scoped(dir, "workspace", false)
}

fn discover_skills_scoped(dir: &Path, source_scope: &str, is_builtin: bool) -> Vec<Value> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let path = entry.path();
        if ft.is_file() {
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Some(skill) =
                    parse_skill_file_with_scope(&path, &path, source_scope, is_builtin)
                {
                    out.push(skill);
                }
            }
        } else if ft.is_dir() {
            let skill_md = path.join("SKILL.md");
            if skill_md.is_file() {
                if let Some(skill) =
                    parse_skill_file_with_scope(&skill_md, &path, source_scope, is_builtin)
                {
                    out.push(skill);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        an.cmp(bn)
    });
    out
}

/// 合并已安装/内置插件 skills 与当前空间技能；同名时 workspace 覆盖插件。
fn discover_skills_with_plugins(dir: &Path) -> Vec<Value> {
    let mut by_name = HashMap::<String, Value>::new();
    for skill in discover_plugin_skills() {
        let key = skill
            .get("name")
            .and_then(Value::as_str)
            .map(normalize_skill_key)
            .unwrap_or_default();
        by_name.insert(key, skill);
    }
    for skill in discover_skills(dir) {
        let key = skill
            .get("name")
            .and_then(Value::as_str)
            .map(normalize_skill_key)
            .unwrap_or_default();
        by_name.insert(key, skill);
    }
    let mut out = by_name.into_values().collect::<Vec<_>>();
    out.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(right.get("name").and_then(Value::as_str).unwrap_or(""))
    });
    out
}

fn discover_plugin_skills() -> Vec<Value> {
    let mut out = Vec::new();
    let mut seen_plugins = std::collections::HashSet::new();
    for plugin_root in crate::plugins::all_plugin_roots() {
        if !plugin_root.is_dir() || !seen_plugins.insert(plugin_root.clone()) {
            continue;
        }
        for skills_dir in [
            plugin_root.join("skills"),
            plugin_root.join(".agents").join("skills"),
        ] {
            out.extend(discover_skills_scoped(&skills_dir, "plugin", true));
        }
    }
    out
}

/// 解析单个技能文件：抽取 frontmatter 的 `name`/`description`/`disabled`，body 为正文。
fn parse_skill_file(file_path: &Path, base_dir: &Path) -> Option<Value> {
    parse_skill_file_with_scope(file_path, base_dir, "workspace", false)
}

fn parse_skill_file_with_scope(
    file_path: &Path,
    base_dir: &Path,
    source_scope: &str,
    is_builtin: bool,
) -> Option<Value> {
    let content = std::fs::read_to_string(file_path).ok()?;
    let (fm, body) = split_frontmatter(&content);
    let fm = fm.unwrap_or_default();
    let name = fm_get(&fm, "name")
        .filter(|s| !s.is_empty())
        .or_else(|| infer_name_from_path(file_path))
        .unwrap_or_default();
    let description = fm_get(&fm, "description")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| infer_description_from_body(&body));
    let disabled = fm_get(&fm, "disabled")
        .map(|v| v.trim() == "true")
        .unwrap_or(false);
    Some(json!({
        "name": name,
        "description": description,
        "location": file_path.to_string_lossy(),
        "baseDir": base_dir.to_string_lossy(),
        "body": body,
        "sourceScope": source_scope,
        "isBuiltin": is_builtin,
        "disabled": disabled,
    }))
}

/// 导入技能目录；若目录没有 `SKILL.md`，但包含 Markdown，则生成兼容入口。
fn import_skill_directory(source: &Path, skills_root: &Path) -> anyhow::Result<(PathBuf, bool)> {
    if !source.is_dir() {
        anyhow::bail!("sourcePath is not a directory: {}", source.display());
    }
    std::fs::create_dir_all(skills_root)?;
    let source = source.canonicalize()?;
    let skills_root = skills_root.canonicalize()?;
    if skills_root == source || skills_root.starts_with(&source) {
        anyhow::bail!("sourcePath cannot contain the workspace skills directory");
    }
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("imported-skill");
    let base_name = sanitize_skill_name(source_name);
    let mut destination = skills_root.join(&base_name);
    let mut suffix = 1usize;
    while destination.exists() {
        destination = skills_root.join(format!("{base_name}-{suffix}"));
        suffix += 1;
    }
    copy_dir_recursive(&source, &destination)?;

    let mut has_skill_entry = false;
    let mut has_markdown = false;
    scan_skill_pack(&destination, &mut has_skill_entry, &mut has_markdown)?;
    if has_skill_entry {
        return Ok((destination, false));
    }
    if !has_markdown {
        let _ = std::fs::remove_dir_all(&destination);
        anyhow::bail!("selected directory contains no SKILL.md or Markdown documents");
    }

    let is_open_montage = source
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("openmontage")
        || destination.join("pipeline_defs").is_dir()
        || destination.join("INDEX.md").is_file();
    let skill_name = if is_open_montage {
        "openmontage-video-production".to_string()
    } else {
        base_name
    };
    let wrapper = format!(
        "---\nname: {skill_name}\ndescription: 从 {source_name} 导入的技能包。按需读取包内 Markdown。\n---\n\n# {source_name}\n\n先读取 INDEX.md 或 README.md，再按任务需要读取子目录文档。所有相对路径均以本目录为基准。\n"
    );
    std::fs::write(destination.join("SKILL.md"), wrapper)?;
    Ok((destination, true))
}

fn scan_skill_pack(
    root: &Path,
    has_skill_entry: &mut bool,
    has_markdown: &mut bool,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" || name == "dist" {
                continue;
            }
            scan_skill_pack(&path, has_skill_entry, has_markdown)?;
        } else if file_type.is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("SKILL.md") {
                *has_skill_entry = true;
            }
            if name.to_ascii_lowercase().ends_with(".md") {
                *has_markdown = true;
            }
        }
    }
    Ok(())
}

/// 写技能文件（任意路径，对齐 Beav `skills:save` 的 `fs.writeFile(location, content)`）。
fn write_skill_file(location: &str, content: &str) -> anyhow::Result<()> {
    let path = Path::new(location);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = content.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// 创建新技能：`<skills_dir>/<sanitized>/SKILL.md` + `agents/` 子目录。同名已存在时报错。
fn create_skill(skills_dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let dir_name = sanitize_skill_name(name);
    let skill_dir = skills_dir.join(&dir_name);
    if skill_dir.exists() {
        anyhow::bail!("同名技能已存在");
    }
    let skill_file = skill_dir.join("SKILL.md");
    std::fs::create_dir_all(skill_dir.join("agents"))?;
    let template = format!(
        "---\nname: {dir_name}\ndescription: 请添加技能描述\n---\n\n# {dir_name}\n\n在这里编写技能的详细指令...\n"
    );
    std::fs::write(&skill_file, template)?;
    Ok(skill_file)
}

/// 启用/禁用技能：按 name 查找技能文件，改写 frontmatter 的 `disabled` 字段。返回是否发生变更。
fn toggle_skill(skills_dir: &Path, name: &str, enable: bool) -> anyhow::Result<bool> {
    let target = find_skill_file(skills_dir, name)?;
    let original = std::fs::read_to_string(&target)?;
    let (fm, _) = split_frontmatter(&original);
    let currently_disabled = fm
        .as_deref()
        .and_then(|f| fm_get(f, "disabled"))
        .map(|v| v.trim() == "true")
        .unwrap_or(false);
    let want_disabled = !enable;
    if currently_disabled == want_disabled {
        return Ok(false);
    }
    let updated = set_frontmatter_disabled(&original, want_disabled);
    std::fs::write(&target, updated)?;
    Ok(true)
}

/// 在技能目录中按 name（大小写/空白归一化）查找技能文件路径。
fn find_skill_file(skills_dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let needle = normalize_skill_key(name);
    let entries = std::fs::read_dir(skills_dir)?;
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let path = entry.path();
        if ft.is_file() {
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Some(skill) = parse_skill_file(&path, &path) {
                    if let Some(n) = skill.get("name").and_then(|v| v.as_str()) {
                        if normalize_skill_key(n) == needle {
                            return Ok(path);
                        }
                    }
                }
            }
        } else if ft.is_dir() {
            let skill_md = path.join("SKILL.md");
            if skill_md.is_file() {
                if let Some(skill) = parse_skill_file(&skill_md, &path) {
                    if let Some(n) = skill.get("name").and_then(|v| v.as_str()) {
                        if normalize_skill_key(n) == needle {
                            return Ok(skill_md);
                        }
                    }
                }
            }
        }
    }
    anyhow::bail!("未找到技能: {name}")
}

// ---- frontmatter 解析/改写（无 YAML 依赖，行级处理）----

/// 拆分 markdown 为 `(frontmatter, body)`。frontmatter 为首个 `---`/`...` 闭合块之间的文本（不含分隔符）。
/// 无合法 frontmatter 时返回 `(None, content)`。
fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.is_empty() || lines[0].trim_end_matches('\r').trim() != "---" {
        return (None, content.to_string());
    }
    for i in 1..lines.len() {
        let t = lines[i].trim_end_matches('\r').trim();
        if t == "---" || t == "..." {
            let fm = lines[1..i]
                .iter()
                .map(|l| l.trim_end_matches('\r'))
                .collect::<Vec<_>>()
                .join("\n");
            let body = if i + 1 < lines.len() {
                lines[i + 1..]
                    .iter()
                    .map(|l| l.trim_end_matches('\r'))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim_start_matches('\n')
                    .to_string()
            } else {
                String::new()
            };
            return (Some(fm), body);
        }
    }
    (None, content.to_string())
}

/// 从 frontmatter 文本读取首个 `key: value`（去引号/空白）。key 须精确匹配 `key:` 前缀。
fn fm_get(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in frontmatter.split('\n') {
        let t = line.trim_start().trim_end_matches('\r');
        if let Some(rest) = t.strip_prefix(prefix.as_str()) {
            let val = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            return Some(val.to_string());
        }
    }
    None
}

/// 在 frontmatter 中设置 `disabled` 字段（存在则改值，否则在开分隔符后插入）。无 frontmatter 时新建。
fn set_frontmatter_disabled(content: &str, disabled: bool) -> String {
    let had_trailing_nl = content.ends_with('\n');
    let new_line = format!("disabled: {}", if disabled { "true" } else { "false" });
    let lines: Vec<String> = content
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let has_fm = !lines.is_empty() && lines[0].trim() == "---";
    let close_idx = if has_fm {
        (1..lines.len()).find(|&i| {
            let t = lines[i].trim();
            t == "---" || t == "..."
        })
    } else {
        None
    };

    let mut out_lines: Vec<String> = match close_idx {
        Some(close) => {
            let mut out = lines.clone();
            let existing = (1..close).find(|&i| out[i].trim_start().starts_with("disabled:"));
            match existing {
                Some(idx) => out[idx] = new_line,
                None => out.insert(1, new_line),
            }
            out
        }
        None => {
            let mut out = vec![
                "---".to_string(),
                new_line,
                "---".to_string(),
                String::new(),
            ];
            out.extend(lines.iter().cloned());
            out
        }
    };

    // 去掉 split 带来的末尾空行（若原内容以 \n 结尾，split 末尾会有一个 ""），再按原状补回单换行。
    if out_lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        out_lines.pop();
    }
    let mut out = out_lines.join("\n");
    if had_trailing_nl {
        out.push('\n');
    }
    out
}

/// 从文件路径推断技能名（`SKILL.md` → 父目录名，其余 → 去扩展名的文件名）。
fn infer_name_from_path(path: &Path) -> Option<String> {
    let base = path.file_name()?.to_string_lossy().to_string();
    if base.eq_ignore_ascii_case("skill.md") {
        return path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
    }
    Some(base.trim_end_matches(".md").to_string())
}

/// 从正文首条非标题非空行推断描述（截断 120 字符，对齐 `inferSkillDescription`）。
fn infer_description_from_body(body: &str) -> String {
    const DEFAULT: &str = "按需加载的专用技能指令";
    for line in body.split('\n').map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if line.starts_with('#') {
            continue;
        }
        let compact: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !compact.is_empty() {
            return compact.chars().take(120).collect();
        }
    }
    DEFAULT.to_string()
}

/// 技能名归一化（对齐 `skillNameToKey`/`normalizeSkillKey`：小写、去 `.md`、空白/下划线→`-`）。
fn normalize_skill_key(value: &str) -> String {
    let mut s = value.trim().to_lowercase();
    if s.ends_with(".md") {
        s.truncate(s.len() - 3);
    }
    s = s.replace('\\', "/");
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_whitespace() || ch == '_' {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    out
}

/// 技能文件/目录名清洗（对齐 `sanitizeSkillFileName`：空白→`-`，仅保留 `[A-Za-z0-9._-]` 与 CJK）。
fn sanitize_skill_name(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in value.trim().chars() {
        if ch.is_whitespace() {
            if !last_dash && !out.is_empty() {
                out.push('-');
                last_dash = true;
            }
            continue;
        }
        let code = ch as u32;
        let allowed = ch.is_ascii_alphanumeric()
            || ch == '.'
            || ch == '_'
            || ch == '-'
            || (0x4e00..=0x9fa5).contains(&code);
        if allowed {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        format!("skill-{}", now_ts())
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// tools:hooks
// ---------------------------------------------------------------------------

/// 从 payload 构造 hook 对象（容错读取可选字段，对齐 Beav `registerRuntimeHook`）。
fn build_hook(payload: &Value, id: &str, event: &str, hook_type: &str) -> Value {
    let str_field = |key: &str| -> Option<String> {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    let headers = payload.get("headers").filter(|v| v.is_object()).cloned();
    let timeout_ms = payload
        .get("timeoutMs")
        .and_then(|v| v.as_i64())
        .filter(|n| n.is_positive());
    let enabled = payload
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    json!({
        "id": id,
        "event": event,
        "type": hook_type,
        "matcher": str_field("matcher"),
        "command": str_field("command"),
        "prompt": str_field("prompt"),
        "url": str_field("url"),
        "headers": headers.unwrap_or(Value::Null),
        "timeoutMs": timeout_ms.unwrap_or(20_000),
        "enabled": enabled,
    })
}

// ---------------------------------------------------------------------------
// plugin：浏览器扩展目录
// ---------------------------------------------------------------------------

/// 打包浏览器扩展目录（env `YUNYING_BROWSER_PLUGIN_DIR` 或可执行文件同级候选，须含 `manifest.json`）。
fn bundled_plugin_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("YUNYING_BROWSER_PLUGIN_DIR") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            candidates.push(d.join("resources").join("browser-extension"));
            candidates.push(d.join("browser-extension"));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.join("manifest.json").is_file())
}

/// 导出的浏览器扩展目录（env `YUNYING_BROWSER_PLUGIN_EXPORT_DIR` > `YUNYING_DATA_DIR/browser-extension` > `~/.redconvert/browser-extension`）。
fn exported_plugin_dir() -> PathBuf {
    if let Ok(d) = std::env::var("YUNYING_BROWSER_PLUGIN_EXPORT_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(d) = std::env::var("YUNYING_DATA_DIR") {
        return PathBuf::from(d).join("browser-extension");
    }
    home_dir().join(".redconvert").join("browser-extension")
}

/// 递归拷贝目录（`src` → `dst`，覆盖 `dst`）。供 `plugin:prepare-browser-extension`。
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.is_dir() {
        std::fs::remove_dir_all(dst)?;
    }
    std::fs::create_dir_all(dst)?;
    copy_dir_inner(src, dst)?;
    Ok(())
}

fn copy_dir_inner(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_dir_inner(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 当前毫秒时间戳（`std::time::SystemTime`）。
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 是否 dry-run：`dryRun===true` 或 `confirm===false`。
fn is_dry_run(payload: &Value) -> bool {
    payload
        .get("dryRun")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || payload
            .get("confirm")
            .map(|v| v.as_bool() == Some(false))
            .unwrap_or(false)
}

/// 用户 home 目录（env `HOME` > `USERPROFILE` > `.`）。
fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// skills fs：临时目录下发现技能 + frontmatter 解析 + enable/disable 改写。
    #[test]
    fn skills_fs_discover_and_frontmatter() {
        let dir = std::env::temp_dir().join(format!("yunying_devtools_skills_{}", now_ts()));
        let skills = dir.join("skills");
        std::fs::create_dir_all(skills.join("foo")).unwrap();
        std::fs::write(
            skills.join("foo").join("SKILL.md"),
            "---\nname: foo\ndescription: 测试技能\n---\n\n# foo\n\n正文内容",
        )
        .unwrap();
        std::fs::write(
            skills.join("bar.md"),
            "---\nname: bar\ndescription: 另一个\n---\nbar body",
        )
        .unwrap();

        let found = discover_skills(&skills);
        assert_eq!(found.len(), 2, "应发现 foo(SKILL.md) 与 bar(*.md)");
        // 按 name 排序：bar 在前。
        assert_eq!(found[0]["name"], "bar");
        assert_eq!(found[1]["name"], "foo");
        assert_eq!(found[1]["description"], "测试技能");
        assert_eq!(found[1]["disabled"], false);
        assert!(found[1]["location"].as_str().unwrap().ends_with("SKILL.md"));

        // 无 frontmatter 的正文应推断描述。
        let plain = std::env::temp_dir().join(format!("yunying_devtools_plain_{}.md", now_ts()));
        std::fs::write(&plain, "# 标题\n\n这是描述行").unwrap();
        let parsed = parse_skill_file(&plain, &plain).unwrap();
        assert_eq!(parsed["description"], "这是描述行");

        // enable/disable 改写 frontmatter。
        let content = "---\nname: foo\ndescription: x\n---\n\nbody";
        let disabled = set_frontmatter_disabled(content, true);
        assert!(disabled.contains("disabled: true"));
        let (fm, _) = split_frontmatter(&disabled);
        assert_eq!(fm_get(&fm.unwrap(), "disabled"), Some("true".to_string()));

        let enabled = set_frontmatter_disabled(&disabled, false);
        let (fm2, _) = split_frontmatter(&enabled);
        assert_eq!(
            fm2.as_deref().and_then(|f| fm_get(f, "disabled")),
            Some("false".to_string())
        );

        // 无 frontmatter 的内容应新建 frontmatter。
        let with_fm = set_frontmatter_disabled("# plain\n\ntext", true);
        assert!(with_fm.starts_with("---\n"));
        assert!(with_fm.contains("disabled: true"));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&plain);
    }

    #[test]
    fn bundled_open_montage_skill_is_visible() {
        let empty = std::env::temp_dir().join(format!("yunying_devtools_empty_{}", now_ts()));
        std::fs::create_dir_all(&empty).unwrap();
        let found = discover_skills_with_plugins(&empty);
        let open_montage = found
            .iter()
            .find(|skill| skill["name"] == "openmontage-video-production")
            .expect("bundled OpenMontage skill");
        assert_eq!(open_montage["sourceScope"], "plugin");
        assert_eq!(open_montage["isBuiltin"], true);
        let _ = std::fs::remove_dir_all(empty);
    }

    #[test]
    fn imports_open_montage_style_directory_with_generated_entry() {
        let root = std::env::temp_dir().join(format!("yunying_devtools_import_{}", now_ts()));
        let source = root.join("OpenMontage");
        let destination = root.join("workspace-skills");
        std::fs::create_dir_all(source.join("skills").join("creative")).unwrap();
        std::fs::write(source.join("INDEX.md"), "# OpenMontage skills").unwrap();
        std::fs::write(
            source
                .join("skills")
                .join("creative")
                .join("video-editing.md"),
            "# Video Editing",
        )
        .unwrap();
        let (imported, generated) = import_skill_directory(&source, &destination).unwrap();
        assert!(generated);
        assert!(imported.join("SKILL.md").is_file());
        assert!(std::fs::read_to_string(imported.join("SKILL.md"))
            .unwrap()
            .contains("openmontage-video-production"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// tools:hooks：内存 CRUD 往返（register/list/remove）。
    #[test]
    fn hooks_in_memory_roundtrip() {
        let id = format!("test-hook-{}", now_ts());
        // 清理可能残留的同 id 记录。
        {
            let mut store = hooks_store();
            store
                .as_mut()
                .unwrap()
                .retain(|h| h.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
        }

        let hook = build_hook(
            &json!({ "id": id, "event": "tool.before", "type": "command", "command": "echo hi" }),
            &id,
            "tool.before",
            "command",
        );
        {
            let mut store = hooks_store();
            store.as_mut().unwrap().push(hook.clone());
        }

        let listed = hooks_store().as_ref().unwrap().clone();
        assert!(listed
            .iter()
            .any(|h| h.get("id").and_then(|v| v.as_str()) == Some(id.as_str())));
        let stored = listed
            .iter()
            .find(|h| h.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
            .unwrap();
        assert_eq!(stored["event"], "tool.before");
        assert_eq!(stored["type"], "command");
        assert_eq!(stored["enabled"], true);
        assert_eq!(stored["timeoutMs"], 20_000);

        // remove。
        let removed = {
            let mut store = hooks_store();
            let vec = store.as_mut().unwrap();
            let before = vec.len();
            vec.retain(|h| h.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
            vec.len() < before
        };
        assert!(removed);
        let after = hooks_store().as_ref().unwrap().clone();
        assert!(!after
            .iter()
            .any(|h| h.get("id").and_then(|v| v.as_str()) == Some(id.as_str())));
    }

    /// 纯函数：技能名归一化 + 清洗。
    #[test]
    fn skill_name_helpers() {
        assert_eq!(normalize_skill_key("My Skill.md"), "my-skill");
        assert_eq!(normalize_skill_key("Foo_Bar"), "foo-bar");
        assert_eq!(normalize_skill_key("  Already-Clean  "), "already-clean");

        assert_eq!(sanitize_skill_name("Redbook Ops"), "Redbook-Ops");
        assert_eq!(sanitize_skill_name("a/b\\c?d*e"), "a-b-c-d-e");
        assert!(
            !sanitize_skill_name("").is_empty(),
            "空名应回退为 skill-<ts>"
        );
        // CJK 保留。
        assert_eq!(sanitize_skill_name("技能 一"), "技能-一");
    }

    /// cli-runtime execute 占位（真实实现需 PTY 子进程，CI 跳过）。
    #[tokio::test]
    #[ignore = "cli-runtime execute needs real PTY subprocess; stub only"]
    async fn cli_runtime_execute_stub() {
        let exec_id = format!("ignore-exec-{}", now_ts());
        let run = json!({ "id": exec_id, "status": "running-stub" });
        cli_runs_store()
            .as_mut()
            .unwrap()
            .insert(exec_id.clone(), run.clone());
        let got = cli_runs_store()
            .as_ref()
            .unwrap()
            .get(&exec_id)
            .cloned()
            .unwrap();
        assert_eq!(got["status"], "running-stub");
        cli_runs_store().as_mut().unwrap().remove(&exec_id);
    }

    /// skills market-search 占位（真实实现走 clawhub.ai 网络，CI 跳过）。
    #[test]
    #[ignore = "skills:market-search needs network (clawhub.ai); placeholder only"]
    fn skills_market_search_placeholder() {
        // 占位实现当前返回空数组（无 HTTP 客户端）。真实接入 reqwest 后此处发起网络请求。
        let payload = json!({ "query": "redbook" });
        assert_eq!(payload["query"], "redbook");
    }
}
