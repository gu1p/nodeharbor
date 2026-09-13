use async_trait::async_trait;
use nodeharbor_agent::{CommandOutput, Runner, Vm};
use nodeharbor_core::Resources;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";
struct ScriptedRunner {
    outputs: Mutex<VecDeque<CommandOutput>>,
    calls: Mutex<Vec<Vec<String>>>,
}
impl ScriptedRunner {
    fn new(outputs: Vec<CommandOutput>) -> Arc<Self> {
        Arc::new(Self {
            outputs: Mutex::new(outputs.into()),
            calls: Mutex::new(vec![]),
        })
    }
}
#[async_trait]
impl Runner for ScriptedRunner {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        Ok(self
            .outputs
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected operation"))
    }
}
fn ok(stdout: String) -> CommandOutput {
    CommandOutput {
        success: true,
        stdout,
        stderr: String::new(),
    }
}
fn budget() -> Resources {
    Resources {
        cpus: 2,
        memory_mib: 3072,
        disk_gib: 20,
    }
}

#[tokio::test]
async fn a_failed_launch_remains_owned_and_can_be_stopped_without_guest_networking() {
    let directory = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::new(vec![
        ok(json!({"list":[]}).to_string()),
        CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "waiting for an IP address timed out".into(),
        },
        ok(String::new()),
    ]);
    let vm = Vm::managed(ID, directory.path(), runner.clone()).unwrap();
    assert!(vm
        .create(&budget(), directory.path(), json!([]))
        .await
        .is_err());
    // A fresh application process must recover from the persisted creation receipt.
    let recovered = Vm::managed(ID, directory.path(), runner.clone()).unwrap();
    assert!(recovered.has_receipt().unwrap());
    recovered.stop().await.unwrap();
    assert_eq!(runner.calls.lock().unwrap()[2], ["stop", NAME]);
}

#[tokio::test]
async fn an_existing_unowned_vm_is_neither_claimed_nor_started() {
    let directory = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::new(vec![ok(
        json!({"list":[{"name":NAME,"state":"Stopped"}]}).to_string()
    )]);
    let vm = Vm::managed(ID, directory.path(), runner.clone()).unwrap();
    assert!(vm
        .create(&budget(), directory.path(), json!([]))
        .await
        .is_err());
    assert!(!vm.has_receipt().unwrap());
    assert!(vm.start().await.is_err());
    assert_eq!(runner.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_unknown_vm_state_is_not_reported_as_stopped() {
    let runner = ScriptedRunner::new(vec![ok(
        json!({"list":[{"name":NAME,"state":"Unknown"}]}).to_string()
    )]);
    let vm = Vm::new(ID, runner).unwrap();
    let state = vm.info().await.unwrap();
    assert!(
        state.running,
        "An uncertain hypervisor state must prevent an unsafe update"
    );
    assert!(!state.reachable);
}

#[tokio::test]
async fn stop_recovers_a_partial_worker_even_when_the_controller_is_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::new(vec![
        ok(json!({"list":[]}).to_string()),
        CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "no guest network".into(),
        },
        ok(json!({"list":[{"name":NAME,"state":"Unknown"}]}).to_string()),
        ok(String::new()),
    ]);
    let vm = Vm::managed(ID, directory.path(), runner.clone()).unwrap();
    assert!(vm
        .create(&budget(), directory.path(), json!([]))
        .await
        .is_err());
    let agent =
        nodeharbor_agent::Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("unreachable-controller-test-token".into());
            config.controller_url = Some("http://127.0.0.1:9".into());
            Ok(())
        })
        .unwrap();
    agent.action("stop").await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), agent.tick())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        runner.calls.lock().unwrap().last().unwrap(),
        // The explicit Stop now action uses Multipass's immediate shutdown.
        &["stop", "--force", NAME]
    );
    assert!(!agent.snapshot().await.unwrap().worker.running);
}

#[tokio::test]
async fn a_paused_partial_worker_with_unknown_state_is_reconciled_through_immediate_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let runner = ScriptedRunner::new(vec![
        ok(json!({"list":[]}).to_string()),
        CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "guest network unavailable".into(),
        },
        ok(json!({"list":[{"name":NAME,"state":"Unknown"}]}).to_string()),
        ok(String::new()),
    ]);
    let vm = Vm::managed(ID, directory.path(), runner.clone()).unwrap();
    assert!(vm
        .create(&budget(), directory.path(), json!([]))
        .await
        .is_err());
    let agent =
        nodeharbor_agent::Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-token".into());
            config.policy.enabled = false;
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    assert_eq!(
        runner.calls.lock().unwrap().last().unwrap(),
        &["stop", "--force", NAME]
    );
}
