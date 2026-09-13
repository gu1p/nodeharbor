use async_trait::async_trait;
use nodeharbor_agent::{CommandOutput, Runner, Vm};
use serde_json::{json, Value};
use std::sync::Arc;
const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const PROBE: &str = "a8b219f7-a1a0-44a8-a876-bd06a64d91cb";
struct Inventory(Value);
#[async_trait]
impl Runner for Inventory {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let stdout = if args
            .last()
            .is_some_and(|a| a == "/etc/nodeharbor/device-id")
        {
            ID.into()
        } else {
            self.0.to_string()
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}
fn pod(uid: &str, name: &str, namespace: &str) -> Value {
    json!({"metadata":{"name":name,"namespace":namespace,"uid":uid},"state":"SANDBOX_READY"})
}
#[tokio::test]
async fn only_the_controller_verified_probe_uid_is_excluded_from_workloads() {
    let vm = Vm::new(
        ID,
        Arc::new(Inventory(json!({"items":[
            pod(PROBE,"probe-123","nodeharbor-system"),
            pod("de08b5c1-248f-4f9f-b8d3-6b2522d4b78c","probe-imposter","nodeharbor-system"),
            pod("9737e550-a2df-4a5a-9e0f-a88290fa9d4c","build","nodeharbor-ci")
        ]}))),
    )
    .unwrap();
    let all = vm.workloads(&[]).await.unwrap();
    assert_eq!(all.len(), 3);
    let work = vm.workloads(&[PROBE.into()]).await.unwrap();
    assert_eq!(
        work.iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["probe-imposter", "build"]
    );
}
#[tokio::test]
async fn missing_or_malformed_inventory_is_an_error_not_an_empty_worker() {
    for inventory in [
        json!({}),
        json!({"items":null}),
        json!({"items":{}}),
        json!({"items":[{}]}),
    ] {
        let vm = Vm::new(ID, Arc::new(Inventory(inventory))).unwrap();
        assert!(vm.workloads(&[]).await.is_err());
    }
}
