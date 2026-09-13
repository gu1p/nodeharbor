use async_trait::async_trait;
use nodeharbor_agent::{Agent, CommandOutput, OutputStream, ProgressSink, Runner};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;

#[derive(Default)]
struct WaitingForNetwork {
    entered: Notify,
    finish: Notify,
}
#[async_trait]
impl Runner for WaitingForNetwork {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        assert_eq!(args[0], "list");
        Ok(CommandOutput {
            success: true,
            stdout: "{\"list\":[]}".into(),
            stderr: String::new(),
        })
    }
    async fn run_with_progress(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
        progress: ProgressSink,
    ) -> anyhow::Result<CommandOutput> {
        assert_eq!(args[0], "launch");
        progress(
            OutputStream::Stderr,
            "Waiting for the VM to receive an IP address",
        );
        self.entered.notify_one();
        self.finish.notified().await;
        Ok(CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "VM launch timed out".into(),
        })
    }
}

#[tokio::test]
async fn preparing_exposes_progress_before_the_command_finishes_and_preserves_its_failure() {
    let directory = tempfile::tempdir().unwrap();
    let runner = Arc::new(WaitingForNetwork::default());
    let agent = Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_token = Some("private-test-device-credential".into());
            config.prepare_requested = true;
            config.policy.resources.cpus = 1;
            config.policy.resources.memory_mib = 2048;
            config.policy.resources.disk_gib = 15;
            Ok(())
        })
        .unwrap();
    let clone = agent.clone();
    let task = tokio::spawn(async move { clone.tick().await });
    tokio::time::timeout(Duration::from_secs(3), runner.entered.notified())
        .await
        .unwrap();
    let progress = agent.activity();
    assert_eq!(progress.step.unwrap().message, "Creating Ubuntu VM");
    assert!(progress
        .entries
        .iter()
        .any(|entry| entry.message.contains("receive an IP address")));
    assert!(
        !task.is_finished(),
        "Progress must be available while preparation is waiting"
    );
    runner.finish.notify_one();
    assert!(task.await.unwrap().is_err());
    let progress = agent.activity();
    assert!(progress.step.is_none());
    assert!(progress
        .entries
        .iter()
        .any(|entry| entry.level == "error" && entry.message.contains("VM launch timed out")));
    assert!(!serde_json::to_string(&progress)
        .unwrap()
        .contains("private-test-device-credential"));
}

struct ConfigurationFailure;
const DEVICE: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const VM_NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";
#[async_trait]
impl Runner for ConfigurationFailure {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let stdout = if args[0] == "list" {
            serde_json::json!({"list":[{"name":VM_NAME,"state":"Running"}]}).to_string()
        } else if args.iter().any(|arg| arg == "cat") {
            DEVICE.into()
        } else {
            String::new()
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
    async fn run_with_progress(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        _: u64,
        progress: ProgressSink,
    ) -> anyhow::Result<CommandOutput> {
        assert!(args
            .iter()
            .any(|arg| arg == "/usr/local/lib/nodeharbor/configure_worker.py"));
        let grant: serde_json::Value = serde_json::from_slice(&stdin.unwrap()).unwrap();
        progress(
            OutputStream::Stdout,
            "NodeHarbor step: Connecting to the private network",
        );
        progress(
            OutputStream::Stderr,
            &format!("Access denied for {}", grant["k3sToken"].as_str().unwrap()),
        );
        anyhow::bail!(
            "Connection refused for {}",
            grant["netbirdSetupKey"].as_str().unwrap()
        )
    }
}

#[tokio::test]
async fn configuration_progress_and_failures_are_visible_without_exposing_the_bootstrap_grant() {
    use std::future::IntoFuture;
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let router=axum::Router::new().route("/api/v1/device/bootstrap",axum::routing::post(||async{
        axum::Json(serde_json::json!({"k3sToken":"private-kubernetes-value","netbirdSetupKey":"private-netbird-value"}))
    }));
    let server = tokio::spawn(axum::serve(listener, router).into_future());
    let agent = Agent::open_with_runner(directory.path(), Arc::new(ConfigurationFailure)).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = DEVICE.into();
            config.device_token = Some("private-device-value".into());
            config.controller_url = Some(address);
            config.vm_created = true;
            config.prepare_requested = true;
            config.allocated_resources = Some(config.policy.resources.clone());
            Ok(())
        })
        .unwrap();
    std::fs::write(
        directory.path().join(format!("{VM_NAME}.receipt.json")),
        serde_json::json!({"version":1,"deviceId":DEVICE,"name":VM_NAME}).to_string(),
    )
    .unwrap();
    assert!(agent.tick().await.is_err());
    server.abort();
    let logs = serde_json::to_string(&agent.activity()).unwrap();
    assert!(logs.contains("Connecting to the private network"));
    assert!(logs.contains("Connection refused"));
    for secret in [
        "private-kubernetes-value",
        "private-netbird-value",
        "private-device-value",
    ] {
        assert!(!logs.contains(secret));
    }
    assert!(agent
        .activity()
        .entries
        .iter()
        .any(|entry| entry.level == "error"));
}
