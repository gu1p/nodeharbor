use async_trait::async_trait;
use nodeharbor_agent::{Agent, CommandOutput, Runner, Vm};
use nodeharbor_core::Resources;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";
struct Host {
    running: AtomicBool,
    calls: Mutex<Vec<Vec<String>>>,
}
#[async_trait]
impl Runner for Host {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        let stdout = if args[0] == "list" {
            json!({"list":[{"name":NAME,"state":if self.running.load(Ordering::SeqCst){"Running"}else{"Stopped"}}]})
                .to_string()
        } else if args
            .last()
            .is_some_and(|a| a == "/etc/nodeharbor/device-id")
        {
            ID.to_string()
        } else {
            String::new()
        };
        if args[0] == "stop" {
            self.running.store(false, Ordering::SeqCst);
        }
        if args[0] == "start" {
            self.running.store(true, Ordering::SeqCst);
        }
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}
fn receipt(dir: &std::path::Path) {
    std::fs::write(
        dir.join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
}
fn budget() -> Resources {
    Resources {
        cpus: 2,
        memory_mib: 3072,
        disk_gib: 25,
    }
}
#[tokio::test]
async fn a_saved_resource_budget_is_applied_to_a_stopped_worker_without_enabling_sharing() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host {
        running: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
    });
    let agent = Agent::open_with_runner(dir.path(), host.clone()).unwrap();
    receipt(dir.path());
    agent
        .store
        .update(|c| {
            c.device_id = ID.into();
            c.device_token = Some("test-token".into());
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 20,
            });
            c.policy.resources = budget();
            c.policy.enabled = false;
            Ok(())
        })
        .unwrap();
    let _ = agent.tick().await;
    assert_eq!(
        agent.store.load().unwrap().allocated_resources,
        Some(budget())
    );
    assert!(!agent.store.load().unwrap().policy.enabled);
    let calls = host.calls.lock().unwrap();
    assert!(calls
        .iter()
        .any(|a| a == &["set", &format!("local.{NAME}.cpus=2")]));
    assert!(calls
        .iter()
        .any(|a| a == &["set", &format!("local.{NAME}.memory=3072M")]));
    assert!(calls
        .iter()
        .any(|a| a == &["set", &format!("local.{NAME}.disk=25G")]));
    assert!(!calls
        .iter()
        .any(|a| ["start", "launch", "delete"].contains(&a[0].as_str())));
}
#[tokio::test]
async fn resources_are_never_changed_on_a_running_or_unowned_vm() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host {
        running: AtomicBool::new(true),
        calls: Mutex::new(vec![]),
    });
    let vm = Vm::managed(ID, dir.path(), host.clone()).unwrap();
    assert!(vm.resize(&budget()).await.is_err());
    assert!(host.calls.lock().unwrap().is_empty());
    receipt(dir.path());
    assert!(vm.resize(&budget()).await.is_err());
    assert!(!host.calls.lock().unwrap().iter().any(|a| a[0] == "set"));
}

#[tokio::test]
async fn changing_a_running_workers_budget_drains_and_stops_it_before_resizing() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host {
        running: AtomicBool::new(true),
        calls: Mutex::new(vec![]),
    });
    let agent = Agent::open_with_runner(dir.path(), host.clone()).unwrap();
    receipt(dir.path());
    agent
        .store
        .update(|c| {
            c.device_id = ID.into();
            c.device_token = Some("test-token".into());
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 20,
            });
            c.policy.resources = budget();
            c.policy.enabled = false;
            c.policy.drain_seconds = 0;
            Ok(())
        })
        .unwrap();
    for _ in 0..3 {
        let _ = agent.tick().await;
    }
    assert_eq!(
        agent.store.load().unwrap().allocated_resources,
        Some(budget())
    );
    let calls = host.calls.lock().unwrap();
    let stopped = calls.iter().position(|a| a[0] == "stop").unwrap();
    let resized = calls.iter().position(|a| a[0] == "set").unwrap();
    assert!(stopped < resized);
}

#[tokio::test]
async fn retrying_partial_preparation_applies_the_budget_before_starting_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host {
        running: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
    });
    let agent = Agent::open_with_runner(dir.path(), host.clone()).unwrap();
    receipt(dir.path());
    agent
        .store
        .update(|c| {
            c.device_id = ID.into();
            c.device_token = Some("test-token".into());
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.vm_created = true;
            c.vm_configured = false;
            c.allocated_resources = Some(Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 20,
            });
            c.policy.resources = budget();
            c.prepare_requested = true;
            Ok(())
        })
        .unwrap();
    assert!(
        agent.tick().await.is_err(),
        "The unavailable controller prevents final configuration"
    );
    let calls = host.calls.lock().unwrap();
    let resized = calls
        .iter()
        .position(|a| a[0] == "set")
        .expect("Apply the changed budget, do not merely record it");
    let started = calls.iter().position(|a| a[0] == "start").unwrap();
    assert!(resized < started);
    assert_eq!(
        agent.store.load().unwrap().allocated_resources,
        Some(budget())
    );
    assert!(!agent.store.load().unwrap().vm_configured);
}

#[tokio::test]
async fn preparing_an_already_configured_worker_does_not_bypass_the_normal_drain() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host {
        running: AtomicBool::new(true),
        calls: Mutex::new(vec![]),
    });
    let agent = Agent::open_with_runner(dir.path(), host.clone()).unwrap();
    receipt(dir.path());
    agent
        .store
        .update(|c| {
            c.device_id = ID.into();
            c.device_token = Some("test-token".into());
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 20,
            });
            c.policy.resources = budget();
            c.prepare_requested = true;
            c.policy.drain_seconds = 300;
            Ok(())
        })
        .unwrap();
    let _ = agent.tick().await;
    assert_eq!(agent.snapshot().await.unwrap().state, "draining");
    assert_eq!(
        agent
            .store
            .load()
            .unwrap()
            .allocated_resources
            .unwrap()
            .cpus,
        1
    );
    assert!(!host
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|a| ["set", "stop", "start", "launch"].contains(&a[0].as_str())));
}
