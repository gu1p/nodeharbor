use async_trait::async_trait;
use nodeharbor_agent::{guest_files, CommandOutput, Runner, Vm, VmProvider};
use nodeharbor_core::Resources;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
#[derive(Default)]
struct LimaHost {
    state: Mutex<Option<String>>,
    calls: Mutex<Vec<Vec<String>>>,
    configuration: Mutex<Option<Value>>,
    fail_start: Mutex<bool>,
    wrong_owner: Mutex<bool>,
}
#[async_trait]
impl Runner for LimaHost {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        let mut state = self.state.lock().unwrap();
        let stdout = match args[0].as_str() {
            "list" => {
                assert!(args.iter().any(|arg| arg == "--filter=.name == \"worker\""));
                state
                    .as_ref()
                    .map(|s| json!({"name":"worker","status":s}).to_string())
                    .unwrap_or_default()
            }
            "start" => {
                if args.iter().any(|a| a == "--name=worker") {
                    let file = args.last().unwrap();
                    *self.configuration.lock().unwrap() =
                        Some(serde_json::from_slice(&std::fs::read(file)?)?);
                }
                *state = Some("Running".into());
                if *self.fail_start.lock().unwrap() {
                    anyhow::bail!("VM startup interrupted");
                }
                String::new()
            }
            "shell" => {
                assert!(args.iter().any(|a| a == "--workdir=/"));
                assert!(args.iter().any(|a| a == "worker"));
                if args
                    .last()
                    .is_some_and(|a| a == "/etc/nodeharbor/device-id")
                {
                    if *self.wrong_owner.lock().unwrap() {
                        "another-device".into()
                    } else {
                        ID.into()
                    }
                } else {
                    String::new()
                }
            }
            "stop" => {
                *state = Some("Stopped".into());
                String::new()
            }
            "edit" => {
                assert_eq!(state.as_deref(), Some("Stopped"));
                String::new()
            }
            "delete" => {
                assert_eq!(state.as_deref(), Some("Stopped"));
                *state = None;
                String::new()
            }
            _ => panic!("Unexpected native Lima operation: {args:?}"),
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
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
async fn lima_preparation_uses_native_networking_without_host_files_or_personal_keys() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(LimaHost::default());
    let vm = Vm::managed(ID, dir.path(), host.clone()).unwrap();
    assert!(!vm.info().await.unwrap().installed);
    vm.create(&budget(), dir.path(), guest_files())
        .await
        .unwrap();
    assert!(vm.info().await.unwrap().reachable);
    let config = host.configuration.lock().unwrap().clone().unwrap();
    assert_eq!(
        config["vmType"],
        if cfg!(target_os = "macos") {
            "vz"
        } else {
            "qemu"
        }
    );
    assert_eq!(config["plain"], true);
    assert_eq!(config["ssh"]["overVsock"], false);
    assert_eq!(config["networks"], json!([{"lima":"user-v2"}]));
    assert_eq!(config["mounts"], json!([]));
    assert_eq!(config["ssh"]["forwardAgent"], false);
    assert_eq!(config["ssh"]["loadDotSSHPubKeys"], false);
    assert_eq!(config["containerd"], json!({"user":false,"system":false}));
    assert_eq!(config["portForwards"][0]["ignore"], true);
    assert_eq!(config["portForwards"][0]["guestIPMustBeZero"], false);
    assert_eq!(config["cpus"], 2);
    assert_eq!(config["memory"], "3072MiB");
    assert_eq!(config["disk"], "20GiB");
    assert!(config["images"][0]["digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    let data = config["provision"].as_array().unwrap();
    assert_eq!(data.len(), 6);
    for entry in data {
        assert_eq!(entry["mode"], "data");
        assert_eq!(entry["overwrite"], false);
    }
    let owner = data
        .iter()
        .find(|v| v["path"] == "/etc/nodeharbor/device-id")
        .unwrap();
    assert_eq!(owner["content"], ID);
    vm.stop().await.unwrap();
    vm.resize(&budget()).await.unwrap();
    vm.start().await.unwrap();
    vm.stop_now().await.unwrap();
    vm.remove().await.unwrap();
    assert!(!vm.has_receipt().unwrap());
    assert!(!vm.info().await.unwrap().installed);
    let calls = host.calls.lock().unwrap();
    assert!(calls
        .iter()
        .any(|a| a[0] == "stop" && a.iter().any(|v| v == "--force")));
    assert!(calls
        .iter()
        .any(|a| a[0] == "edit" && a.iter().any(|v| v == "--memory=3")));
    assert!(!calls
        .iter()
        .any(|a| matches!(a[0].as_str(), "exec" | "launch" | "set")));
}

#[tokio::test]
async fn failed_lima_creation_keeps_an_ownership_receipt_and_can_be_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(LimaHost::default());
    *host.fail_start.lock().unwrap() = true;
    let vm = Vm::managed(ID, dir.path(), host.clone()).unwrap();
    assert!(vm
        .create(&budget(), dir.path(), guest_files())
        .await
        .is_err());
    assert!(vm.has_receipt().unwrap());
    vm.stop_now().await.unwrap();
    assert!(vm.info().await.unwrap().stopped);
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("worker.receipt.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["provider"], "lima");
    assert_eq!(receipt["deviceId"], ID);
}

#[tokio::test]
async fn an_existing_lima_instance_cannot_be_adopted_or_deleted_without_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(LimaHost::default());
    *host.state.lock().unwrap() = Some("Stopped".into());
    let vm = Vm::managed(ID, dir.path(), host.clone()).unwrap();
    assert!(vm
        .create(&budget(), dir.path(), guest_files())
        .await
        .is_err());
    assert!(vm.start().await.is_err());
    assert!(vm.remove().await.is_err());
    assert!(!host
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|a| a[0] == "delete" || a[0] == "start"));
    std::fs::write(
        dir.path().join("worker.receipt.json"),
        json!({"version":1,"name":"worker","deviceId":ID}).to_string(),
    )
    .unwrap();
    assert!(
        vm.has_receipt().is_err(),
        "A legacy receipt cannot authorize a different provider"
    );
}

#[test]
fn legacy_configuration_stays_on_multipass_and_new_provider_is_versioned() {
    let mut old = serde_json::to_value(nodeharbor_agent::Configuration::default()).unwrap();
    old.as_object_mut().unwrap().remove("vmProvider");
    let config: nodeharbor_agent::Configuration = serde_json::from_value(old).unwrap();
    assert_eq!(config.vm_provider, VmProvider::Multipass);
    let dir = tempfile::tempdir().unwrap();
    let store = nodeharbor_agent::Store::open(dir.path()).unwrap();
    store
        .update(|c| {
            c.vm_provider = VmProvider::Lima;
            c.format_version = 2;
            Ok(())
        })
        .unwrap();
    let saved = store.load().unwrap();
    assert_eq!(saved.vm_provider, VmProvider::Lima);
    let mut malformed = serde_json::to_value(saved).unwrap();
    malformed["formatVersion"] = json!(1);
    std::fs::write(dir.path().join("config.json"), malformed.to_string()).unwrap();
    assert!(
        store.load().is_err(),
        "Older apps must not misinterpret a Lima worker as Multipass"
    );
}
