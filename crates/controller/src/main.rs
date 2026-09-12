use clap::Parser;
use nodeharbor_controller::{router, State};
use std::{net::SocketAddr, path::PathBuf};
use tower_http::services::{ServeDir, ServeFile};
#[derive(Parser)]
#[command(version, about = "NodeHarbor fleet controller")]
struct Args {
    #[arg(long, env = "NODEHARBOR_LISTEN", default_value = "127.0.0.1:8090")]
    listen: SocketAddr,
    #[arg(
        long,
        env = "NODEHARBOR_DATABASE",
        default_value = "sqlite://nodeharbor.db"
    )]
    database: String,
    #[arg(long, env = "NODEHARBOR_ADMIN_TOKEN_FILE")]
    admin_token_file: PathBuf,
    #[arg(long, env = "NODEHARBOR_UI_DIR", default_value = "ui/dist")]
    ui_dir: PathBuf,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let token = std::fs::read_to_string(&args.admin_token_file)?;
    let state = State::open(&args.database, token.trim()).await?;
    let app = router(state).fallback_service(
        ServeDir::new(&args.ui_dir)
            .not_found_service(ServeFile::new(args.ui_dir.join("index.html"))),
    );
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(listen=%args.listen,"controller listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
