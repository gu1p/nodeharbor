use nodeharbor_agent::{CommandOutput, Runner, Vm};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;

const OWNER: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
#[derive(Default)]
struct Guest {
    leases: AtomicUsize,
    reject: AtomicBool,
    renewed: Notify,
}
#[async_trait::async_trait]
impl Runner for Guest {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        if args.iter().any(|a| a.ends_with("watchdog.py")) {
            anyhow::ensure!(!self.reject.load(Ordering::SeqCst), "Owner lease rejected");
            self.leases.fetch_add(1, Ordering::SeqCst);
            self.renewed.notify_one();
        }
        Ok(CommandOutput {
            success: true,
            stdout: OWNER.into(),
            stderr: String::new(),
        })
    }
}
#[tokio::test]
async fn slow_host_verification_keeps_the_guest_alive_and_ends_renewal_on_completion_or_cancellation(
) {
    for cancel in [false, true] {
        let guest = Arc::new(Guest::default());
        let vm = Vm::new(OWNER, guest.clone()).unwrap();
        let done = Arc::new(Notify::new());
        let finish = done.clone();
        let task = tokio::spawn(async move {
            vm.with_owner_lease(async {
                finish.notified().await;
                Ok(42)
            })
            .await
        });
        guest.renewed.notified().await;
        tokio::time::pause();
        for expected in 2..=6 {
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::time::timeout(Duration::from_millis(1), guest.renewed.notified())
                .await
                .expect("Hashing beyond the watchdog deadline must retain the owner's lease");
            assert_eq!(guest.leases.load(Ordering::SeqCst), expected);
        }
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            done.notify_one();
            assert_eq!(task.await.unwrap().unwrap(), 42);
        }
        tokio::time::advance(Duration::from_secs(300)).await;
        assert_eq!(
            guest.leases.load(Ordering::SeqCst),
            6,
            "No detached renewal after host verification"
        );
        tokio::time::resume();
    }
}
#[tokio::test]
async fn rejected_owner_lease_prevents_the_next_verification_step() {
    let guest = Arc::new(Guest::default());
    let vm = Vm::new(OWNER, guest.clone()).unwrap();
    let progressed = Arc::new(AtomicBool::new(false));
    let next = progressed.clone();
    let task = tokio::spawn(async move {
        vm.with_owner_lease(std::future::pending::<anyhow::Result<()>>())
            .await?;
        next.store(true, Ordering::SeqCst);
        Ok::<_, anyhow::Error>(())
    });
    guest.renewed.notified().await;
    guest.reject.store(true, Ordering::SeqCst);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(tokio::time::timeout(Duration::from_millis(1), task)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert!(!progressed.load(Ordering::SeqCst));
}
