//! Social Connection / MediaCrawler 设置页 IPC。
//!
//! Social Connection 账号池以 Playwright 兼容 storage_state 文件为事实源：
//! - `get-status` 发现已配置账号与磁盘上的全部账号文件；
//! - `check-account` 使用纯 Rust CDP 浏览器做真实在线校验；
//! - `start/get/stop-login` 管理 Rust 浏览器任务并把二维码图片转成 data URL 返回前端；
//! - import/export/delete 提供账号文件完整生命周期。
//!
//! 账号索引与最近状态同步到 `platform_accounts`，但不会把 cookie 打到日志/IPC 状态列表中。

use base64::Engine;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use socialconnect::account::{Account, AccountFileInfo, SUPPORTED_PLATFORMS};
use socialconnect::native::{
    native_capabilities, resolve_native_data_root, write_native_runtime_descriptor,
    NativeLoginHandle, NativeRuntime, NATIVE_RUNTIME_CONFIG_ENV,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use super::AppState;
use crate::db::Db;

const CHECK_TIMEOUT: Duration = Duration::from_secs(120);
const LOGIN_QR_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone)]
struct ActiveLogin {
    runtime: NativeRuntime,
    account: Account,
    handle: Arc<Mutex<NativeLoginHandle>>,
}

static ACTIVE_LOGINS: Lazy<Mutex<HashMap<String, ActiveLogin>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn opt_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn session_key(account: &Account) -> String {
    format!("{}:{}", account.platform, account.profile)
}

pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        "social-tools:get-status" => social_tools_get_status(state).await,
        "social-tools:save-config" => save_config(payload, state).await,
        "social-tools:check-account" => check_account(payload, state).await,
        "social-tools:start-login" => start_login(payload, state).await,
        "social-tools:get-login-status" => get_login_status(payload, state).await,
        "social-tools:stop-login" => stop_login(payload, state).await,
        "social-tools:import-account" => import_account(payload, state).await,
        "social-tools:export-account" => export_account(payload, state).await,
        "social-tools:delete-account" => delete_account(payload, state).await,

        // MediaCrawler 仍由其 API 运行时承载；这里至少保持设置页返回契约一致，避免 `{ok:true}`
        // 被前端误判失败。真正 crawler 启停在后续 MediaCrawler 专项迁移中完成。
        "social-tools:start-mediacrawler-login" => Ok(json!({
            "success": false,
            "running": false,
            "message": "MediaCrawler API 登录运行时尚未启动，请先确认本机 MediaCrawler 服务。",
            "error": "mediacrawler_runtime_unavailable",
        })),
        "social-tools:get-mediacrawler-status" => Ok(json!({
            "success": false,
            "running": false,
            "status": "idle",
            "logs": [],
            "message": "MediaCrawler 当前无登录任务。",
        })),
        "social-tools:stop-mediacrawler" => Ok(json!({
            "success": true,
            "running": false,
            "status": "idle",
            "logs": [],
            "message": "MediaCrawler 登录任务已停止。",
        })),
        "social-tools:open-social-cookies-dir" => open_social_cookies_dir(state).await,
        "social-tools:open-mediacrawler-browser-data-dir" => open_media_crawler_dir().await,
        other => Err(anyhow::anyhow!("accounts 命名空间未实现通道: {other}")),
    }
}

async fn base_status(state: &AppState) -> anyhow::Result<Value> {
    super::system::invoke("social-tools:get-status", Value::Null, state).await
}

