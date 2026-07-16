//! 商媒运营助手桌面应用 Tauri 壳。
//!
//! 嵌入 [`yunying_server`]（Goose + 工具 + DB），对 Beav 渲染层（已是 Tauri 形态）暴露
//! IPC 原语：`ipc_invoke` / `ipc_send` / `convert_file_src`，以及目录导入使用的
//! `pick_directory` / `pick_files`（见 Beav `src/bridge/ipcRenderer.ts`）。
//!
//! `ipc_invoke`/`ipc_send` 路由到 [`yunying_server::ipc`] 的通道分发器（替代 Electron 的 344 个 ipcMain）。
//!
//! 编译需 `tauri.conf.json` + 图标 + 前端 dist（见 `tauri.conf.json` 的 `build.frontendDist`）。
//! 本 crate 不在核心 `check.sh` 内（它是桌面打包目标，独立编译）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use yunying_server::ipc::{dispatch_invoke, dispatch_send, AppState, EventEmitter};
use yunying_server::{db::Db, goose_bridge::GooseBridge};

/// Tauri 后端的 `EventEmitter`：通过 `AppHandle::emit` 把事件推给前端（对齐 `listen(channel, cb)`）。
struct TauriEmitter {
    app: tauri::AppHandle,
}

impl EventEmitter for TauriEmitter {
    fn emit(&self, channel: &str, payload: serde_json::Value) {
        let _ = self.app.emit(channel, payload);
    }
}

/// `invoke('ipc_invoke', { channel, payload })` → 路由到 [`yunying_server::ipc::dispatch_invoke`]。
#[tauri::command]
async fn ipc_invoke(
    channel: String,
    payload: Option<serde_json::Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let payload = payload.unwrap_or(serde_json::Value::Null);
    dispatch_invoke(&channel, payload, state.inner())
        .await
        .map_err(|e| e.to_string())
}

/// `invoke('ipc_send', { channel, payload })` → 路由到 [`yunying_server::ipc::dispatch_send`]（fire-and-forget）。
#[tauri::command]
async fn ipc_send(
    channel: String,
    payload: Option<serde_json::Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let payload = payload.unwrap_or(serde_json::Value::Null);
    dispatch_send(&channel, payload, state.inner().clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 文件路径 → webview 可引用的 asset URL（对齐前端 `convertFileSrc`）。
/// Tauri 的 asset 协议在 `tauri.conf.json` 的 `app.security`/`assetProtocol` 配置。
#[tauri::command]
fn convert_file_src(path: String) -> String {
    format!("asset://localhost/{}", path.replace('\\', "/"))
}

/// Tauri 原生目录选择器。技能和插件导入共用，不再要求用户手填 sourcePath。
#[tauri::command]
async fn pick_directory(title: Option<String>) -> Result<Option<String>, String> {
    let mut dialog = rfd::AsyncFileDialog::new();
    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        dialog = dialog.set_title(title);
    }
    Ok(dialog
        .pick_folder()
        .await
        .map(|folder| folder.path().to_string_lossy().into_owned()))
}

/// Tauri 原生多文件选择器。资料库和素材导入共用，取消时返回空列表。
#[tauri::command]
async fn pick_files(title: Option<String>) -> Result<Vec<String>, String> {
    let mut dialog = rfd::AsyncFileDialog::new();
    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        dialog = dialog.set_title(title);
    }
    Ok(dialog
        .pick_files()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|file| file.path().to_string_lossy().into_owned())
        .collect())
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES") | Ok("on") | Ok("ON")
    )
}

fn tool_is_registered(tool_names: &[String], expected: &str) -> bool {
    tool_names
        .iter()
        .any(|name| name == expected || name.rsplit("__").next() == Some(expected))
}

const REQUIRED_OPERATIONS_TOOLS: &[&str] = &[
    "list_capabilities",
    "start_task",
    "generate_image",
    "upload_video",
    "social_check_account",
    "create_note",
];

