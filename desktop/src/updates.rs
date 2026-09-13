use crate::{update_flow, Desktop};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    enabled: bool,
    phase: String,
    message: String,
    current_version: String,
    available_version: Option<String>,
    downloaded: u64,
    total: Option<u64>,
}
impl Status {
    pub fn is_installing(&self) -> bool {
        self.phase == "installing"
    }
}
pub struct Updates {
    status: Mutex<Status>,
    generation: AtomicU64,
    request: AtomicU8,
    wake: tokio::sync::Notify,
}
impl Updates {
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(Status {
                enabled,
                phase: "idle".into(),
                message: "Updates are checked automatically while NodeHarbor is running".into(),
                current_version: env!("CARGO_PKG_VERSION").into(),
                available_version: None,
                downloaded: 0,
                total: None,
            }),
            generation: AtomicU64::new(0),
            request: AtomicU8::new(0),
            wake: tokio::sync::Notify::new(),
        })
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    fn report(&self, phase: &str, message: &str) {
        let mut status = self.status.lock().unwrap();
        status.phase = phase.into();
        status.message = message.into();
    }
    pub fn action(&self, app: &AppHandle, action: &str) -> Result<Status, String> {
        if self.status().phase == "installing" {
            return Err("The update is being installed".into());
        }
        match action {
            "enable" | "disable" => {
                let enabled = action == "enable";
                app.state::<Desktop>()
                    .agent
                    .store
                    .update(|config| {
                        config.automatic_updates = enabled;
                        Ok(())
                    })
                    .map_err(|e| e.to_string())?;
                self.status.lock().unwrap().enabled = enabled;
                if enabled {
                    self.request.store(1, Ordering::SeqCst);
                    self.wake.notify_one();
                } else {
                    self.generation.fetch_add(1, Ordering::SeqCst);
                    if ["downloading", "waiting"].contains(&self.status().phase.as_str()) {
                        self.report("cancelling", "Cancelling the update…");
                    }
                }
            }
            "cancel" => {
                self.generation.fetch_add(1, Ordering::SeqCst);
                self.report("cancelling", "Cancelling the update…");
            }
            "check" | "install" => {
                if ["checking", "downloading", "waiting", "cancelling"]
                    .contains(&self.status().phase.as_str())
                {
                    return Err("An update operation is already running".into());
                }
                self.request
                    .store(if action == "install" { 2 } else { 1 }, Ordering::SeqCst);
                self.report("checking", "Checking for updates…");
                self.wake.notify_one();
            }
            _ => return Err("Unknown update action".into()),
        }
        Ok(self.status())
    }
    pub async fn run(self: Arc<Self>, app: AppHandle) {
        let mut delay = Duration::from_secs(30);
        loop {
            tokio::select! { _ = tokio::time::sleep(delay) => {}, _ = self.wake.notified() => {} }
            delay = Duration::from_secs(6 * 60 * 60);
            let request = self.request.swap(0, Ordering::SeqCst);
            if request == 0 && !self.status().enabled {
                continue;
            }
            if app.state::<Desktop>().quitting.load(Ordering::SeqCst) {
                return;
            }
            if let Err(error) = self.check(&app, request == 2).await {
                self.report("error", &format!("Update could not finish: {error}. You can retry; your sharing rules are kept."));
            }
        }
    }
    async fn check(
        self: &Arc<Self>,
        app: &AppHandle,
        install_requested: bool,
    ) -> Result<(), String> {
        self.report("checking", "Checking for updates…");
        let generation = self.generation.load(Ordering::SeqCst);
        let updater = app
            .updater_builder()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        let update = tokio::time::timeout(Duration::from_secs(30), updater.check())
            .await
            .map_err(|_| "The update check timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let Some(update) = update else {
            self.status.lock().unwrap().available_version = None;
            self.report("idle", "You are up to date");
            return Ok(());
        };
        // Never accept a download URL that changes the HTTPS-only trust boundary.
        if update.download_url.scheme() != "https" {
            return Err("The update download must use HTTPS".into());
        }
        {
            let mut status = self.status.lock().unwrap();
            status.available_version = Some(update.version.clone());
            status.downloaded = 0;
            status.total = None;
        }
        if (!install_requested && !self.status().enabled)
            || generation != self.generation.load(Ordering::SeqCst)
        {
            self.report(
                "available",
                &format!("NodeHarbor {} is available", update.version),
            );
            return Ok(());
        }
        let runtime = Runtime {
            app: app.clone(),
            updates: self.clone(),
            update,
            generation,
            bytes: Mutex::new(None),
        };
        if update_flow::apply(&runtime).await? {
            app.restart();
        } else {
            self.report(
                "available",
                "Update cancelled. Your sharing rules still apply.",
            );
        }
        Ok(())
    }
}
struct Runtime {
    app: AppHandle,
    updates: Arc<Updates>,
    update: Update,
    generation: u64,
    bytes: Mutex<Option<Vec<u8>>>,
}
#[async_trait::async_trait]
impl update_flow::UpdateRuntime for Runtime {
    async fn download(&self) -> Result<(), String> {
        self.updates
            .report("downloading", "Downloading and verifying the update…");
        let bytes = self
            .update
            .download(
                |size, total| {
                    let mut status = self.updates.status.lock().unwrap();
                    status.downloaded += size as u64;
                    status.total = total;
                },
                || {},
            )
            .await
            .map_err(|e| e.to_string())?;
        *self.bytes.lock().unwrap() = Some(bytes);
        Ok(())
    }
    async fn prepare(&self) -> Result<(), String> {
        self.updates.report(
            "waiting",
            "Waiting for running jobs to finish. New assignments are paused.",
        );
        self.app
            .state::<Desktop>()
            .agent
            .begin_application_update()
            .await
            .map_err(|e| e.to_string())
    }
    async fn ready(&self) -> Result<bool, String> {
        self.app
            .state::<Desktop>()
            .agent
            .application_update_ready()
            .await
            .map_err(|e| e.to_string())
    }
    async fn wait(&self) {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    fn cancelled(&self) -> bool {
        self.generation != self.updates.generation.load(Ordering::SeqCst)
            || self.app.state::<Desktop>().quitting.load(Ordering::SeqCst)
    }
    async fn install(&self) -> Result<(), String> {
        let state = self.app.state::<Desktop>();
        state
            .quitting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "NodeHarbor is closing".to_string())?;
        self.updates.report(
            "installing",
            "Installing the update. NodeHarbor will restart automatically.",
        );
        let bytes = self
            .bytes
            .lock()
            .unwrap()
            .take()
            .ok_or("No verified update was downloaded")?;
        let update = self.update.clone();
        let result = tauri::async_runtime::spawn_blocking(move || update.install(bytes))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        if result.is_err() {
            state.quitting.store(false, Ordering::SeqCst);
        }
        result
    }
    async fn release(&self) -> Result<(), String> {
        self.app
            .state::<Desktop>()
            .agent
            .cancel_application_update()
            .await
            .map_err(|e| e.to_string())
    }
}
