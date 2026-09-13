use async_trait::async_trait;
use nodeharbor_agent::{CommandOutput, Runner, Vm};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
#[derive(Default)]
struct Guest {
    wrong_owner: bool,
    fail_update: bool,
    files: Mutex<Option<Value>>,
    configured: Mutex<bool>,
}
#[async_trait]
impl Runner for Guest {
    async fn run(
        &self,
        args: &[String],
        input: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        if args.iter().any(|arg| arg == "-c") {
            anyhow::ensure!(!self.fail_update, "Guest files could not be installed");
            *self.files.lock().unwrap() = Some(serde_json::from_slice(&input.unwrap())?);
        }
        if args
            .last()
            .is_some_and(|arg| arg == "/usr/local/lib/nodeharbor/configure_worker.py")
        {
            *self.configured.lock().unwrap() = true;
            anyhow::ensure!(
                self.files.lock().unwrap().is_some(),
                "Preparation used obsolete guest scripts"
            );
        }
        Ok(CommandOutput {
            success: true,
            stdout: if args.iter().any(|arg| arg == "cat") {
                if self.wrong_owner {
                    "another-device"
                } else {
                    ID
                }
                .into()
            } else {
                String::new()
            },
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn preparing_an_existing_vm_installs_the_apps_current_guest_code_before_using_it() {
    let guest = Arc::new(Guest::default());
    Vm::new(ID, guest.clone())
        .unwrap()
        .configure(json!({"credential":"bootstrap-only"}))
        .await
        .unwrap();
    let payload = guest.files.lock().unwrap().clone().unwrap();
    assert_eq!(payload["deviceId"], ID);
    assert_eq!(payload["files"], nodeharbor_agent::guest_files());
    assert!(!payload.to_string().contains("bootstrap-only"));
    assert!(*guest.configured.lock().unwrap());
}

#[tokio::test]
async fn failed_updates_and_wrong_ownership_prevent_guest_configuration() {
    for wrong_owner in [false, true] {
        let guest = Arc::new(Guest {
            wrong_owner,
            fail_update: !wrong_owner,
            ..Default::default()
        });
        let result = Vm::new(ID, guest.clone())
            .unwrap()
            .configure(json!({}))
            .await;
        assert!(result.is_err());
        assert!(!*guest.configured.lock().unwrap());
        assert!(guest.files.lock().unwrap().is_none());
    }
}
