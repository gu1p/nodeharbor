use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nodeharbor_agent::{Agent, Store};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version = nodeharbor_core::build_version(), about = "NodeHarbor local worker supervisor")]
struct Args {
    #[arg(long, env = "NODEHARBOR_CONFIG_DIR")]
    config_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Status,
    /// Locally grant or revoke fleet administrators permission to edit node settings.
    RemoteConfiguration {
        #[arg(value_parser = ["allow", "revoke"])]
        permission: String,
    },
    Run,
    Pause,
    Resume,
    Prepare,
    Stop,
    /// Replace the owned worker through the normal drain and access-reset flow.
    ReplaceWorker {
        #[arg(long)]
        confirm_delete_disk: bool,
    },
    /// Wait for the previous application process before installing an update.
    WaitForAppExit {
        #[arg(long)]
        executable: PathBuf,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
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

async fn wait_for_app_exit(executable: &std::path::Path, timeout: u64) -> Result<()> {
    anyhow::ensure!(
        (1..=3600).contains(&timeout),
        "Application exit timeout must be between 1 and 3600 seconds"
    );
    anyhow::ensure!(
        executable.is_absolute(),
        "Use the previous application's absolute executable path"
    );
    let executable = executable
        .canonicalize()
        .context("Cannot identify the previous application executable")?;
    let own_pid = sysinfo::get_current_pid().map_err(anyhow::Error::msg)?;
    let mut system = sysinfo::System::new();
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
        );
        let running = system.processes().iter().any(|(pid, process)| {
            *pid != own_pid
                // Linux also enumerates tasks. Only process leaders represent
                // applications; a helper's own threads have different task IDs.
                && process.thread_kind().is_none()
                && process
                    .exe()
                    .and_then(|path| path.canonicalize().ok())
                    .as_ref()
                    == Some(&executable)
        });
        if !running {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "The previous application is still running; its installation must be preserved"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_for_stop(agent: &Agent, timeout: u64) -> Result<()> {
    let config = agent.store.load()?;
    let vm = agent.local_vm()?;
    if !config.vm_created && !config.prepare_requested && !vm.has_receipt()? {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_secs(timeout);
    // If the desktop is running it owns supervision. Otherwise this command
    // takes over only long enough to drain the enrolled device's worker.
    let supervisor = agent.store.supervisor_lock().ok();
    loop {
        let state = vm.info().await?;
        if !state.running {
            return Ok(());
        }
        if supervisor.is_some() {
            agent.tick().await?;
        }
        anyhow::ensure!(Instant::now() < deadline, "The worker did not finish draining in time; the existing application must be preserved");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn supervise_with_logs(agent: &Agent) -> Result<()> {
    use std::io::Write;
    let supervision = agent.run();
    tokio::pin!(supervision);
    let mut refresh = tokio::time::interval(Duration::from_secs(1));
    let mut last_id = 0;
    loop {
        tokio::select! {
            result=&mut supervision=>return result,
            _=refresh.tick()=>{
                for entry in agent.activity().entries {
                    if entry.id>last_id {
                        // These are the same bounded, redacted entries shown in
                        // the desktop. A closed log sink must not stop supervision.
                        let _=writeln!(std::io::stderr().lock(),"{}",serde_json::to_string(&entry)?);
                        last_id=entry.id;
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if let Command::WaitForAppExit {
        executable,
        timeout,
    } = &args.command
    {
        return wait_for_app_exit(executable, *timeout).await;
    }
    let directory = args
        .config_dir
        .map(Ok)
        .unwrap_or_else(Store::default_directory)?;
    let agent = Agent::open(&directory)?;
    match args.command {
        Command::WaitForAppExit { .. } => unreachable!(),
        Command::RemoteConfiguration { permission } => {
            agent.set_remote_consent(permission == "allow").await?;
            println!("Remote configuration permission: {permission}");
        }
        Command::Status => {
            let mut snapshot = agent.snapshot().await?;
            if agent.store.load()?.vm_created {
                snapshot.worker = agent.local_vm()?.info().await?;
                if snapshot.worker.running {
                    snapshot.state = "running".into();
                    snapshot.reason = "The local worker is running; the fleet dashboard reports its qualification".into();
                }
            }
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        Command::Run => {
            tokio::select! {
                result = supervise_with_logs(&agent) => result?,
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
        Command::ReplaceWorker {
            confirm_delete_disk,
        } => {
            anyhow::ensure!(confirm_delete_disk,"Replacement permanently deletes the owned worker disk. Supply --confirm-delete-disk to request it");
            agent.recreate_worker(agent.store.load()?.policy).await?;
            println!("Worker replacement requested. Enrollment and sharing rules are preserved; sharing remains off.");
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

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use nodeharbor_agent::{CommandOutput, Runner};
    use serde_json::json;
    use std::sync::Arc;

    struct Worker(bool);
    #[async_trait::async_trait]
    impl Runner for Worker {
        async fn run(&self, args: &[String], _: Option<Vec<u8>>, _: u64) -> Result<CommandOutput> {
            anyhow::ensure!(
                args[0] == "list",
                "Shutdown verification must only inspect this worker"
            );
            Ok(CommandOutput {
                success: true,
                stdout: json!({"list":[{"name":"nodeharbor-9511182e9c484d20a15b1da8bb441386","state":if self.0 {"Running"} else {"Stopped"}}]}).to_string(),
                stderr: String::new(),
            })
        }
    }
    fn fixture(directory: &std::path::Path, running: bool) -> Agent {
        let agent = Agent::open_with_runner(directory, Arc::new(Worker(running))).unwrap();
        agent.store.update(|config| {
            config.format_version = 2;
            config.device_id = "9511182e-9c48-4d20-a15b-1da8bb441386".into();
            config.vm_created = true;
            config.policy.enabled = false;
            config.device_token = Some("test-device-credential".into());
            config.controller_url = Some("http://127.0.0.1:1".into());
            config.recreation = Some(serde_json::from_value(json!({"requestId":"67345f21-3878-4d50-82aa-19bca630407e","accessRemoved":false,"targetProvider":"lima"}))?);
            Ok(())
        }).unwrap();
        std::fs::write(directory.join("nodeharbor-9511182e9c484d20a15b1da8bb441386.receipt.json"),
            json!({"version":1,"deviceId":"9511182e-9c48-4d20-a15b-1da8bb441386","name":"nodeharbor-9511182e9c484d20a15b1da8bb441386"}).to_string()).unwrap();
        agent
    }

    #[tokio::test]
    async fn a_stopped_worker_can_update_while_controller_replacement_cleanup_is_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let agent = fixture(directory.path(), false);
        let before = std::fs::read(directory.path().join("config.json")).unwrap();
        wait_for_stop(&agent, 1).await.unwrap();
        assert_eq!(
            before,
            std::fs::read(directory.path().join("config.json")).unwrap()
        );
        assert!(agent.local_vm().unwrap().has_receipt().unwrap());
    }

    #[tokio::test]
    async fn a_running_worker_still_prevents_an_update_until_its_supervisor_stops_it() {
        let directory = tempfile::tempdir().unwrap();
        let agent = fixture(directory.path(), true);
        let _supervisor = agent.store.supervisor_lock().unwrap();
        assert!(wait_for_stop(&agent, 1)
            .await
            .unwrap_err()
            .to_string()
            .contains("did not finish draining"));
        assert!(agent.local_vm().unwrap().has_receipt().unwrap());
    }
}
