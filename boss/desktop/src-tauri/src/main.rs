#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use boss_core::{load_environment, server, BossCore};
use serde_json::Value;
use tauri::{Manager, State};

#[tauri::command]
async fn call_boss_tool(
    state: State<'_, BossCore>,
    name: String,
    args: Option<Value>,
) -> Result<Value, String> {
    let core = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        core.dispatch_tool(
            &name,
            args.unwrap_or_else(|| Value::Object(Default::default())),
        )
    })
    .await
    .map_err(|error| format!("老板工具任务异常：{error}"))?
    .map_err(|error| error.to_string())
}

fn main() {
    load_environment();
    tauri::Builder::default()
        .setup(|app| {
            let core = if std::env::var_os("BOSS_ACCOUNTING_DB").is_some() {
                BossCore::open_default()
            } else {
                let data_dir = app.path().app_data_dir()?;
                BossCore::open(data_dir.join("boss-accounting.sqlite"))
            }
            .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?;
            let http_core = core.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = server::serve_from_env(http_core).await {
                    eprintln!(
                        "[老板 HTTP 服务] 启动或运行失败：{error}；桌面 UI 将继续运行，员工远程同步暂不可用"
                    );
                }
            });
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call_boss_tool])
        .run(tauri::generate_context!())
        .expect("运营老板助手启动失败");
}
