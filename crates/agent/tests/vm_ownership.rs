use async_trait::async_trait;
use nodeharbor_agent::{CommandOutput, Runner, Vm};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
struct FakeRunner {
    outputs: Mutex<VecDeque<CommandOutput>>,
    calls: Mutex<Vec<Vec<String>>>,
}
#[async_trait]
impl Runner for FakeRunner {
    async fn run(
        &self,
        args: &[String],
        _stdin: Option<Vec<u8>>,
        _timeout: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        Ok(self
            .outputs
            .lock()
            .unwrap()
            .pop_front()
            .expect("Unexpected VM operation"))
    }
}
#[tokio::test]
async fn stopping_a_vm_requires_its_in_guest_ownership_marker() {
    let runner = Arc::new(FakeRunner {
        outputs: Mutex::new(VecDeque::from([CommandOutput {
            success: true,
            stdout: "someone-elses-device".into(),
            stderr: String::new(),
        }])),
        calls: Mutex::new(vec![]),
    });
    let vm = Vm::new("9511182e-9c48-4d20-a15b-1da8bb441386", runner.clone()).unwrap();
    assert!(vm.stop().await.is_err());
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|args| args.first().unwrap() != "stop"));
}
#[tokio::test]
async fn only_the_managed_device_vm_can_be_stopped() {
    let runner = Arc::new(FakeRunner {
        outputs: Mutex::new(VecDeque::from([
            CommandOutput {
                success: true,
                stdout: "9511182e-9c48-4d20-a15b-1da8bb441386\n".into(),
                stderr: String::new(),
            },
            CommandOutput {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        ])),
        calls: Mutex::new(vec![]),
    });
    let vm = Vm::new("9511182e-9c48-4d20-a15b-1da8bb441386", runner.clone()).unwrap();
    vm.stop().await.unwrap();
    assert_eq!(
        runner.calls.lock().unwrap()[1],
        vec!["stop", "nodeharbor-9511182e9c484d20a15b1da8bb441386"]
    );
}
#[tokio::test]
async fn vm_inventory_does_not_adopt_an_unrelated_instance() {
    let runner = Arc::new(FakeRunner {
        outputs: Mutex::new(VecDeque::from([CommandOutput {
            success: true,
            stdout: r#"{"list":[{"name":"other-vm","state":"Running","ipv4":["10.0.0.5"]}]}"#
                .into(),
            stderr: String::new(),
        }])),
        calls: Mutex::new(vec![]),
    });
    let vm = Vm::new("9511182e-9c48-4d20-a15b-1da8bb441386", runner).unwrap();
    assert!(!vm.info().await.unwrap().installed);
}
