use async_trait::async_trait;
use nodeharbor_agent::{CommandOutput, Runner, Vm};
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
struct PreparingGuest {
    entered: Notify,
    complete: Notify,
    renewed: Notify,
    leases: AtomicUsize,
    reject_lease: AtomicBool,
}
#[async_trait]
impl Runner for PreparingGuest {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        if args
            .iter()
            .any(|arg| arg == "/usr/local/lib/nodeharbor/configure_worker.py")
        {
            self.entered.notify_one();
            self.complete.notified().await;
        }
        if args
            .iter()
            .any(|arg| arg == "/usr/local/lib/nodeharbor/watchdog.py")
        {
            anyhow::ensure!(
                !self.reject_lease.load(Ordering::SeqCst),
                "Guest rejected the owner lease"
            );
            self.leases.fetch_add(1, Ordering::SeqCst);
            self.renewed.notify_one();
        }
        Ok(CommandOutput {
            success: true,
            stdout: if args.iter().any(|arg| arg == "cat") {
                ID.into()
            } else {
                String::new()
            },
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn a_long_preparation_renews_its_lease_only_while_the_owner_is_supervising() {
    for cancel in [false, true] {
        let guest = Arc::new(PreparingGuest::default());
        let vm = Vm::new(ID, guest.clone()).unwrap();
        let task = tokio::spawn(async move { vm.configure(json!({})).await });
        guest.entered.notified().await;
        assert_eq!(
            guest.leases.load(Ordering::SeqCst),
            1,
            "Renew before starting a potentially long configuration"
        );
        tokio::time::pause();
        guest.renewed.notified().await;
        for expected in 2..=5 {
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::time::timeout(Duration::from_millis(1), guest.renewed.notified())
                .await
                .unwrap();
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
            5,
            "No detached task may keep an abandoned worker alive"
        );
        tokio::time::resume();
    }
}

#[tokio::test]
async fn preparation_cannot_succeed_after_losing_its_owner_lease() {
    let guest = Arc::new(PreparingGuest::default());
    let vm = Vm::new(ID, guest.clone()).unwrap();
    let task = tokio::spawn(async move { vm.configure(json!({})).await });
    guest.entered.notified().await;
    guest.reject_lease.store(true, Ordering::SeqCst);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30)).await;
    let result = tokio::time::timeout(Duration::from_millis(1), task)
        .await
        .expect("Losing the lease must end preparation");
    assert!(result.unwrap().is_err());
}
