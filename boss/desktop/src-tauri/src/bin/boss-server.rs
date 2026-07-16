use boss_core::{load_environment, server, BossCore};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_environment();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("boss_core::server=info,boss_server=info")),
        )
        .init();
    server::serve_from_env(BossCore::open_default()?).await
}