fn configure_social_runtime_environment(settings: &serde_json::Value, data_dir: &Path) {
    let social_config = settings
        .get("social_tools_json")
        .and_then(|value| value.as_str())
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let social_connection = social_config
        .get("socialConnection")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let data_root = socialconnect::native::resolve_native_data_root(
        social_connection
            .get("dataDir")
            .and_then(|value| value.as_str()),
        social_connection
            .get("sauBin")
            .and_then(|value| value.as_str()),
        &data_dir.join("social-connection"),
    );
    let browser = social_connection
        .get("browserExecutable")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let proxy_url = social_connection
        .get("proxyUrl")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let runtime = socialconnect::native::NativeRuntime::new(data_root, browser, proxy_url.clone());
    let runtime_config_path = data_dir.join("social-connection-native-runtime.json");
    std::env::set_var("SOCIAL_CONNECTION_DATA_DIR", runtime.data_root());
    std::env::set_var("SOCIAL_COOKIE_DIR", runtime.cookies_dir());
    if let Some(browser) = runtime.browser_executable() {
        std::env::set_var("SOCIAL_CONNECTION_BROWSER", browser);
    } else {
        std::env::remove_var("SOCIAL_CONNECTION_BROWSER");
    }
    std::env::set_var(
        socialconnect::native::NATIVE_RUNTIME_CONFIG_ENV,
        &runtime_config_path,
    );
    if let Some(proxy_url) = proxy_url.as_deref() {
        std::env::set_var("SOCIAL_CONNECTION_PROXY_URL", proxy_url);
    } else {
        std::env::remove_var("SOCIAL_CONNECTION_PROXY_URL");
    }
    if let Err(error) =
        socialconnect::native::write_native_runtime_descriptor(&runtime_config_path, &runtime)
    {
        eprintln!("[警告] Social Connection 运行时描述写入失败：{error}");
    }
    if let Err(error) = std::fs::create_dir_all(runtime.cookies_dir()) {
        eprintln!("[警告] Social Connection 账号目录创建失败：{error}");
    }
    eprintln!(
        "[启动] Social Connection runtime=rust-cdp browser={} cookies={}",
        runtime
            .browser_executable()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<missing>".to_string()),
        runtime.cookies_dir().display(),
    );
}

fn operations_mcp_path(app: &tauri::App) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let executable_name = if cfg!(windows) {
        "yunying-ops-mcp.exe"
    } else {
        "yunying-ops-mcp"
    };
    if cfg!(debug_assertions) {
        // 开发态优先使用 workspace 刚构建的二进制，避免旧 bundle 资源副本遮蔽代码变更。
        candidates.push(workspace_root.join("target/debug").join(executable_name));
    }
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.extend([
            resource_dir.join(executable_name),
            resource_dir.join("bin").join(executable_name),
            resource_dir
                .join("_up_")
                .join("target")
                .join("release")
                .join(executable_name),
        ]);
    }
    candidates.push(workspace_root.join("target/release").join(executable_name));
    if !cfg!(debug_assertions) {
        candidates.push(workspace_root.join("target/debug").join(executable_name));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn bundled_uv_path(app: &tauri::App) -> Option<PathBuf> {
    let executable_name = if cfg!(windows) { "uv.exe" } else { "uv" };
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.extend([
            resource_dir.join("bin").join(executable_name),
            resource_dir.join(executable_name),
            resource_dir
                .join("_up_")
                .join("runtime")
                .join("bin")
                .join(executable_name),
        ]);
    }
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("UV_BIN") {
            candidates.push(PathBuf::from(path));
        }
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(
                std::env::split_paths(&path).map(|directory| directory.join(executable_name)),
            );
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn bundled_python_path(app: &tauri::App) -> Option<PathBuf> {
    let executable_name = if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    };
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.extend([
            resource_dir.join("python-runtime").join(executable_name),
            resource_dir
                .join("runtime")
                .join("python")
                .join(executable_name),
        ]);
    }
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("JIUBAN_PYTHON_BIN") {
            candidates.push(PathBuf::from(path));
        }
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("runtime/python")
                .join(executable_name),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn bundled_ffmpeg_dir(app: &tauri::App) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("ffmpeg-runtime"));
    }
    if cfg!(debug_assertions) {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("Beav/desktop/.ffmpeg-runtime/remotion-compositor"),
        );
    }
    let ffmpeg_name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let ffprobe_name = if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    candidates.into_iter().find(|directory| {
        directory.join(ffmpeg_name).is_file() && directory.join(ffprobe_name).is_file()
    })
}

fn prepend_env_path(name: &str, directory: &Path) {
    let mut paths = vec![directory.to_path_buf()];
    if let Some(existing) = std::env::var_os(name) {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(value) = std::env::join_paths(paths) {
        std::env::set_var(name, value);
    }
}

fn setting_value_is_empty(value: Option<&serde_json::Value>) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(value)) => value.trim().is_empty(),
        Some(serde_json::Value::Array(value)) => value.is_empty(),
        Some(serde_json::Value::Object(value)) => value.is_empty(),
        Some(_) => false,
    }
}

