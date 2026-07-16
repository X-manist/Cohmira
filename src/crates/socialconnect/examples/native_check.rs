//! 手工诊断：`cargo run -p socialconnect --example native_check -- <data-root> <platform> <profile> [browser]`。

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
    let platform = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("缺少 platform"))?;
    let profile = args.next().unwrap_or_else(|| "default".to_string());
    let browser = args.next().map(PathBuf::from);
    let runtime = NativeRuntime::new(data_root, browser, None::<String>);
    let account = Account::try_new(&platform, &profile)?;
    let result = runtime
        .check_account(&account, Duration::from_secs(120))
        .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    std::process::exit(if result.success { 0 } else { 2 });
}
