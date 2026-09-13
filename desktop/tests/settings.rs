#[path = "../src/settings.rs"]
mod settings;
use settings::{save_with_startup, StartupRegistration};
use std::{
    cell::{Cell, RefCell},
    future,
};
struct Startup {
    enabled: Cell<bool>,
    fail: Cell<bool>,
    events: RefCell<Vec<String>>,
}
impl StartupRegistration for Startup {
    fn is_enabled(&self) -> Result<bool, String> {
        Ok(self.enabled.get())
    }
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.events.borrow_mut().push(format!("startup:{enabled}"));
        self.enabled.set(enabled);
        if self.fail.replace(false) {
            Err("OS registration failed".into())
        } else {
            Ok(())
        }
    }
}
fn startup() -> Startup {
    Startup {
        enabled: Cell::new(false),
        fail: Cell::new(false),
        events: RefCell::new(vec![]),
    }
}
#[tokio::test]
async fn registration_finishes_before_a_destructive_worker_request_is_committed() {
    let os = startup();
    let result = save_with_startup(&os, true, || {
        assert!(os.enabled.get());
        os.events.borrow_mut().push("commit".into());
        future::ready(Ok::<_, String>("queued"))
    })
    .await;
    assert_eq!(result.unwrap(), "queued");
    assert_eq!(*os.events.borrow(), ["startup:true", "commit"]);
}
#[tokio::test]
async fn failed_registration_restores_actual_os_state_without_committing_a_worker_request() {
    let os = startup();
    os.fail.set(true);
    let result = save_with_startup(&os, true, || {
        panic!("Cannot commit recreation when startup registration failed");
        #[allow(unreachable_code)]
        future::ready(Ok::<_, String>(()))
    })
    .await;
    assert!(result.is_err());
    assert!(!os.enabled.get());
    assert_eq!(*os.events.borrow(), ["startup:true", "startup:false"]);
}
#[tokio::test]
async fn failed_settings_validation_restores_registration_without_rewriting_worker_state() {
    let os = startup();
    let result = save_with_startup(&os, true, || {
        future::ready(Err::<(), _>("Invalid resource budget".into()))
    })
    .await;
    assert!(result.unwrap_err().contains("Invalid resource budget"));
    assert!(!os.enabled.get());
}
#[tokio::test]
async fn unchanged_registration_does_not_touch_os_settings() {
    let os = startup();
    save_with_startup(&os, false, || future::ready(Ok::<_, String>(())))
        .await
        .unwrap();
    assert!(os.events.borrow().is_empty());
}
