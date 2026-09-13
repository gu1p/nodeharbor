use crate::{worker_transition, Configuration, Store, Vm, VmInfo, WorkerAction, WorkerInput};
use anyhow::{Context, Result};
use nodeharbor_core::{evaluate, validate_policy, Policy, Resources};
use serde::Serialize;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone, Copy)]
enum Starting {
    Preparation,
    Resume,
}
#[derive(Clone, Default)]
struct Runtime {
    state: Option<String>,
    reason: Option<String>,
    worker: VmInfo,
    workloads: Vec<Value>,
    remote_paused: bool,
    starting: Option<Starting>,
    stopping_now: bool,
}
#[derive(Clone)]
pub struct Agent {
    pub store: Store,
    runtime: Arc<Mutex<Runtime>>,
    operation: Arc<Mutex<()>>,
    client: reqwest::Client,
    runner: Arc<dyn crate::Runner>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub architecture: String,
    pub state: String,
    pub reason: String,
    pub policy: Policy,
    pub resources: Resources,
    pub worker: VmInfo,
    pub enrolled: bool,
    pub controller_url: String,
    pub version: String,
    pub workloads: Vec<Value>,
}
impl Agent {
    pub fn open(directory: &Path) -> Result<Self> {
        Self::open_with_runner(directory, Arc::new(crate::vm::MultipassRunner))
    }
    pub fn open_with_runner(directory: &Path, runner: Arc<dyn crate::Runner>) -> Result<Self> {
        Ok(Self {
            runner,
            store: Store::open(directory)?,
            runtime: Arc::new(Mutex::new(Runtime::default())),
            operation: Arc::new(Mutex::new(())),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let config = self.store.load()?;
        let allocated = config
            .allocated_resources
            .as_ref()
            .map(|r| r.disk_gib)
            .unwrap_or(0);
        let observation = crate::observe::observation(&self.store.directory, allocated);
        let decision = evaluate(&config.policy, &observation);
        let mut runtime = self.runtime.lock().await.clone();
        runtime.worker.installed = config.vm_configured;
        Ok(Snapshot {
            device_id: config.device_id,
            name: config.name,
            platform: std::env::consts::OS.into(),
            architecture: crate::observe::architecture().into(),
            state: runtime.state.unwrap_or_else(|| "paused".into()),
            reason: runtime.reason.unwrap_or(decision.reason),
            policy: config.policy,
            resources: observation.resources,
            worker: runtime.worker,
            enrolled: config.device_token.is_some(),
            controller_url: config.controller_url.unwrap_or_default(),
            version: env!("CARGO_PKG_VERSION").into(),
            workloads: runtime.workloads,
        })
    }
    pub async fn save_policy(&self, policy: Policy) -> Result<Snapshot> {
        let host = self.snapshot().await?.resources;
        validate_policy(&policy, &host).map_err(anyhow::Error::msg)?;
        self.store.update(|config| {
            if let Some(allocated) = &config.allocated_resources {
                anyhow::ensure!(
                    policy.resources.disk_gib >= allocated.disk_gib,
                    "Shrinking the worker disk requires explicitly recreating the VM"
                );
            }
            if policy.enabled {
                anyhow::ensure!(
                    config.device_token.is_some(),
                    "Connect this computer to a fleet before enabling sharing"
                );
            }
            config.policy = policy;
            if config.vm_configured && local_drain_required(config, &self.store.directory) {
                config.draining_since.get_or_insert_with(now_seconds);
            }
            Ok(())
        })?;
        self.snapshot().await
    }
    pub async fn action(&self, action: &str) -> Result<Snapshot> {
        anyhow::ensure!(
            ["prepare", "resume", "pause", "stop"].contains(&action),
            "Unknown worker action"
        );
        self.store.update(|config| {
            if action == "prepare" || action == "resume" {
                anyhow::ensure!(
                    config.device_token.is_some(),
                    "Connect this computer to a fleet first"
                );
            }
            match action {
                "prepare" => {
                    config.prepare_requested = true;
                    config.stop_requested = false;
                }
                "resume" => {
                    anyhow::ensure!(
                        config.vm_configured,
                        "Prepare the worker before enabling sharing"
                    );
                    config.policy.enabled = true;
                    config.stop_requested = false;
                    config.draining_since = None;
                }
                "pause" => {
                    config.policy.enabled = false;
                    config.prepare_requested = false;
                    if config.vm_created {
                        config.draining_since.get_or_insert_with(now_seconds);
                    }
                }
                "stop" => {
                    config.policy.enabled = false;
                    config.prepare_requested = false;
                    config.stop_requested = true;
                }
                _ => unreachable!(),
            };
            Ok(())
        })?;
        self.snapshot().await
    }
    pub async fn enroll(&self, url: &str, code: &str) -> Result<Snapshot> {
        let parsed = reqwest::Url::parse(url.trim()).context("Enter a valid controller URL")?;
        anyhow::ensure!(
            parsed.scheme() == "https"
                || (parsed.scheme() == "http"
                    && ["127.0.0.1", "localhost", "[::1]"]
                        .contains(&parsed.host_str().unwrap_or_default())),
            "Use HTTPS for the fleet controller"
        );
        anyhow::ensure!(
            parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.query().is_none()
                && parsed.fragment().is_none(),
            "The controller URL cannot contain credentials, query parameters, or fragments"
        );
        let existing = self.store.load()?;
        anyhow::ensure!(
            existing.device_token.is_none(),
            "This computer is already enrolled; existing enrollment has been preserved"
        );
        let url = parsed.as_str().trim_end_matches('/');
        let response=self.client.post(format!("{url}/api/v1/enroll")).json(&json!({"code":code.trim(),"name":existing.name,"platform":std::env::consts::OS,"architecture":crate::observe::architecture()})).send().await?;
        let data = checked(response).await?;
        let device_id = data["deviceId"]
            .as_str()
            .context("The controller returned no device identity")?;
        uuid::Uuid::parse_str(device_id)?;
        let token = data["token"]
            .as_str()
            .filter(|s| s.len() >= 32)
            .context("The controller returned an invalid device credential")?;
        self.store.update(|config| {
            anyhow::ensure!(
                config.device_token.is_none(),
                "This computer was already enrolled"
            );
            config.device_id = device_id.into();
            config.device_token = Some(token.into());
            config.controller_url = Some(url.into());
            Ok(())
        })?;
        self.snapshot().await
    }
    pub async fn fleet(&self) -> Result<Vec<Value>> {
        let config = self.store.load()?;
        if config.device_token.is_none() {
            return Ok(Vec::new());
        }
        let value = self.request(&config, "/device/fleet", None).await?;
        value
            .as_array()
            .cloned()
            .context("The controller returned an invalid fleet response")
    }
    async fn request(
        &self,
        config: &Configuration,
        path: &str,
        payload: Option<Value>,
    ) -> Result<Value> {
        let url = format!(
            "{}/api/v1{path}",
            config
                .controller_url
                .as_ref()
                .context("This computer is not connected")?
        );
        let request = if let Some(payload) = payload {
            self.client.post(url).json(&payload)
        } else {
            self.client.get(url)
        };
        checked(
            request
                .bearer_auth(
                    config
                        .device_token
                        .as_ref()
                        .context("No device credential is saved")?,
                )
                .send()
                .await?,
        )
        .await
    }
    async fn set_status(&self, state: &str, reason: impl Into<String>) {
        let mut runtime = self.runtime.lock().await;
        runtime.state = Some(state.into());
        runtime.reason = Some(reason.into());
    }
    pub async fn run(&self) -> Result<()> {
        let _lock = self.store.supervisor_lock()?;
        loop {
            if let Err(error) = self.tick().await {
                self.set_status("error", error.to_string()).await;
            }
            let saved = serde_json::to_value(self.store.load()?)?;
            let next = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let current = self.store.load()?;
                if current.stop_requested
                    || deadline_expired(&current)
                    || serde_json::to_value(&current)? != saved
                    || tokio::time::Instant::now() >= next
                {
                    break;
                }
            }
        }
    }
    pub async fn tick(&self) -> Result<()> {
        let _operation = self.operation.lock().await;
        let config = self.store.load()?;
        if config.stop_requested || deadline_expired(&config) {
            // Shutdown itself must finish before another shutdown is issued.
            return self.finish_immediate_stop().await;
        }
        // The losing future is dropped before shutdown. MultipassRunner kills
        // its CLI child on drop; the owned VM is stopped through Multipass itself.
        let outcome = tokio::select! {
            result = self.tick_inner() => Some(result),
            result = self.wait_for_owner_interruption() => result.map(|()| None).unwrap_or_else(|error| Some(Err(error))),
        };
        self.runtime.lock().await.starting = None;
        if let Some(result) = outcome {
            return result;
        }
        self.finish_immediate_stop().await
    }
    async fn finish_immediate_stop(&self) -> Result<()> {
        self.runtime.lock().await.stopping_now = true;
        let result = async {
            let config = self.store.update(|current| {
                // A failed shutdown is retried even after the agent restarts.
                current.stop_requested = true;
                Ok(())
            })?;
            let vm = Vm::managed(
                &config.device_id,
                &self.store.directory,
                self.runner.clone(),
            )?;
            if (config.vm_created || vm.has_receipt()?) && vm.info().await?.running {
                vm.stop_now().await?;
            }
            self.store.update(|current| {
                current.stop_requested = false;
                current.draining_since = None;
                Ok(())
            })?;
            {
                let mut runtime = self.runtime.lock().await;
                runtime.worker.running = false;
                runtime.workloads.clear();
            }
            self.set_status("paused", "Worker stopped").await;
            Ok(())
        }
        .await;
        self.runtime.lock().await.stopping_now = false;
        result
    }
    async fn wait_for_owner_interruption(&self) -> Result<()> {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            // Poll the atomic store so installer actions in another process are
            // observed too. A normal pause of running workloads still drains.
            let config = self.store.load()?;
            let runtime = self.runtime.lock().await;
            if runtime.stopping_now {
                continue;
            }
            let pause = match runtime.starting {
                Some(Starting::Preparation) => !config.prepare_requested,
                Some(Starting::Resume) => !config.policy.enabled,
                None => false,
            };
            if config.stop_requested || pause || deadline_expired(&config) {
                return Ok(());
            }
        }
    }
    async fn tick_inner(&self) -> Result<()> {
        let config = self.store.load()?;
        if config.device_token.is_none() {
            self.set_status(
                "paused",
                "Connect this computer to a fleet to prepare its worker",
            )
            .await;
            return Ok(());
        }
        let vm = Vm::managed(
            &config.device_id,
            &self.store.directory,
            self.runner.clone(),
        )?;
        let owned = config.vm_created || vm.has_receipt()?;
        if config.stop_requested {
            return self.finish_immediate_stop().await;
        }
        if config.prepare_requested && config.vm_configured {
            // An already prepared worker uses the normal policy and drain flow,
            // including resource changes. Preparation cannot override that flow.
            self.store.update(|current| {
                current.prepare_requested = false;
                Ok(())
            })?;
        }
        if config.prepare_requested && !config.vm_configured {
            self.runtime.lock().await.starting = Some(Starting::Preparation);
            self.set_status(
                "preparing",
                "Preparing the Linux worker within your resource budget",
            )
            .await;
            let info = if owned {
                vm.info().await?
            } else {
                VmInfo::default()
            };
            let exists = info.installed;
            if !exists {
                let observation = crate::observe::observation(&self.store.directory, 0);
                validate_policy(&config.policy, &observation.resources)
                    .map_err(anyhow::Error::msg)?;
                vm.create(
                    &config.policy.resources,
                    &self.store.directory,
                    crate::guest_files(),
                )
                .await?;
            } else if config.allocated_resources.as_ref() != Some(&config.policy.resources) {
                // A retry may follow a failed launch or changed settings. Apply
                // the budget to the existing VM before recording it as allocated.
                if info.running {
                    vm.stop().await?;
                }
                vm.resize(&config.policy.resources).await?;
            }
            self.store.update(|c| {
                c.vm_created = true;
                c.allocated_resources = Some(config.policy.resources.clone());
                Ok(())
            })?;
            if !self.store.load()?.prepare_requested {
                if vm.info().await?.running {
                    vm.stop().await?;
                }
                return Ok(());
            }
            if exists && !vm.info().await?.reachable {
                vm.start().await?;
            }
            if !self.store.load()?.prepare_requested {
                vm.stop().await?;
                return Ok(());
            }
            // Bootstrap credentials must be fresh after a potentially slow first image download.
            let bootstrap = self
                .request(&config, "/device/bootstrap", Some(json!({})))
                .await?;
            vm.configure(bootstrap).await?;
            self.store.update(|c| {
                c.prepare_requested = false;
                c.vm_configured = true;
                Ok(())
            })?;
            self.runtime.lock().await.starting = None;
        }
        let config = self.store.load()?;
        let info = if config.vm_created || vm.has_receipt()? {
            vm.info().await?
        } else {
            VmInfo::default()
        };
        self.runtime.lock().await.worker = info.clone();
        if info.running && !config.vm_configured && !config.prepare_requested {
            // A partial worker can be Starting or Unknown and cannot perform a
            // guest shutdown. Reconcile the owner's pause through the hypervisor.
            vm.stop_now().await?;
            self.runtime.lock().await.worker.running = false;
            self.set_status(
                "paused",
                "Incomplete worker stopped. Prepare it again to retry",
            )
            .await;
            return Ok(());
        }
        if !config.vm_created {
            self.set_status(
                "paused",
                "Connected to your fleet. Prepare the Linux worker to continue",
            )
            .await;
            self.heartbeat(&config).await?;
            return Ok(());
        }
        if config.stop_requested && info.running {
            return self.finish_immediate_stop().await;
        }
        let resize_pending = config.vm_configured
            && config.allocated_resources.as_ref() != Some(&config.policy.resources);
        if resize_pending && !info.running {
            vm.resize(&config.policy.resources).await?;
            self.store.update(|current| {
                // Record exactly what was applied, even if the owner edits settings again.
                current.allocated_resources = Some(config.policy.resources.clone());
                Ok(())
            })?;
            self.set_status(
                "paused",
                "Worker resources updated; your sharing rules still apply",
            )
            .await;
            return Ok(());
        }
        let was_draining = config.draining_since.is_some();
        // Record local restrictions before HTTP or guest inspection can stall.
        if info.running && local_drain_required(&config, &self.store.directory) {
            let current = self.begin_drain()?;
            if deadline_expired(&current) {
                return self.finish_immediate_stop().await;
            }
        } else if !info.running {
            self.clear_drain()?;
        }
        let heartbeat_error = self.heartbeat(&self.store.load()?).await.err();
        // Owner actions can arrive from another process during the request.
        let config = self.store.load()?;
        let resize_pending = config.vm_configured
            && config.allocated_resources.as_ref() != Some(&config.policy.resources);
        let observation = crate::observe::observation(
            &self.store.directory,
            config
                .allocated_resources
                .as_ref()
                .map(|r| r.disk_gib)
                .unwrap_or(0),
        );
        let decision = evaluate(&config.policy, &observation);
        let allowed = decision.allowed
            && !resize_pending
            && heartbeat_error.is_none()
            && !self.runtime.lock().await.remote_paused;
        if info.running && !allowed {
            let current = self.begin_drain()?;
            if deadline_expired(&current) {
                return self.finish_immediate_stop().await;
            }
        }
        // An unreachable guest has an unknown workload count, not an empty one.
        // Keep the grace period, but never let a failed inspection cancel its deadline.
        let workloads = if info.running && info.reachable {
            vm.workloads().await.ok()
        } else if info.running {
            None
        } else {
            Some(Vec::new())
        };
        let now = now_seconds();
        let draining_since = self.store.load()?.draining_since;
        let transition = worker_transition(&WorkerInput {
            permitted: allowed,
            running: info.running,
            // The first drain still tells the controller to stop assignments.
            draining_since: if was_draining { draining_since } else { None },
            now,
            drain_seconds: config.policy.drain_seconds,
            workloads: workloads.as_ref().map_or(usize::MAX, Vec::len),
        });
        match transition {
            WorkerAction::Start => {
                self.runtime.lock().await.starting = Some(Starting::Resume);
                self.set_status(
                    "connecting",
                    "Starting the Linux worker and reconnecting to the private network",
                )
                .await;
                let bootstrap = self
                    .request(&config, "/device/bootstrap", Some(json!({})))
                    .await?;
                vm.start().await?;
                vm.configure(bootstrap).await?;
                {
                    let mut runtime = self.runtime.lock().await;
                    runtime.starting = None;
                }
                self.clear_drain_if_permitted()?;
            }
            WorkerAction::Drain => {
                self.set_status(
                    "draining",
                    if resize_pending {
                        "Waiting for running work before applying your new resource limits"
                    } else {
                        "Pausing new assignments and waiting for running work"
                    },
                )
                .await;
                let drain = self
                    .request(&config, "/device/drain", Some(json!({})))
                    .await;
                if info.reachable {
                    vm.renew_lease().await?;
                }
                drain?;
            }
            WorkerAction::Wait => {
                self.set_status("draining", "Waiting for running work to finish")
                    .await;
                // Evictions denied by a disruption budget must be retried as work finishes.
                let drain = self
                    .request(&config, "/device/drain", Some(json!({})))
                    .await;
                if info.reachable {
                    vm.renew_lease().await?;
                }
                drain?;
            }
            WorkerAction::Stop => {
                let deadline_expired = draining_since.is_some_and(|since| {
                    now.saturating_sub(since) >= u64::from(config.policy.drain_seconds)
                });
                if deadline_expired {
                    return self.finish_immediate_stop().await;
                } else {
                    vm.stop().await?;
                }
                self.clear_drain()?;
                self.runtime.lock().await.worker.running = false;
            }
            WorkerAction::Keep if info.running => {
                if draining_since.is_some() {
                    self.request(&config, "/device/resume", Some(json!({})))
                        .await?;
                    self.clear_drain_if_permitted()?;
                }
                vm.renew_lease().await?;
                self.set_status("sharing", "Your worker is available for eligible workloads")
                    .await;
            }
            WorkerAction::Keep => {
                let remote_paused = self.runtime.lock().await.remote_paused;
                self.set_status(
                    "paused",
                    if remote_paused {
                        "Paused from the fleet dashboard".into()
                    } else {
                        decision.reason
                    },
                )
                .await
            }
        }
        self.runtime.lock().await.workloads = workloads.unwrap_or_default();
        if let Some(error) = heartbeat_error {
            return Err(error);
        }
        self.heartbeat(&config).await?;
        Ok(())
    }
    fn begin_drain(&self) -> Result<Configuration> {
        self.store.update(|config| {
            config.draining_since.get_or_insert_with(now_seconds);
            Ok(())
        })
    }
    fn clear_drain(&self) -> Result<()> {
        self.store.update(|config| {
            config.draining_since = None;
            Ok(())
        })?;
        Ok(())
    }
    fn clear_drain_if_permitted(&self) -> Result<()> {
        self.store.update(|config| {
            if !config.stop_requested && !local_drain_required(config, &self.store.directory) {
                config.draining_since = None;
            }
            Ok(())
        })?;
        Ok(())
    }
    async fn heartbeat(&self, config: &Configuration) -> Result<()> {
        let runtime = self.runtime.lock().await.clone();
        let observation = crate::observe::observation(
            &self.store.directory,
            config
                .allocated_resources
                .as_ref()
                .map(|r| r.disk_gib)
                .unwrap_or(0),
        );
        let permitted = evaluate(&config.policy, &observation).allowed;
        let value=self.request(config,"/heartbeat",Some(json!({"state":runtime.state.unwrap_or_else(||"paused".into()),"reason":runtime.reason.unwrap_or_default(),"resources":config.policy.resources,
            "allowCi":config.policy.allow_ci,"allowServices":config.policy.allow_services,"permitted":permitted}))).await?;
        self.runtime.lock().await.remote_paused = value["remotePaused"].as_bool().unwrap_or(false);
        Ok(())
    }
}
fn now_seconds() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}
fn deadline_expired(config: &Configuration) -> bool {
    config.draining_since.is_some_and(|since| {
        now_seconds().saturating_sub(since) >= u64::from(config.policy.drain_seconds)
    })
}
fn local_drain_required(config: &Configuration, directory: &Path) -> bool {
    let allocated = config.allocated_resources.as_ref();
    let observation = crate::observe::observation(directory, allocated.map_or(0, |r| r.disk_gib));
    !evaluate(&config.policy, &observation).allowed
        || (config.vm_configured && allocated != Some(&config.policy.resources))
}
async fn checked(mut response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    anyhow::ensure!(
        response.content_length().unwrap_or(0) <= 1024 * 1024,
        "The controller response is too large"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("The controller returned an unreadable response")?
    {
        anyhow::ensure!(
            body.len() + chunk.len() <= 1024 * 1024,
            "The controller response is too large"
        );
        body.extend_from_slice(&chunk);
    }
    let value: Value =
        serde_json::from_slice(&body).context("The controller returned an unreadable response")?;
    anyhow::ensure!(
        status.is_success(),
        "{} ({status})",
        value["error"]
            .as_str()
            .unwrap_or("The controller could not complete this request")
    );
    Ok(value)
}