fn migrate_legacy_settings(data_dir: &Path, db: &Db) -> anyhow::Result<usize> {
    let Some(application_support) = data_dir.parent() else {
        return Ok(0);
    };
    let legacy_path = application_support
        .join("yunying-agent-desktop")
        .join("redconvert.db");
    if !legacy_path.is_file() || legacy_path == data_dir.join("redconvert.db") {
        return Ok(0);
    }

    let legacy_db = Db::open(&legacy_path)?;
    let current = db
        .settings()
        .get()
        .unwrap_or_else(|_| serde_json::json!({}));
    let legacy = legacy_db
        .settings()
        .get()
        .unwrap_or_else(|_| serde_json::json!({}));
    let mut patch = serde_json::Map::new();
    for key in [
        "api_endpoint",
        "api_key",
        "model_name",
        "workspace_dir",
        "active_space_id",
        "ai_sources_json",
        "default_ai_source_id",
        "image_provider",
        "image_endpoint",
        "image_api_key",
        "image_model",
        "image_provider_template",
        "image_aspect_ratio",
        "image_size",
        "image_quality",
        "video_endpoint",
        "video_api_key",
        "video_model",
        "mcp_servers_json",
        "social_tools_json",
    ] {
        let legacy_value = legacy.get(key);
        if setting_value_is_empty(current.get(key)) && !setting_value_is_empty(legacy_value) {
            patch.insert(key.to_string(), legacy_value.cloned().unwrap_or_default());
        }
    }
    let migrated = patch.len();
    if migrated > 0 {
        db.settings().save(&serde_json::Value::Object(patch))?;
    }
    Ok(migrated)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // 1) data dir（打包后所有用户数据存放处）。
            let data_dir = app.path().app_data_dir()?;
            let resource_dir = app.path().resource_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            if std::env::var_os("YUNYING_DATA_DIR").is_none() {
                std::env::set_var("YUNYING_DATA_DIR", &data_dir);
            }
            std::env::set_var("GOOSE_PATH_ROOT", data_dir.join("goose"));

            let bundled_plugins = resource_dir.join("builtin-plugins");
            if bundled_plugins.is_dir() {
                std::env::set_var("JIUBAN_BUILTIN_PLUGINS_ROOT", &bundled_plugins);
            }
            if let Some(python_path) = bundled_python_path(app) {
                std::env::set_var("JIUBAN_PYTHON_BIN", &python_path);
                if let Some(python_home) = python_path.parent() {
                    std::env::set_var("PYTHONHOME", python_home);
                    prepend_env_path("PATH", python_home);
                }
                eprintln!("[插件] offline Python runtime = {}", python_path.display());
            } else {
                eprintln!("[警告] 打包资源中未找到 python-runtime；将尝试开发态 uv 后备");
            }
            let uv_path = bundled_uv_path(app);
            if let Some(uv_path) = uv_path.as_ref() {
                std::env::set_var("JIUBAN_UV_BIN", uv_path);
                eprintln!("[插件] uv runtime = {}", uv_path.display());
            } else {
                eprintln!("[警告] 打包资源中未找到 uv；Python 插件无法按需启动");
            }
            if let Some(ffmpeg_dir) = bundled_ffmpeg_dir(app) {
                let ffmpeg_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
                let ffprobe_name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
                std::env::set_var("JIUBAN_FFMPEG_DIR", &ffmpeg_dir);
                std::env::set_var("FFMPEG_BIN", ffmpeg_dir.join(ffmpeg_name));
                std::env::set_var("FFPROBE_BIN", ffmpeg_dir.join(ffprobe_name));
                prepend_env_path("PATH", &ffmpeg_dir);
                #[cfg(target_os = "macos")]
                prepend_env_path("DYLD_LIBRARY_PATH", &ffmpeg_dir);
                eprintln!("[插件] ffmpeg runtime = {}", ffmpeg_dir.display());
            } else {
                eprintln!("[警告] 打包资源中未找到 ffmpeg-runtime；视频剪辑插件无法执行");
            }

            // 2) config.json：优先 app_data_dir（已部署），否则从 bundled resource 复制。
            //    config.json 作为 Tauri resource 打包在 .app 内（tauri.conf.json resources），
            //    首次启动自动复制到 app_data_dir → 一键安装即可运行。
            let config_path = data_dir.join("config.json");
            if !config_path.exists() {
                // 从 bundled resource 复制。Tauri 会把 "../config.json" 放到 Resources/_up_/config.json。
                let resource_config = [
                    resource_dir.join("config.json"),
                    resource_dir.join("_up_").join("config.json"),
                ]
                .into_iter()
                .find(|path| path.exists());
                if let Some(resource_config) = resource_config {
                    std::fs::copy(&resource_config, &config_path)?;
                    eprintln!("[启动] 从 bundled resource 部署 config.json → {}", config_path.display());
                } else {
                    // dev 模式无 bundled resource → 尝试 cwd。
                    if let Ok(c) = yunying_config::load(None) {
                        let _ = std::fs::write(&config_path, serde_json::to_string_pretty(&c).unwrap_or_default());
                        eprintln!("[启动] 从 cwd 加载 config.json（dev 模式）");
                    } else {
                        // 全新安装：创建默认（空 key，用户在设置页配置）。
                        let default = yunying_config::Config::default();
                        let _ = std::fs::write(&config_path, serde_json::to_string_pretty(&default).unwrap_or_default());
                        eprintln!("[启动] 首次运行：已创建默认 config.json（请在设置页配置 API key）");
                    }
                }
            }
            // `load(None)` 以及随后启动的 Ops/OpenMontage 子进程必须与正式应用使用
            // 同一份 app_data 配置，不能依赖启动时的当前工作目录。
            std::env::set_var("YUNYING_CONFIG_PATH", &config_path);
            // 先同步绑定 loopback 随机端口并安装子进程环境；绑定失败直接终止 setup。
            let bridge_server = tauri::async_runtime::block_on(
                yunying_server::plugins::bridge::PluginBridgeServer::bind_loopback(),
            )?;
            let bridge_endpoint = bridge_server.endpoint().clone();
            bridge_endpoint.install_process_environment();
            let bridge_address = bridge_endpoint.address();
            let builtin_plugins_sync = tauri::async_runtime::spawn_blocking(|| {
                let results = yunying_server::plugins::sync_builtin_plugins()?;
                for result in results {
                    eprintln!(
                        "[插件] 已同步 {} {} copied={} reused={} -> {}",
                        result.plugin.id,
                        result.plugin.version,
                        result.copied_files,
                        result.reused_files,
                        result.plugin.root.display()
                    );
                }
                Ok::<(), anyhow::Error>(())
            });
            let mut cfg = yunying_config::load(Some(&config_path)).unwrap_or_default();

            // 3) DB：userData/redconvert.db。
            let db = Db::open(&data_dir.join("redconvert.db"))?;
            match migrate_legacy_settings(&data_dir, &db) {
                Ok(count) if count > 0 => {
                    eprintln!("[启动] 已从旧桌面数据目录迁移 {count} 项缺失设置");
                }
                Ok(_) => {}
                Err(error) => eprintln!("[警告] 迁移旧桌面设置失败：{error}"),
            }
            if let Ok(settings) = db.settings().get() {
                configure_social_runtime_environment(
                    &settings,
                    &data_dir,
                );
                let before = (
                    cfg.goose.provider.clone(),
                    cfg.goose.model.clone(),
                    cfg.goose.base_url.clone(),
                    cfg.goose.api_key.clone(),
                );
                cfg = yunying_server::goose_bridge::apply_settings_to_config(cfg, &settings);
                let after = (
                    cfg.goose.provider.clone(),
                    cfg.goose.model.clone(),
                    cfg.goose.base_url.clone(),
                    cfg.goose.api_key.clone(),
                );
                if before != after {
                    eprintln!(
                        "[启动] 已使用 settings 中的默认 AI 源覆盖 Goose 配置（provider={} model={}）",
                        cfg.goose.provider, cfg.goose.model
                    );
                }
            }

            // 4) Goose 嵌入（容错：API key 为空或初始化失败时降级）。
            //    必须在 async runtime 内构造（Agent::new() → sqlx 需要 Tokio context）。
            let goose = tauri::async_runtime::block_on(async {
                if cfg.goose.api_key.is_empty() {
                    eprintln!("[启动] config.json 缺 goose.api_key——GooseBridge 降级（无 provider）。请在设置页配置 API key 后重启。");
                    GooseBridge::default()
                } else {
                    match GooseBridge::new(&cfg).await {
                        Ok(g) => {
                            eprintln!("[启动] GooseBridge 初始化成功（provider={} model={}）", cfg.goose.provider, cfg.goose.model);
                            g
                        }
                        Err(e) => {
                            eprintln!("[警告] GooseBridge 初始化失败：{e}——降级为无 provider。");
                            GooseBridge::default()
                        }
                    }
                }
            });

            let emitter: Arc<dyn EventEmitter> = Arc::new(TauriEmitter {
                app: app.handle().clone(),
            });
            let login = Arc::new(yunying_server::login::LoginService::new(Arc::new(
                yunying_server::login::StubLoginDriver,
            )));

            // readiness 1/3：bridge 使用一个仅供生成通道使用的占位 AppState，
            // 因此可以在调度器激活前开始服务。
            let bridge_state = Arc::new(AppState {
                db: db.clone(),
                goose: goose.clone(),
                emitter: emitter.clone(),
                login: login.clone(),
                redclaw_scheduler:
                    yunying_server::ipc::redclaw_runner::RedClawScheduler::inactive(db.clone()),
            });
            tauri::async_runtime::spawn(async move {
                if let Err(error) = bridge_server.serve(bridge_state).await {
                    eprintln!("[错误] 插件 App Bridge 运行失败（{bridge_address}）：{error}");
                }
            });
            eprintln!("[启动] 插件 App Bridge 已安全绑定 {bridge_address}");

            // readiness 2/3：operations 注册返回且关键工具可发现后才算就绪。
            let operations_ready = if let Some(ops_mcp_path) = operations_mcp_path(app) {
                let path = ops_mcp_path.to_string_lossy().into_owned();
                match tauri::async_runtime::block_on(async {
                    goose.register_operations_mcp(&path).await?;
                    goose.tool_names().await
                }) {
                    Ok(tool_names) => {
                        let missing = REQUIRED_OPERATIONS_TOOLS
                            .iter()
                            .copied()
                            .filter(|expected| !tool_is_registered(&tool_names, expected))
                            .collect::<Vec<_>>();
                        if missing.is_empty() {
                            eprintln!(
                                "[启动] yunying-ops MCP 已就绪：{path}（{} 个工具）",
                                tool_names.len()
                            );
                            true
                        } else {
                            eprintln!(
                                "[错误] yunying-ops MCP 缺少必要工具 {}（当前 {} 个）",
                                missing.join(", "),
                                tool_names.len()
                            );
                            false
                        }
                    }
                    Err(error) => {
                        eprintln!("[错误] yunying-ops MCP 注册失败：{error}");
                        false
                    }
                }
            } else {
                eprintln!("[错误] 未找到 yunying-ops-mcp，调度器不会激活");
                false
            };

            // readiness 3/3：等待资源同步和 OpenMontage MCP 初始化完成，并验证真实视频工具。
            let openmontage_ready = match tauri::async_runtime::block_on(async {
                builtin_plugins_sync
                    .await
                    .map_err(|error| anyhow::anyhow!("等待内置插件同步失败：{error}"))??;
                let settings = db
                    .settings()
                    .get()
                    .unwrap_or_else(|_| serde_json::json!({}));
                let plugin_root =
                    yunying_server::plugins::mount_openmontage(&goose, &settings).await?;
                let tool_names = goose.tool_names().await?;
                if !tool_is_registered(&tool_names, "seedance_video") {
                    anyhow::bail!("OpenMontage 缺少必要工具 seedance_video");
                }
                Ok::<_, anyhow::Error>((plugin_root, tool_names.len()))
            }) {
                Ok((plugin_root, tool_count)) => {
                    eprintln!(
                        "[启动] OpenMontage 已就绪：{}（当前共 {tool_count} 个工具）",
                        plugin_root.display()
                    );
                    true
                }
                Err(error) => {
                    eprintln!("[错误] OpenMontage 自动挂载未就绪：{error}");
                    false
                }
            };

            // 所有外部副作用入口都完成 readiness 后，才允许恢复并认领持久化任务。
            let redclaw_scheduler = if operations_ready && openmontage_ready {
                eprintln!("[启动] bridge/operations/OpenMontage 均已就绪，激活 RedClaw 调度器");
                tauri::async_runtime::block_on(
                    yunying_server::ipc::redclaw_runner::RedClawScheduler::start(
                        db.clone(),
                        goose.clone(),
                        emitter.clone(),
                    ),
                )?
            } else {
                eprintln!("[错误] 必要工具未全部就绪，RedClaw 调度器保持未激活");
                yunying_server::ipc::redclaw_runner::RedClawScheduler::inactive(db.clone())
            };
            let state = Arc::new(AppState {
                db: db.clone(),
                goose: goose.clone(),
                emitter: emitter.clone(),
                login: login.clone(),
                redclaw_scheduler,
            });

            {
                let refresh_state = state.clone();
                tauri::async_runtime::spawn(async move {
                    match yunying_server::ipc::data::refresh_default_ai_source_model_metadata(
                        &refresh_state.db,
                    )
                    .await
                    {
                        Ok(result) => {
                            let active = result.get("activeModel").cloned().unwrap_or_default();
                            eprintln!(
                                "[模型] 自动刷新完成 updated={} active={}",
                                result.get("updated").and_then(|value| value.as_bool()).unwrap_or(false),
                                active
                            );
                            if result.get("updated").and_then(|value| value.as_bool()) == Some(true) {
                                if let Ok(settings) = refresh_state.db.settings().get() {
                                    if let Err(error) = refresh_state.goose.reload_from_settings(&settings).await {
                                        eprintln!("[模型] 自动刷新后的 provider 热更新失败：{error}");
                                    }
                                }
                            }
                        }
                        Err(error) => eprintln!("[模型] 自动刷新失败（继续使用 Goose canonical 信息）：{error}"),
                    }
                });
            }

            if env_flag("YUNYING_STARTUP_IPC_DIAGNOSTICS") {
                let diag_state = state.clone();
                tauri::async_runtime::block_on(async move {
                    eprintln!("[诊断] 测试 spaces:list...");
                    match yunying_server::ipc::dispatch_invoke("spaces:list", serde_json::Value::Null, &diag_state).await {
                        Ok(v) => eprintln!("[诊断OK] spaces:list = {}", serde_json::to_string(&v).unwrap_or_default().chars().take(200).collect::<String>()),
                        Err(e) => eprintln!("[诊断FAIL] spaces:list: {e}"),
                    }
                    eprintln!("[诊断] 测试 chat:create-context-session...");
                    let payload = serde_json::json!({"contextId":"test","contextType":"redclaw","title":"测试"});
                    match yunying_server::ipc::dispatch_invoke("chat:create-context-session", payload, &diag_state).await {
                        Ok(v) => eprintln!("[诊断OK] chat:create-context-session = {}", serde_json::to_string(&v).unwrap_or_default().chars().take(300).collect::<String>()),
                        Err(e) => eprintln!("[诊断FAIL] chat:create-context-session: {e}"),
                    }
                    eprintln!("[诊断] 测试 media:list...");
                    match yunying_server::ipc::dispatch_invoke(
                        "media:list",
                        serde_json::json!({ "limit": 500 }),
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => {
                            let assets = v.get("assets").and_then(|item| item.as_array());
                            let thumbnails = assets
                                .map(|items| items.iter().filter(|item| item.get("thumbnailUrl").is_some()).count())
                                .unwrap_or(0);
                            let html = assets
                                .map(|items| items.iter().filter(|item| item.get("mimeType").and_then(|value| value.as_str()) == Some("text/html")).count())
                                .unwrap_or(0);
                            let external = assets
                                .map(|items| items.iter().filter(|item| item.get("source").and_then(|value| value.as_str()) == Some("external")).count())
                                .unwrap_or(0);
                            eprintln!(
                                "[诊断OK] media:list success={} assets={} thumbnails={} html={} external={} root={}",
                                v.get("success").and_then(|item| item.as_bool()).unwrap_or(false),
                                assets.map(Vec::len).unwrap_or(0),
                                thumbnails,
                                html,
                                external,
                                v.get("root").and_then(|item| item.as_str()).unwrap_or(""),
                            );
                        }
                        Err(e) => eprintln!("[诊断FAIL] media:list: {e}"),
                    }
                    eprintln!("[诊断] 测试 manuscripts:list...");
                    match yunying_server::ipc::dispatch_invoke(
                        "manuscripts:list",
                        serde_json::Value::Null,
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => eprintln!(
                            "[诊断OK] manuscripts:list items={}",
                            v.as_array().map(Vec::len).unwrap_or(0),
                        ),
                        Err(e) => eprintln!("[诊断FAIL] manuscripts:list: {e}"),
                    }
                    eprintln!("[诊断] 测试 redclaw 任务通道...");
                    match yunying_server::ipc::dispatch_invoke(
                        "redclaw:runner-status",
                        serde_json::Value::Null,
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => eprintln!(
                            "[诊断OK] redclaw:runner-status automaticExecution={} persistentDefinitions={}",
                            v.get("capabilities")
                                .and_then(|item| item.get("automaticExecution"))
                                .and_then(|item| item.as_bool())
                                .unwrap_or(false),
                            v.get("capabilities")
                                .and_then(|item| item.get("persistentDefinitions"))
                                .and_then(|item| item.as_bool())
                                .unwrap_or(false),
                        ),
                        Err(e) => eprintln!("[诊断FAIL] redclaw:runner-status: {e}"),
                    }
                    for channel in [
                        "redclaw:runner-list-scheduled",
                        "redclaw:runner-list-long-cycle",
                    ] {
                        match yunying_server::ipc::dispatch_invoke(
                            channel,
                            serde_json::Value::Null,
                            &diag_state,
                        )
                        .await
                        {
                            Ok(v) => eprintln!(
                                "[诊断OK] {} tasks={}",
                                channel,
                                v.get("tasks").and_then(|item| item.as_array()).map(Vec::len).unwrap_or(0),
                            ),
                            Err(e) => eprintln!("[诊断FAIL] {channel}: {e}"),
                        }
                    }
                    match yunying_server::ipc::dispatch_invoke(
                        "redclaw:task-list",
                        serde_json::json!({ "includeDrafts": true }),
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => eprintln!(
                            "[诊断OK] redclaw:task-list tasks={}",
                            v.as_array().map(Vec::len).unwrap_or(0),
                        ),
                        Err(e) => eprintln!("[诊断FAIL] redclaw:task-list: {e}"),
                    }
                    match yunying_server::ipc::dispatch_invoke(
                        "generation:list-job-summaries",
                        serde_json::json!({ "limit": 200 }),
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => eprintln!(
                            "[诊断OK] generation:list-job-summaries jobs={}",
                            v.get("items").and_then(|item| item.as_array()).map(Vec::len).unwrap_or(0),
                        ),
                        Err(e) => eprintln!("[诊断FAIL] generation:list-job-summaries: {e}"),
                    }
                    eprintln!("[诊断] 测试 logs:append-renderer...");
                    match yunying_server::ipc::dispatch_invoke(
                        "logs:append-renderer",
                        serde_json::json!({
                            "level": "info",
                            "category": "startup.diagnostics",
                            "event": "ipc.smoke",
                            "message": "renderer log IPC smoke",
                            "fields": { "source": "tauri-startup" }
                        }),
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => eprintln!(
                            "[诊断OK] logs:append-renderer success={} path={}",
                            v.get("success").and_then(|item| item.as_bool()).unwrap_or(false),
                            v.get("path").and_then(|item| item.as_str()).unwrap_or(""),
                        ),
                        Err(e) => eprintln!("[诊断FAIL] logs:append-renderer: {e}"),
                    }
                    eprintln!("[诊断] 测试 notifications:permission_state...");
                    match yunying_server::ipc::dispatch_invoke(
                        "notifications:permission_state",
                        serde_json::Value::Null,
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => eprintln!(
                            "[诊断OK] notifications:permission_state state={}",
                            v.get("state").and_then(|item| item.as_str()).unwrap_or(""),
                        ),
                        Err(e) => eprintln!("[诊断FAIL] notifications:permission_state: {e}"),
                    }
                    eprintln!("[诊断] 测试 plugins:list / plugins:get-settings...");
                    match yunying_server::ipc::dispatch_invoke(
                        "plugins:list",
                        serde_json::Value::Null,
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => {
                            let plugins = v.get("plugins").and_then(|item| item.as_array());
                            eprintln!(
                                "[诊断OK] plugins:list success={} plugins={}",
                                v.get("success").and_then(|item| item.as_bool()).unwrap_or(false),
                                plugins.map(Vec::len).unwrap_or(0),
                            );
                            if let Some(plugin_id) = plugins
                                .and_then(|items| items.iter().find(|item| {
                                    item.get("settings")
                                        .and_then(|settings| settings.get("fields"))
                                        .and_then(|fields| fields.as_array())
                                        .is_some_and(|fields| !fields.is_empty())
                                }))
                                .and_then(|item| item.get("id").or_else(|| item.get("name")))
                                .and_then(|item| item.as_str())
                            {
                                match yunying_server::ipc::dispatch_invoke(
                                    "plugins:get-settings",
                                    serde_json::json!({ "id": plugin_id }),
                                    &diag_state,
                                )
                                .await
                                {
                                    Ok(settings) => eprintln!(
                                        "[诊断OK] plugins:get-settings id={} success={} fields={}",
                                        plugin_id,
                                        settings.get("success").and_then(|item| item.as_bool()).unwrap_or(false),
                                        settings
                                            .get("settings")
                                            .and_then(|item| item.get("values"))
                                            .and_then(|item| item.as_object())
                                            .map(serde_json::Map::len)
                                            .unwrap_or(0),
                                    ),
                                    Err(e) => eprintln!("[诊断FAIL] plugins:get-settings: {e}"),
                                }
                            }
                        }
                        Err(e) => eprintln!("[诊断FAIL] plugins:list: {e}"),
                    }
                    eprintln!("[诊断] 测试 social-tools:get-status...");
                    match yunying_server::ipc::dispatch_invoke(
                        "social-tools:get-status",
                        serde_json::Value::Null,
                        &diag_state,
                    )
                    .await
                    {
                        Ok(v) => eprintln!(
                            "[诊断OK] social-tools:get-status success={} config={}",
                            v.get("success").and_then(|item| item.as_bool()).unwrap_or(false),
                            v.get("config").is_some_and(|item| item.is_object()),
                        ),
                        Err(e) => eprintln!("[诊断FAIL] social-tools:get-status: {e}"),
                    }
                });
            }

            if let Ok(mode) = std::env::var("YUNYING_GENERATION_SMOKE") {
                let mode = mode.trim().to_lowercase();
                let diag_state = state.clone();
                tauri::async_runtime::block_on(async move {
                    if mode == "image" || mode == "all" {
                        eprintln!("[生成测试] 开始真实图片生成...");
                        let payload = serde_json::json!({
                            "prompt": "商媒运营助手桌面应用测试图：白色桌面上的一杯柠檬水，自然光，真实产品摄影，无文字",
                            "title": "商媒运营助手图片生成 smoke",
                            "count": 1,
                            "size": "1024x1024",
                            "quality": "standard",
                            "aspectRatio": "1:1"
                        });
                        match yunying_server::ipc::dispatch_invoke(
                            "image-gen:generate",
                            payload,
                            &diag_state,
                        )
                        .await
                        {
                            Ok(value) => eprintln!(
                                "[生成测试] image success={} assets={} first={}",
                                value.get("success").and_then(|item| item.as_bool()).unwrap_or(false),
                                value.get("assets").and_then(|item| item.as_array()).map(Vec::len).unwrap_or(0),
                                value
                                    .get("assets")
                                    .and_then(|item| item.as_array())
                                    .and_then(|items| items.first())
                                    .and_then(|item| item.get("absolutePath"))
                                    .and_then(|item| item.as_str())
                                    .unwrap_or_else(|| value.get("error").and_then(|item| item.as_str()).unwrap_or("")),
                            ),
                            Err(error) => eprintln!("[生成测试] image invoke failed: {error}"),
                        }
                    }
                    if mode == "video" || mode == "all" {
                        eprintln!("[生成测试] 开始真实视频生成...");
                        let payload = serde_json::json!({
                            "prompt": "真实产品短视频：白色桌面上的一杯柠檬水，镜头缓慢推进，自然光，无文字",
                            "title": "商媒运营助手视频生成 smoke",
                            "generationMode": "text-to-video",
                            "count": 1,
                            "durationSeconds": 5,
                            "resolution": "720p",
                            "aspectRatio": "16:9"
                        });
                        match yunying_server::ipc::dispatch_invoke(
                            "video-gen:generate",
                            payload,
                            &diag_state,
                        )
                        .await
                        {
                            Ok(value) => eprintln!(
                                "[生成测试] video success={} assets={} first={}",
                                value.get("success").and_then(|item| item.as_bool()).unwrap_or(false),
                                value.get("assets").and_then(|item| item.as_array()).map(Vec::len).unwrap_or(0),
                                value
                                    .get("assets")
                                    .and_then(|item| item.as_array())
                                    .and_then(|items| items.first())
                                    .and_then(|item| item.get("absolutePath"))
                                    .and_then(|item| item.as_str())
                                    .unwrap_or_else(|| value.get("error").and_then(|item| item.as_str()).unwrap_or("")),
                            ),
                            Err(error) => eprintln!("[生成测试] video invoke failed: {error}"),
                        }
                    }
                });
            }

            app.manage(state);

            if env_flag("YUNYING_OPEN_DEVTOOLS") {
                if let Some(webview) = app.get_webview_window("main") {
                    webview.open_devtools();
                    eprintln!("[调试] 已请求打开 WebView DevTools（YUNYING_OPEN_DEVTOOLS=1）");
                } else {
                    eprintln!("[调试] 未找到 main WebView window，无法打开 DevTools");
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if !env_flag("YUNYING_WINDOW_DIAGNOSTICS") {
                return;
            }
            match event {
                tauri::WindowEvent::CloseRequested { .. } => {
                    eprintln!("[窗口] {} 收到关闭请求", window.label());
                }
                tauri::WindowEvent::Destroyed => {
                    eprintln!("[窗口] {} 已销毁", window.label());
                }
                tauri::WindowEvent::Focused(focused) => {
                    eprintln!("[窗口] {} focused={focused}", window.label());
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            ipc_invoke,
            ipc_send,
            convert_file_src,
            pick_directory,
            pick_files
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用失败");
}
