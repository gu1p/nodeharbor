use async_trait::async_trait;
use nodeharbor_agent::{CommandOutput, Runner, Vm, VmProvider};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";

#[derive(Default)]
struct BootingGuest {
    entered: Notify,
    complete: Notify,
    inspected: Notify,
    renewed: Notify,
    reachable: AtomicBool,
    wrong_owner: AtomicBool,
    leases: AtomicUsize,
}
#[async_trait]
impl Runner for BootingGuest {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        timeout: u64,
    ) -> anyhow::Result<CommandOutput> {
        if args[0] == "start" {
            assert_eq!(timeout, 180, "Boot must retain its existing deadline");
            self.entered.notify_one();
            self.complete.notified().await;
        }
        let mut stdout = String::new();
        if args.iter().any(|arg| arg == "cat") {
            self.inspected.notify_one();
            anyhow::ensure!(self.reachable.load(Ordering::SeqCst), "SSH is not ready");
            stdout = if self.wrong_owner.load(Ordering::SeqCst) {
                "another-device".into()
            } else {
                ID.into()
            };
        }
        if args.iter().any(|arg| arg.ends_with("/watchdog.py")) {
            self.leases.fetch_add(1, Ordering::SeqCst);
            self.renewed.notify_one();
        }
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

fn owned_vm(directory: &std::path::Path, guest: Arc<BootingGuest>) -> Vm {
    std::fs::write(
        directory.join("worker.receipt.json"),
        json!({"version":2,"provider":"lima","deviceId":ID,"name":"worker"}).to_string(),
    )
    .unwrap();
    Vm::managed(ID, directory, guest).unwrap()
}

#[tokio::test]
async fn slow_boot_keeps_the_owner_lease_alive_until_completion_or_cancellation() {
    for cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let guest = Arc::new(BootingGuest::default());
        let vm = owned_vm(directory.path(), guest.clone());
        let task = tokio::spawn(async move { vm.start().await });
        guest.entered.notified().await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::time::timeout(Duration::from_millis(1), guest.inspected.notified())
            .await
            .expect("The owner must reconnect while Lima is still waiting for boot");
        assert!(
            !task.is_finished(),
            "An early SSH failure must remain retryable"
        );
        assert_eq!(guest.leases.load(Ordering::SeqCst), 0);
        guest.reachable.store(true, Ordering::SeqCst);
        for expected in 1..=3 {
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::time::timeout(Duration::from_millis(1), guest.renewed.notified())
                .await
                .expect("The watchdog must receive the supervising owner's lease");
            assert_eq!(guest.leases.load(Ordering::SeqCst), expected);
        }
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            guest.complete.notify_one();
            task.await.unwrap().unwrap();
        }
        tokio::time::advance(Duration::from_secs(300)).await;
        assert_eq!(
            guest.leases.load(Ordering::SeqCst),
            3,
            "No detached renewal may survive boot"
        );
        tokio::time::resume();
    }
}

#[tokio::test]
async fn boot_never_renews_a_lease_for_a_different_guest_identity() {
    let directory = tempfile::tempdir().unwrap();
    let guest = Arc::new(BootingGuest::default());
    guest.reachable.store(true, Ordering::SeqCst);
    guest.wrong_owner.store(true, Ordering::SeqCst);
    let vm = owned_vm(directory.path(), guest.clone());
    let task = tokio::spawn(async move { vm.start().await });
    guest.entered.notified().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30)).await;
    let result = tokio::time::timeout(Duration::from_millis(1), task)
        .await
        .expect("A different guest identity must end boot immediately");
    assert!(result.unwrap().is_err());
    assert_eq!(guest.leases.load(Ordering::SeqCst), 0);
}
