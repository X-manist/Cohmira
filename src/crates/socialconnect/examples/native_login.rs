//! 手工诊断：启动纯 Rust 登录，等二维码/浏览器页面就绪后停止。

use socialconnect::account::Account;
use socialconnect::native::NativeRuntime;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let data_root = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("缺少 data-root"))?;
    let platform = args.next().unwrap_or_else(|| "douyin".to_string());
    let profile = args.next().unwrap_or_else(|| "default".to_string());
    let runtime = NativeRuntime::new(data_root, None::<PathBuf>, None::<String>);
    let account = Account::try_new(&platform, &profile)?;
    let mut handle = runtime.start_login(account);
    for _ in 0..80 {
        let state = handle.snapshot().await;
        if state.success || state.qrcode_path.is_some() || !state.running {
            println!("{}", serde_json::to_string_pretty(&state)?);
            handle.stop().await;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&handle.snapshot().await)?
    );
    handle.stop().await;
    Ok(())
}
