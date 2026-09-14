#![cfg(any(target_os = "macos", target_os = "linux"))]
use nodeharbor_agent::{storage::Location, CommandOutput, Runner, Vm, VmProvider};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const OWNER: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
#[derive(Default)]
struct Runtime {
    config: Mutex<Option<Value>>,
}
#[async_trait::async_trait]
impl Runner for Runtime {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let output = match args[0].as_str() {
            "list" => String::new(),
            "start" => {
                *self.config.lock().unwrap() = Some(serde_json::from_slice(&std::fs::read(
                    args.last().unwrap(),
                )?)?);
                String::new()
            }
            "shell" => OWNER.into(),
            _ => anyhow::bail!("Unexpected runtime command"),
        };
        Ok(CommandOutput {
            success: true,
            stdout: output,
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn pooled_worker_attaches_both_disks_without_automatic_formatting_or_host_mounts() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Arc::new(Runtime::default());
    let vm = Vm::managed(OWNER, directory.path(), runtime.clone()).unwrap();
    let locations = vec![
        Location {
            id: "nhone".into(),
            volume_id: "first".into(),
            directory: "/first".into(),
            allocation_gib: 16,
        },
        Location {
            id: "nhtwo".into(),
            volume_id: "second".into(),
            directory: "/second".into(),
            allocation_gib: 16,
        },
    ];
    vm.create_with_storage(
        &nodeharbor_core::Resources {
            cpus: 2,
            memory_mib: 4096,
            disk_gib: 32,
        },
        directory.path(),
        nodeharbor_agent::guest_files(),
        &locations,
        OWNER,
        1,
    )
    .await
    .unwrap();
    let config = runtime.config.lock().unwrap().clone().unwrap();
    assert_eq!(
        config["additionalDisks"],
        json!([{"name":"nhone","format":false},{"name":"nhtwo","format":false}])
    );
    assert_eq!(config["plain"], true);
    assert_eq!(config["disk"], "16GiB");
    assert_eq!(config["mounts"], json!([]));
    assert_eq!(config["ssh"]["loadDotSSHPubKeys"], false);
    let provision = config["provision"].as_array().unwrap();
    let request = provision
        .iter()
        .find(|entry| entry["path"] == "/etc/nodeharbor/storage-request.json")
        .unwrap();
    let request: Value = serde_json::from_str(request["content"].as_str().unwrap()).unwrap();
    assert_eq!(request["deviceId"], OWNER);
    assert_eq!(request["disks"][0]["device"], "/dev/vdb");
    assert_eq!(request["disks"][1]["device"], "/dev/vdc");
    assert!(provision.iter().any(|entry| entry["script"]
        .as_str()
        .is_some_and(|script| script.contains("storage_pool.py") && script.contains("migrate"))));
    assert!(!serde_json::to_string(&config).unwrap().contains("/first"));
}
