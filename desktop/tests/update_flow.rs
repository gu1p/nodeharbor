#[path = "../src/update_flow.rs"]
mod update_flow;
use std::sync::Mutex;
use update_flow::{apply, UpdateRuntime};
struct Runtime {
    events: Mutex<Vec<&'static str>>,
    fail: &'static str,
    cancel: bool,
}
impl Runtime {
    fn new(fail: &'static str, cancel: bool) -> Self {
        Self {
            events: Mutex::new(vec![]),
            fail,
            cancel,
        }
    }
    fn record(&self, event: &'static str) -> Result<(), String> {
        self.events.lock().unwrap().push(event);
        if event == self.fail {
            Err(event.into())
        } else {
            Ok(())
        }
    }
}
#[async_trait::async_trait]
impl UpdateRuntime for Runtime {
    async fn download(&self) -> Result<(), String> {
        self.record("verify download")
    }
    async fn prepare(&self) -> Result<(), String> {
        self.record("hold assignments")
    }
    async fn ready(&self) -> Result<bool, String> {
        self.record("inspect worker")?;
        Ok(self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == "inspect worker")
            .count()
            > 1)
    }
    async fn wait(&self) {
        self.events.lock().unwrap().push("wait for jobs");
    }
    fn cancelled(&self) -> bool {
        self.cancel && self.events.lock().unwrap().contains(&"wait for jobs")
    }
    async fn install(&self) -> Result<(), String> {
        self.record("install")
    }
    async fn release(&self) -> Result<(), String> {
        self.record("restore sharing")
    }
}
#[tokio::test]
async fn verified_download_precedes_maintenance_and_installation_waits_for_the_worker() {
    let r = Runtime::new("", false);
    assert!(apply(&r).await.unwrap());
    assert_eq!(
        *r.events.lock().unwrap(),
        [
            "verify download",
            "hold assignments",
            "inspect worker",
            "wait for jobs",
            "inspect worker",
            "install"
        ]
    );
}
#[tokio::test]
async fn invalid_signature_cannot_pause_jobs_or_install() {
    let r = Runtime::new("verify download", false);
    assert!(apply(&r).await.is_err());
    assert_eq!(*r.events.lock().unwrap(), ["verify download"]);
}
#[tokio::test]
async fn cancel_and_failed_install_restore_sharing_without_installing_on_a_busy_worker() {
    for (failure, cancel) in [
        ("", true),
        ("install", false),
        ("inspect worker", false),
        ("hold assignments", false),
    ] {
        let r = Runtime::new(failure, cancel);
        let result = apply(&r).await;
        assert!(cancel && result == Ok(false) || !cancel && result.is_err());
        let events = r.events.lock().unwrap();
        assert_eq!(events.last(), Some(&"restore sharing"));
        if cancel {
            assert!(!events.contains(&"install"));
        }
    }
}
