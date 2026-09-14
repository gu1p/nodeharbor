use nodeharbor_agent::{CommandOutput, Runner, Vm, VmProvider};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";

struct PreparingStorage {
    stalled_step: &'static str,
    entered: Notify,
    complete: Notify,
    renewed: Notify,
    leases: AtomicUsize,
    reject_lease: AtomicBool,
}
impl PreparingStorage {
    fn new(stalled_step: &'static str) -> Self {
        Self {
            stalled_step,
            entered: Notify::new(),
            complete: Notify::new(),
            renewed: Notify::new(),
            leases: AtomicUsize::new(0),
            reject_lease: AtomicBool::new(false),
        }
    }
}
#[async_trait::async_trait]
impl Runner for PreparingStorage {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        input: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let step = if input.is_some() {
            "install"
        } else if args.iter().any(|arg| arg == "disable") {
            "quiesce"
        } else if args.iter().any(|arg| arg.contains("apt-get")) {
            "packages"
        } else {
            "other"
        };
        if step == self.stalled_step {
            self.entered.notify_one();
            self.complete.notified().await;
        }
        if args.iter().any(|arg| arg.ends_with("/watchdog.py")) {
            anyhow::ensure!(
                !self.reject_lease.load(Ordering::SeqCst),
                "Guest rejected the supervising owner's lease"
            );
            self.leases.fetch_add(1, Ordering::SeqCst);
            self.renewed.notify_one();
        }
        Ok(CommandOutput {
            success: true,
            stdout: if args.iter().any(|arg| arg == "cat") {
                ID.into()
            } else if args.iter().any(|arg| arg == "--property=LoadState") {
                "loaded".into()
            } else {
                String::new()
            },
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn every_slow_storage_preparation_step_renews_only_until_completion_or_cancellation() {
    for step in ["install", "quiesce", "packages"] {
        for cancel in [false, true] {
            let guest = Arc::new(PreparingStorage::new(step));
            let vm = Vm::new(ID, guest.clone()).unwrap();
            let task = tokio::spawn(async move { vm.prepare_storage_guest().await });
            guest.entered.notified().await;
            assert_eq!(
                guest.leases.load(Ordering::SeqCst),
                1,
                "Renew before {step}"
            );
            guest.renewed.notified().await;
            tokio::time::pause();
            for expected in 2..=6 {
                tokio::time::advance(Duration::from_secs(30)).await;
                tokio::time::timeout(Duration::from_millis(1), guest.renewed.notified())
                    .await
                    .expect("Storage preparation must retain the supervising owner's lease");
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
                6,
                "No detached renewal may survive storage preparation"
            );
            tokio::time::resume();
        }
    }
}

#[tokio::test]
async fn storage_preparation_ends_when_the_guest_rejects_its_owner_lease() {
    let guest = Arc::new(PreparingStorage::new("packages"));
    let vm = Vm::new(ID, guest.clone()).unwrap();
    let task = tokio::spawn(async move { vm.prepare_storage_guest().await });
    guest.entered.notified().await;
    guest.reject_lease.store(true, Ordering::SeqCst);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30)).await;
    let result = tokio::time::timeout(Duration::from_millis(1), task)
        .await
        .expect("A lost owner lease must interrupt preparation");
    assert!(result.unwrap().is_err());
}
