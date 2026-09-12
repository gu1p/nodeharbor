use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nodeharbor_agent::{Agent, Store, Vm};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version, about = "NodeHarbor local worker supervisor")]
struct Args {
    #[arg(long, env = "NODEHARBOR_CONFIG_DIR")]
    config_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Status,
    Run,
    Pause,
    Resume,
    Prepare,
    Stop,
    /// Drain the owned VM before replacing the application. Fails on timeout.
    PrepareUpdate {
        #[arg(long, default_value_t = 2100)]
        timeout: u64,
    },
    Enroll {
        #[arg(long)]
        url: String,
        #[arg(long)]
        code_file: PathBuf,
    },
}

async fn wait_for_stop(agent: &Agent, timeout: u64) -> Result<()> {
    let config = agent.store.load()?;
    if !config.vm_created && !config.prepare_requested {
        return Ok(());
    }
    let vm = Vm::local(&config.device_id)?;
    let deadline = Instant::now() + Duration::from_secs(timeout);
    // If the desktop is running it owns supervision. Otherwise this command
    // takes over only long enough to drain the enrolled device's worker.
    let supervisor = agent.store.supervisor_lock().ok();
    loop {
        if supervisor.is_some() {
            agent.tick().await?;
        }
        let state = vm.info().await?;
        if !state.running {
            return Ok(());
        }
        anyhow::ensure!(Instant::now() < deadline, "The worker did not finish draining in time; the existing application must be preserved");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let directory = args
        .config_dir
        .map(Ok)
        .unwrap_or_else(Store::default_directory)?;
    let agent = Agent::open(&directory)?;
    match args.command {
        Command::Status => {
            let mut snapshot = agent.snapshot().await?;
            if agent.store.load()?.vm_created {
                snapshot.worker = Vm::local(&snapshot.device_id)?.info().await?;
                if snapshot.worker.running {
                    snapshot.state = "running".into();
                    snapshot.reason = "The local worker is running; the fleet dashboard reports its qualification".into();
                }
            }
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        Command::Run => {
            tokio::select! {
                result = agent.run() => result?,
                _ = tokio::signal::ctrl_c() => {
                    agent.action("pause").await?;
                    wait_for_stop(&agent,2100).await?;
                }
            }
        }
        Command::PrepareUpdate { timeout } => {
            anyhow::ensure!(
                (1..=3600).contains(&timeout),
                "Update drain timeout must be between 1 and 3600 seconds"
            );
            agent.action("pause").await?;
            wait_for_stop(&agent, timeout).await?;
            println!(
                "Worker stopped; the application can be updated without changing its enrollment."
            );
        }
        Command::Stop => {
            agent.action("stop").await?;
            wait_for_stop(&agent, 180).await?;
        }
        Command::Pause => {
            agent.action("pause").await?;
        }
        Command::Resume => {
            agent.action("resume").await?;
        }
        Command::Prepare => {
            agent.action("prepare").await?;
        }
        Command::Enroll { url, code_file } => {
            let code = std::fs::read_to_string(code_file)
                .context("Cannot read the one-time enrollment code")?;
            let snapshot = agent.enroll(&url, code.trim()).await?;
            println!("Enrolled {}. Sharing remains switched off.", snapshot.name);
        }
    }
    Ok(())
}
