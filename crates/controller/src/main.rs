use clap::Parser;
use nodeharbor_controller::{
    configure_runtime, metrics_router, router, ConfiguredController, State, Telemetry,
};
use std::{net::SocketAddr, path::PathBuf};
use tower_http::services::{ServeDir, ServeFile};
#[derive(Parser)]
#[command(version = nodeharbor_core::build_version(), about = "NodeHarbor fleet controller")]
struct Args {
    #[arg(long, env = "NODEHARBOR_LISTEN", default_value = "127.0.0.1:8090")]
    listen: SocketAddr,
    /// Private metrics listener. Disabled unless explicitly configured.
    #[arg(long, env = "NODEHARBOR_METRICS_LISTEN")]
    metrics_listen: Option<SocketAddr>,
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
    #[arg(long, env = "NODEHARBOR_CONFIG_FILE")]
    config_file: Option<PathBuf>,
    #[arg(long, requires = "config_file")]
    check_config: bool,
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
    let telemetry = tokio::task::spawn_blocking(Telemetry::from_env).await??;
    let state = State::open(&args.database, token.trim())
        .await?
        .with_telemetry(telemetry.clone());
    let configured = if let Some(path) = &args.config_file {
        configure_runtime(state, path).await?
    } else {
        ConfiguredController {
            state,
            provisioner: None,
            reconciler: None,
        }
    };
    if args.check_config {
        println!("Controller configuration is valid");
        tokio::task::spawn_blocking(move || telemetry.shutdown()).await??;
        return Ok(());
    }
    let app = router(configured.state.clone()).fallback_service(
        ServeDir::new(&args.ui_dir)
            .not_found_service(ServeFile::new(args.ui_dir.join("index.html"))),
    );
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let (metrics_stop, mut metrics_stopped) = tokio::sync::watch::channel(false);
    let metrics = if let Some(address) = args.metrics_listen {
        let listener = tokio::net::TcpListener::bind(address).await?;
        let app = metrics_router(configured.state.clone());
        Some(tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = metrics_stopped.changed().await;
                })
                .await
        }))
    } else {
        None
    };
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tracing::info!(listen=%args.listen,"controller listening");
    let maintenance = tokio::spawn(configured.maintain());
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = terminate.recv()=>{}}
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    maintenance.abort();
    let _ = maintenance.await;
    let _ = metrics_stop.send(true);
    let metrics_result = if let Some(task) = metrics {
        Some(task.await)
    } else {
        None
    };
    let flushed = tokio::task::spawn_blocking(move || telemetry.shutdown()).await;
    result?;
    if let Some(result) = metrics_result {
        result??;
    }
    flushed??;
    Ok(())
}