fn fallback_social_data_root() -> PathBuf {
    std::env::var_os("YUNYING_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .join("social-connection")
}

fn runtime_from_config(config: &Value) -> NativeRuntime {
    let social_connection = config.get("socialConnection").unwrap_or(&Value::Null);
    let data_root = resolve_native_data_root(
        social_connection.get("dataDir").and_then(Value::as_str),
        social_connection.get("sauBin").and_then(Value::as_str),
        &fallback_social_data_root(),
    );
    let browser = social_connection
        .get("browserExecutable")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SOCIAL_CONNECTION_BROWSER").map(PathBuf::from));
    NativeRuntime::new(
        data_root,
        browser,
        proxy_from_config(config).map(str::to_string),
    )
}

fn proxy_from_config(config: &Value) -> Option<&str> {
    config
        .get("socialConnection")
        .and_then(|value| value.get("proxyUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn account_from_payload(payload: &Value, config: &Value) -> anyhow::Result<Account> {
    let platform = opt_str(payload, "platform").unwrap_or("");
    let configured = config
        .get("socialConnection")
        .and_then(|value| value.get("accounts"))
        .and_then(|value| value.get(platform))
        .and_then(Value::as_str)
        .unwrap_or("default");
    let profile = opt_str(payload, "account")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(configured);
    Account::try_new(platform, profile).map_err(Into::into)
}

async fn social_tools_get_status(state: &AppState) -> anyhow::Result<Value> {
    let mut status = base_status(state).await?;
    let mut config = status.get("config").cloned().unwrap_or_else(|| json!({}));
    let runtime = runtime_from_config(&config);
    config["socialConnection"]["dataDir"] = json!(runtime.data_root().to_string_lossy());
    config["socialConnection"]["browserExecutable"] = runtime
        .browser_executable()
        .map(|path| json!(path.to_string_lossy()))
        .unwrap_or(Value::Null);
    status["config"] = config.clone();
    std::fs::create_dir_all(runtime.cookies_dir()).ok();

    let configured_map = config
        .get("socialConnection")
        .and_then(|value| value.get("accounts"))
        .and_then(Value::as_object);
    let mut configured_accounts = Vec::new();
    for platform in SUPPORTED_PLATFORMS {
        let profile = configured_map
            .and_then(|accounts| accounts.get(*platform))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default");
        let account = Account::try_new(platform, profile)?;
        let info = runtime.account_info(&account)?;
        sync_account_file_to_db(&state.db, &account, &info, "publisher", None)?;
        configured_accounts.push(serde_json::to_value(info)?);
    }

    let discovered = runtime.discover_accounts()?;
    for info in &discovered {
        let account = Account::try_new(&info.platform, &info.account)?;
        sync_account_file_to_db(&state.db, &account, info, "publisher", None)?;
    }
    let account_records = state.db.query_all_json(
        "SELECT platform, profile AS account, status, error_message AS errorMessage, \
                last_login_at AS lastLoginAt, last_check_at AS lastCheckAt, updated_at AS updatedAt \
         FROM platform_accounts WHERE kind IN ('publisher','both') \
         ORDER BY platform ASC, profile ASC",
        &[],
    )?;
    let active_logins = active_login_summaries().await;

    let roots = status
        .get_mut("roots")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("social-tools:get-status 缺少 roots"))?;
    roots.insert(
        "socialConnectionRoot".into(),
        json!(runtime.data_root().to_string_lossy()),
    );
    roots.insert(
        "socialCookiesDir".into(),
        json!(runtime.cookies_dir().to_string_lossy()),
    );

    status["socialConnection"] = json!({
        "rootExists": runtime.data_root().exists(),
        "runtimeMode": "rust-cdp",
        "dataRoot": runtime.data_root().to_string_lossy(),
        "browserExecutable": runtime.browser_executable().map(|path| path.to_string_lossy().into_owned()),
        "browserAvailable": runtime.browser_available(),
        "cookiesDirExists": runtime.cookies_dir().exists(),
        "accounts": configured_accounts,
        "discoveredAccounts": discovered,
        "accountRecords": account_records,
        "activeLogins": active_logins,
        "capabilities": native_capabilities(),
    });
    status["success"] = json!(true);
    Ok(status)
}

async fn save_config(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let response =
        super::system::invoke("social-tools:save-config", payload.clone(), state).await?;
    if response.get("success").and_then(Value::as_bool) != Some(true) {
        return Ok(response);
    }
    let config = response
        .get("config")
        .cloned()
        .unwrap_or_else(|| payload.get("config").cloned().unwrap_or_default());
    let runtime = runtime_from_config(&config);
    std::fs::create_dir_all(runtime.cookies_dir())?;
    std::env::set_var("SOCIAL_CONNECTION_DATA_DIR", runtime.data_root());
    std::env::set_var("SOCIAL_COOKIE_DIR", runtime.cookies_dir());
    if let Some(browser) = runtime.browser_executable() {
        std::env::set_var("SOCIAL_CONNECTION_BROWSER", browser);
    } else {
        std::env::remove_var("SOCIAL_CONNECTION_BROWSER");
    }
    let proxy_url = proxy_from_config(&config);
    if let Some(proxy_url) = proxy_url {
        std::env::set_var("SOCIAL_CONNECTION_PROXY_URL", proxy_url);
        std::env::set_var("YT_PROXY", proxy_url);
    } else {
        std::env::remove_var("SOCIAL_CONNECTION_PROXY_URL");
        std::env::remove_var("YT_PROXY");
    }
    let runtime_config_path = std::env::var_os(NATIVE_RUNTIME_CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime.data_root().join(".yunying-native-runtime.json"));
    std::env::set_var(NATIVE_RUNTIME_CONFIG_ENV, &runtime_config_path);
    write_native_runtime_descriptor(&runtime_config_path, &runtime)?;

    if let Some(accounts) = config
        .get("socialConnection")
        .and_then(|value| value.get("accounts"))
        .and_then(Value::as_object)
    {
        for (platform, profile) in accounts {
            let profile = profile
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("default");
            let account = Account::try_new(platform, profile)?;
            let info = runtime.account_info(&account)?;
            sync_account_file_to_db(&state.db, &account, &info, "publisher", None)?;
        }
    }
    Ok(response)
}

async fn check_account(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let account = account_from_payload(&payload, &config)?;
    let info = runtime.account_info(&account)?;
    if !runtime.browser_available() {
        return Ok(json!({
            "success": false,
            "platform": account.platform,
            "account": account.profile,
            "accountFile": info.cookie_path,
            "accountExists": info.exists,
            "status": "runtime_missing",
            "error": "未找到可用的 Chrome/Chromium/Edge，请在设置中指定浏览器可执行文件。",
        }));
    }
    if !info.exists && !runtime.profile_dir(&account).exists() {
        update_account_status(
            &state.db,
            &account,
            "expired",
            Some("账号文件不存在"),
            false,
        )?;
        return Ok(json!({
            "success": false,
            "platform": account.platform,
            "account": account.profile,
            "accountFile": info.cookie_path,
            "accountExists": false,
            "status": "expired",
            "error": "账号文件和独立浏览器 profile 均不存在，请先扫码登录或导入 Cookie。",
        }));
    }

    let output = runtime.check_account(&account, CHECK_TIMEOUT).await?;
    let success = output.success;
    let error_message = if success {
        None
    } else {
        Some(output.message.clone())
    };
    let latest_info = runtime.account_info(&account)?;
    sync_account_file_to_db(
        &state.db,
        &account,
        &latest_info,
        "publisher",
        Some(if success { "logged_in" } else { "expired" }),
    )?;
    update_account_status(
        &state.db,
        &account,
        if success { "logged_in" } else { "expired" },
        error_message.as_deref(),
        false,
    )?;
    Ok(json!({
        "success": success,
        "loggedIn": success,
        "platform": account.platform,
        "account": account.profile,
        "runtimeMode": "rust-cdp",
        "currentUrl": output.current_url,
        "accountFile": latest_info.cookie_path,
        "accountExists": latest_info.exists,
        "accountUpdatedAt": latest_info.updated_at,
        "status": if success { "logged_in" } else { "expired" },
        "message": output.message,
        "error": error_message,
    }))
}

async fn start_login(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let account = account_from_payload(&payload, &config)?;
    let restart = payload
        .get("restart")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !runtime.browser_available() {
        return Ok(json!({
            "success": false,
            "running": false,
            "platform": account.platform,
            "account": account.profile,
            "accountFile": runtime.account_file(&account).to_string_lossy(),
            "message": "未找到可用的 Chrome/Chromium/Edge，请在设置中指定浏览器可执行文件。",
            "error": "browser_runtime_missing",
        }));
    }

    let key = session_key(&account);
    if let Some(existing) = active_login(&key).await {
        let running = existing.handle.lock().await.is_running().await;
        if running && !restart {
            return snapshot_active(&existing, &state.db).await;
        }
        if running {
            existing.handle.lock().await.stop().await;
        }
        remove_active_login_if_same(&key, &existing).await;
    }

    let login = ActiveLogin {
        runtime: runtime.clone(),
        account: account.clone(),
        handle: Arc::new(Mutex::new(runtime.start_login(account.clone()))),
    };
    let selected = {
        let mut active = ACTIVE_LOGINS.lock().await;
        if let Some(current) = active.get(&key) {
            current.clone()
        } else {
            active.insert(key.clone(), login.clone());
            login.clone()
        }
    };
    if !Arc::ptr_eq(&selected.handle, &login.handle) {
        login.handle.lock().await.stop().await;
        return snapshot_active(&selected, &state.db).await;
    }
    update_account_status(&state.db, &account, "logging_in", None, false)?;

    let deadline = tokio::time::Instant::now() + LOGIN_QR_WAIT;
    loop {
        let snapshot = snapshot_active(&login, &state.db).await?;
        if snapshot
            .get("qrcodeUrl")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
            || snapshot.get("running").and_then(Value::as_bool) != Some(true)
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn get_login_status(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let account = account_from_payload(&payload, &config)?;
    let key = session_key(&account);
    if let Some(active) = active_login(&key).await {
        return snapshot_active(&active, &state.db).await;
    }

    let info = runtime.account_info(&account)?;
    let record = state.db.query_one_json(
        "SELECT status, error_message AS errorMessage, last_login_at AS lastLoginAt, \
                last_check_at AS lastCheckAt \
         FROM platform_accounts WHERE platform=?1 AND profile=?2",
        &[json!(account.platform), json!(account.profile)],
    )?;
    let persisted_status = record
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or(if info.exists { "saved" } else { "idle" });
    let logged_in = persisted_status == "logged_in";
    let message = match persisted_status {
        "logged_in" => "登录完成，账号文件已保存并通过在线校验。",
        "saved_unverified" => "账号文件已保存，但在线校验未通过；请点击检查账号。",
        "saved" => "账号文件已保存，尚未进行在线检查。",
        "expired" => "账号登录态已失效，请重新登录。",
        "error" => "账号登录任务失败，请重新登录。",
        _ if info.exists => "账号文件已存在。",
        _ => "未发现正在运行的登录进程。",
    };
    Ok(json!({
        "success": info.exists && info.valid_json,
        "loggedIn": logged_in,
        "running": false,
        "platform": account.platform,
        "account": account.profile,
        "status": persisted_status,
        "message": message,
        "accountFile": info.cookie_path,
        "accountExists": info.exists,
        "accountUpdatedAt": info.updated_at,
        "error": record.as_ref().and_then(|value| value.get("errorMessage")).cloned(),
        "lastLoginAt": record.as_ref().and_then(|value| value.get("lastLoginAt")).cloned(),
        "lastCheckAt": record.as_ref().and_then(|value| value.get("lastCheckAt")).cloned(),
        "stdout": "",
        "stderr": "",
    }))
}

async fn stop_login(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let account = account_from_payload(&payload, &config)?;
    let key = session_key(&account);
    if let Some(login) = active_login(&key).await {
        login.handle.lock().await.stop().await;
        update_account_status(&state.db, &account, "idle", None, false)?;
        let mut snapshot = snapshot_active(&login, &state.db).await?;
        remove_active_login_if_same(&key, &login).await;
        snapshot["stopSuccess"] = json!(true);
        snapshot["success"] = json!(true);
        snapshot["running"] = json!(false);
        snapshot["status"] = json!("stopped");
        snapshot["message"] = json!("登录进程已停止。");
        return Ok(snapshot);
    }
    Ok(json!({
        "success": true,
        "stopSuccess": true,
        "running": false,
        "status": "idle",
        "platform": account.platform,
        "account": account.profile,
        "accountFile": runtime.account_file(&account).to_string_lossy(),
        "message": "当前没有运行中的登录进程。",
        "stdout": "",
        "stderr": "",
    }))
}

async fn import_account(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let account = account_from_payload(&payload, &config)?;
    let content =
        opt_str(&payload, "content").ok_or_else(|| anyhow::anyhow!("缺少账号 JSON content"))?;
    if content.len() > 16 * 1024 * 1024 {
        anyhow::bail!("账号文件超过 16 MiB，已拒绝导入");
    }
    runtime.store().import_json(&account, content)?;
    let info = runtime.account_info(&account)?;
    sync_account_file_to_db(&state.db, &account, &info, "publisher", Some("saved"))?;
    Ok(json!({
        "success": true,
        "platform": account.platform,
        "account": account.profile,
        "accountFile": info.cookie_path,
        "accountExists": info.exists,
        "accountUpdatedAt": info.updated_at,
        "message": "账号文件已导入。",
    }))
}

async fn export_account(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let account = account_from_payload(&payload, &config)?;
    let content = runtime.store().export_json(&account)?;
    Ok(json!({
        "success": true,
        "platform": account.platform,
        "account": account.profile,
        "filename": account.cookie_filename(),
        "content": content,
    }))
}

async fn delete_account(payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let account = account_from_payload(&payload, &config)?;
    let key = session_key(&account);
    if let Some(login) = active_login(&key).await {
        login.handle.lock().await.stop().await;
        remove_active_login_if_same(&key, &login).await;
    }
    let deleted = runtime.store().delete(&account)?;
    let profile_deleted = runtime.delete_profile(&account)?;
    state.db.execute_json(
        "DELETE FROM platform_accounts WHERE platform=?1 AND profile=?2",
        &[json!(account.platform), json!(account.profile)],
    )?;
    Ok(json!({
        "success": true,
        "deleted": deleted,
        "profileDeleted": profile_deleted,
        "platform": account.platform,
        "account": account.profile,
        "message": if deleted || profile_deleted { "账号登录态与独立浏览器 profile 已删除。" } else { "账号登录态原本不存在。" },
    }))
}

async fn open_social_cookies_dir(state: &AppState) -> anyhow::Result<Value> {
    let status = base_status(state).await?;
    let config = status.get("config").cloned().unwrap_or_default();
    let runtime = runtime_from_config(&config);
    let path = runtime.cookies_dir();
    std::fs::create_dir_all(&path)?;
    let open_error = webbrowser::open(&format!("file://{}", path.to_string_lossy())).err();
    Ok(json!({
        "success": open_error.is_none(),
        "path": path.to_string_lossy(),
        "error": open_error.map(|error| error.to_string()),
    }))
}

async fn open_media_crawler_dir() -> anyhow::Result<Value> {
    let path = std::env::var_os("MEDIACRAWLER_BROWSER_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("YUNYING_DATA_DIR")
                .map(PathBuf::from)
                .map(|root| root.join("mediacrawler/browser_data"))
        })
        .unwrap_or_else(|| PathBuf::from("browser_data"));
    std::fs::create_dir_all(&path)?;
    let open_error = webbrowser::open(&format!("file://{}", path.to_string_lossy())).err();
    Ok(json!({
        "success": open_error.is_none(),
        "path": path.to_string_lossy(),
        "error": open_error.map(|error| error.to_string()),
    }))
}

async fn active_login(key: &str) -> Option<ActiveLogin> {
    ACTIVE_LOGINS.lock().await.get(key).cloned()
}

async fn remove_active_login_if_same(key: &str, expected: &ActiveLogin) {
    let mut active = ACTIVE_LOGINS.lock().await;
    let should_remove = active
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(&current.handle, &expected.handle));
    if should_remove {
        active.remove(key);
    }
}

async fn active_login_summaries() -> Vec<Value> {
    let logins = ACTIVE_LOGINS
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut summaries = Vec::with_capacity(logins.len());
    for login in logins {
        let state = login.handle.lock().await.snapshot().await;
        summaries.push(json!({
            "platform": login.account.platform,
            "account": login.account.profile,
            "running": state.running,
            "success": state.success,
            "status": state.status,
            "message": state.message,
            "startedAt": state.started_at,
            "finishedAt": state.finished_at,
            "qrcodeAvailable": state.qrcode_path.is_some(),
        }));
    }
    summaries
}

async fn snapshot_active(login: &ActiveLogin, db: &Db) -> anyhow::Result<Value> {
    let state = login.handle.lock().await.snapshot().await;
    let info = login.runtime.account_info(&login.account)?;
    let qrcode_url = state
        .qrcode_path
        .as_deref()
        .and_then(|path| image_data_url(path).ok());

    if state.success {
        sync_account_file_to_db(db, &login.account, &info, "publisher", Some("logged_in"))?;
        update_account_status(db, &login.account, "logged_in", None, true)?;
    } else if !state.running && state.status != "stopped" {
        let persisted_status = if state.status == "saved_unverified" {
            "saved_unverified"
        } else {
            "error"
        };
        update_account_status(
            db,
            &login.account,
            persisted_status,
            state.error.as_deref(),
            false,
        )?;
    }

    Ok(json!({
        "success": state.success,
        "loggedIn": state.success && state.status == "logged_in",
        "running": state.running,
        "platform": login.account.platform,
        "account": login.account.profile,
        "runtimeMode": "rust-cdp",
        "browserExecutable": login.runtime.browser_executable().map(|path| path.to_string_lossy().into_owned()),
        "currentUrl": state.current_url,
        "startedAt": state.started_at,
        "finishedAt": state.finished_at,
        "stdout": "",
        "stderr": "",
        "accountFile": info.cookie_path,
        "accountExists": info.exists,
        "accountUpdatedAt": info.updated_at,
        "qrcodePath": state.qrcode_path.as_ref().map(|path| path.to_string_lossy().into_owned()),
        "qrcodeUrl": qrcode_url,
        "status": state.status,
        "message": state.message,
        "error": state.error,
    }))
}

fn sync_account_file_to_db(
    db: &Db,
    account: &Account,
    info: &AccountFileInfo,
    kind: &str,
    status_override: Option<&str>,
) -> anyhow::Result<()> {
    let raw = if info.exists && info.valid_json {
        std::fs::read_to_string(&info.cookie_path).ok()
    } else {
        None
    };
    let status = status_override.unwrap_or(if info.exists { "saved" } else { "idle" });
    let id = format!("{}_{}", account.platform, account.profile);
    let now = now_ts();
    db.execute_json(
        "INSERT INTO platform_accounts \
         (id,platform,profile,kind,cookies,status,created_at,updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?7) \
         ON CONFLICT(platform,profile) DO UPDATE SET \
           kind=excluded.kind, \
           cookies=COALESCE(excluded.cookies,platform_accounts.cookies), \
           status=CASE WHEN excluded.status='saved' \
                            AND platform_accounts.status NOT IN ('idle','saved') \
                            AND excluded.cookies IS NOT NULL \
                       THEN platform_accounts.status ELSE excluded.status END, \
           updated_at=excluded.updated_at",
        &[
            json!(id),
            json!(account.platform),
            json!(account.profile),
            json!(kind),
            raw.map(Value::String).unwrap_or(Value::Null),
            json!(status),
            json!(now),
        ],
    )?;
    Ok(())
}

fn update_account_status(
    db: &Db,
    account: &Account,
    status: &str,
    error_message: Option<&str>,
    login_succeeded: bool,
) -> anyhow::Result<()> {
    let now = now_ts();
    let last_login = if login_succeeded {
        json!(now)
    } else {
        Value::Null
    };
    db.execute_json(
        "INSERT INTO platform_accounts \
         (id,platform,profile,kind,status,error_message,last_login_at,last_check_at,created_at,updated_at) \
         VALUES (?1,?2,?3,'publisher',?4,?5,?6,?7,?7,?7) \
         ON CONFLICT(platform,profile) DO UPDATE SET \
           status=excluded.status, error_message=excluded.error_message, \
           last_login_at=COALESCE(excluded.last_login_at,platform_accounts.last_login_at), \
           last_check_at=excluded.last_check_at, updated_at=excluded.updated_at",
        &[
            json!(format!("{}_{}", account.platform, account.profile)),
            json!(account.platform),
            json!(account.profile),
            json!(status),
            error_message.map(|value| json!(value)).unwrap_or(Value::Null),
            last_login,
            json!(now),
        ],
    )?;
    Ok(())
}

fn image_data_url(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    let mime = match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goose_bridge::GooseBridge;
    use crate::ipc::NoopEmitter;
    use std::sync::Arc;

    #[test]
    fn fallback_social_root_uses_data_dir() {
        let path = fallback_social_data_root();
        assert!(path.ends_with("social-connection"));
    }

    #[tokio::test]
    async fn native_runtime_status_import_export_delete_contract() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("social-data");
        let browser = temp.path().join("chrome");
        std::fs::write(&browser, b"browser").unwrap();

        let db = Db::open_in_memory().unwrap();
        let config = json!({
            "version": 1,
            "socialConnection": {
                "enabled": true,
                "runtimeMode": "rust-cdp",
                "browserExecutable": browser.to_string_lossy(),
                "dataDir": data_root.to_string_lossy(),
                "headless": true,
                "proxyUrl": "",
                "accounts": { "douyin": "test_profile" }
            }
        });
        db.settings()
            .save(&json!({ "social_tools_json": config.to_string() }))
            .unwrap();
        let state = AppState {
            redclaw_scheduler: crate::ipc::redclaw_runner::RedClawScheduler::inactive(db.clone()),
            db,
            goose: GooseBridge::default(),
            emitter: Arc::new(NoopEmitter),
            login: Arc::new(crate::login::LoginService::new(Arc::new(
                crate::login::StubLoginDriver,
            ))),
        };

        let status = invoke("social-tools:get-status", Value::Null, &state)
            .await
            .unwrap();
        assert_eq!(status["socialConnection"]["runtimeMode"], "rust-cdp");
        assert_eq!(status["socialConnection"]["browserAvailable"], true);

        let imported = invoke(
            "social-tools:import-account",
            json!({
                "platform": "youtube",
                "account": "imported",
                "content": json!({ "cookies": [], "origins": [] }).to_string()
            }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(imported["success"], true);

        let second = invoke(
            "social-tools:import-account",
            json!({
                "platform": "youtube",
                "account": "team_second",
                "content": json!({ "cookies": [], "origins": [] }).to_string()
            }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(second["success"], true);

        let multi_account_status = invoke("social-tools:get-status", Value::Null, &state)
            .await
            .unwrap();
        let youtube_accounts = multi_account_status["socialConnection"]["discoveredAccounts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["platform"] == "youtube")
            .map(|entry| entry["account"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(youtube_accounts.contains(&"imported"));
        assert!(youtube_accounts.contains(&"team_second"));
        assert!(multi_account_status["socialConnection"]["accountRecords"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["platform"] == "youtube" && entry["account"] == "team_second"));

        let exported = invoke(
            "social-tools:export-account",
            json!({ "platform": "youtube", "account": "imported" }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(exported["success"], true);
        assert!(exported["content"].as_str().unwrap().contains("cookies"));

        let stopped = invoke(
            "social-tools:stop-login",
            json!({ "platform": "youtube", "account": "imported" }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(stopped["stopSuccess"], true);
        assert_eq!(stopped["running"], false);
        assert_eq!(stopped["status"], "idle");

        let deleted = invoke(
            "social-tools:delete-account",
            json!({ "platform": "youtube", "account": "imported" }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(deleted["success"], true);
        assert_eq!(deleted["deleted"], true);
    }
}
