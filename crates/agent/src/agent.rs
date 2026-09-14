mod storage_lifecycle;
mod storage_operations;
use crate::{worker_transition, Configuration, Store, Vm, VmInfo, WorkerAction, WorkerInput};
use anyhow::{Context, Result};
use nodeharbor_core::{evaluate, validate_policy, Policy, Resources};
use serde::Serialize;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::Mutex;
#[path = "remote.rs"]
mod remote;
#[path = "remote_storage.rs"]
mod remote_storage;

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
    system_pod_uids: Vec<String>,
    remote_paused: bool,
    starting: Option<Starting>,
    stopping_now: bool,
    application_update_ready: bool,
    update_idle_observed: bool,
}
#[derive(Clone)]
pub struct Agent {
    pub store: Store,
    runtime: Arc<Mutex<Runtime>>,
    operation: Arc<Mutex<()>>,
    client: reqwest::Client,
    runner: Option<Arc<dyn crate::Runner>>,
    activity: crate::activity::ActivityLog,
    storage_volumes: Option<Vec<crate::storage::Volume>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub configuration: nodeharbor_core::configuration::ConfigurationReport,
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub architecture: String,
    pub state: String,
    pub reason: String,
    pub policy: Policy,
    pub resources: Resources,
    pub storage: crate::storage::Inventory,
    pub allocated_resources: Option<Resources>,
    pub recreation_pending: bool,
    pub worker_replacement_available: bool,
    pub worker: VmInfo,
    pub enrolled: bool,
    pub controller_url: String,
    pub version: String,
    pub workloads: Vec<Value>,
}
impl Agent {
    /// Request maintenance without rewriting any owner sharing preferences.
    pub async fn begin_application_update(&self) -> Result<()> {
        let _operation = self.operation.lock().await;
        self.store.update(|config| {
            anyhow::ensure!(
                config.recreation.is_none()
                    && config.storage_operation.is_none()
                    && config.storage_lifecycle.maintenance.is_none()
                    && !config.prepare_requested
                    && !config.stop_requested,
                "Waiting for the current worker operation before updating"
            );
            config.application_update_pending = true;
            Ok(())
        })?;
        {
            let mut runtime = self.runtime.lock().await;
            runtime.application_update_ready = false;
            runtime.update_idle_observed = false;
        }
        // A previous operation's error is not the result of this inspection.
        // Readiness stays false until the supervisor verifies the stopped VM.
        self.set_status("draining", "Preparing the worker for an application update")
            .await;
        Ok(())
    }
    pub async fn application_update_ready(&self) -> Result<bool> {
        let pending = self.store.load()?.application_update_pending;
        let runtime = self.runtime.lock().await;
        anyhow::ensure!(
            runtime.state.as_deref() != Some("error"),
            "{}",
            runtime
                .reason
                .as_deref()
                .unwrap_or("Worker maintenance failed")
        );
        Ok(pending && runtime.application_update_ready)
    }
    /// Also used on startup after an interrupted installation. The current
    /// owner policy, including any pause made during the update, remains intact.
    pub async fn cancel_application_update(&self) -> Result<()> {
        let _operation = self.operation.lock().await;
        self.store.update(|config| {
            config.application_update_pending = false;
            Ok(())
        })?;
        let mut runtime = self.runtime.lock().await;
        runtime.application_update_ready = false;
        runtime.update_idle_observed = false;
        Ok(())
    }
    pub fn open(directory: &Path) -> Result<Self> {
        let new_settings = !directory.join("config.json").exists();
        let agent = Self::open_inner(directory, None)?;
        if new_settings && crate::VmProvider::native() == crate::VmProvider::Lima {
            agent.store.update(|config| {
                config.vm_provider = crate::VmProvider::Lima;
                config.format_version = config.format_version.max(2);
                Ok(())
            })?;
        }
        Ok(agent)
    }
    pub fn open_with_runner(directory: &Path, runner: Arc<dyn crate::Runner>) -> Result<Self> {
        Self::open_inner(directory, Some(runner))
    }
    /// Inject both the VM and discovered volumes for deterministic integration tests.
    #[doc(hidden)]
    pub fn open_with_runner_and_volumes(
        directory: &Path,
        runner: Arc<dyn crate::Runner>,
        volumes: Vec<crate::storage::Volume>,
    ) -> Result<Self> {
        let mut agent = Self::open_inner(directory, Some(runner))?;
        agent.storage_volumes = Some(volumes);
        Ok(agent)
    }
    fn open_inner(directory: &Path, runner: Option<Arc<dyn crate::Runner>>) -> Result<Self> {
        let activity = crate::activity::ActivityLog::default();
        Ok(Self {
            runner,
            storage_volumes: None,
            activity,
            store: Store::open(directory)?,
            runtime: Arc::new(Mutex::new(Runtime::default())),
            operation: Arc::new(Mutex::new(())),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    fn vm(&self, config: &Configuration) -> Result<Vm> {
        let runner = match &self.runner {
            Some(runner) => runner.clone(),
            None => Vm::native_runner(config.vm_provider, &self.store.directory)?,
        };
        Vm::managed(
            &config.device_id,
            &self.store.directory,
            Arc::new(crate::activity::ActivityRunner {
                inner: Arc::new(remote_storage::ConfigurationRunner {
                    store: self.store.clone(),
                    inner: runner,
                }),
                log: self.activity.clone(),
            }),
        )
    }
    pub fn local_vm(&self) -> Result<Vm> {
        self.vm(&self.store.load()?)
    }
    pub fn activity(&self) -> crate::activity::ActivitySnapshot {
        self.activity.snapshot()
    }
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let config = self.store.load()?;
        let worker_replacement_available = config.storage_locations.is_empty()
            && config.storage_operation.is_none()
            && config.storage_lifecycle.maintenance.is_none()
            && config.device_token.is_some()
            && (config.vm_created || self.vm(&config)?.has_receipt()?);
        let observation = self.observe_storage(&config);
        let storage_agent = self.clone();
        let storage_config = config.clone();
        let storage =
            tokio::task::spawn_blocking(move || storage_agent.discover_storage(&storage_config))
                .await?;
        let decision = evaluate(&config.policy, &observation);
        let mut runtime = self.runtime.lock().await.clone();
        runtime.worker.installed = config.vm_configured;
        let storage = self.storage_snapshot(&config, storage);
        Ok(Snapshot {
            configuration: self.configuration_report_for(
                &config,
                observation.resources.clone(),
                &storage,
            ),
            device_id: config.device_id,
            name: config.name,
            platform: std::env::consts::OS.into(),
            architecture: crate::observe::architecture().into(),
            state: runtime.state.unwrap_or_else(|| "paused".into()),
            reason: config
                .setup_notice
                .clone()
                .filter(|_| config.device_token.is_none())
                .unwrap_or_else(|| runtime.reason.unwrap_or(decision.reason)),
            policy: config.policy,
            resources: observation.resources,
            storage,
            allocated_resources: config.allocated_resources,
            recreation_pending: config.recreation.is_some(),
            worker_replacement_available,
            worker: runtime.worker,
            enrolled: config.device_token.is_some(),
            controller_url: config.controller_url.unwrap_or_default(),
            version: env!("CARGO_PKG_VERSION").into(),
            workloads: runtime.workloads,
        })
    }
    pub async fn save_policy(&self, policy: Policy) -> Result<Snapshot> {
        self.save_policy_versioned(policy, None).await
    }
    pub async fn save_policy_versioned(
        &self,
        policy: Policy,
        expected_revision: Option<u64>,
    ) -> Result<Snapshot> {
        let current = self.store.load()?;
        anyhow::ensure!(
            current.storage_operation.is_none() && current.storage_lifecycle.maintenance.is_none(),
            "Wait for the storage change to finish before editing sharing rules"
        );
        if !current.storage_locations.is_empty() {
            anyhow::ensure!(
                policy.resources.disk_gib
                    == current
                        .storage_locations
                        .iter()
                        .map(|location| location.allocation_gib)
                        .sum::<u64>(),
                "Use Storage locations to change the worker storage allowance"
            );
        }
        let host = self.snapshot().await?.resources;
        validate_policy(&policy, &host).map_err(anyhow::Error::msg)?;
        self.store.update(|config| {
            anyhow::ensure!(
                config.storage_operation.is_none()
                    && config.storage_lifecycle.maintenance.is_none(),
                "Wait for the storage change to finish before editing sharing rules"
            );
            if !config.storage_locations.is_empty() {
                anyhow::ensure!(
                    policy.resources.disk_gib
                        == config
                            .storage_locations
                            .iter()
                            .map(|location| location.allocation_gib)
                            .sum::<u64>(),
                    "Storage choices changed; reload sharing rules"
                );
            }
            anyhow::ensure!(
                expected_revision.is_none_or(|r| r == config.remote.revision),
                "Settings changed locally or remotely; reload current settings before saving"
            );
            anyhow::ensure!(
                config.recreation.is_none(),
                "Wait for worker replacement to finish before changing sharing rules"
            );
            if let Some(allocated) = &config.allocated_resources {
                anyhow::ensure!(
                    policy.resources.disk_gib >= allocated.disk_gib,
                    "Shrinking the worker disk requires explicitly recreating the VM"
                );
            }
            if policy.enabled {
                if !config.storage_locations.is_empty()
                    && config.storage_operation.is_none()
                    && config.storage_lifecycle.maintenance.is_none()
                {
                    self.validate_storage(config)?;
                }
                anyhow::ensure!(
                    config.device_token.is_some(),
                    "Connect this computer to a fleet before enabling sharing"
                );
            }
            config.policy = policy;
            if config.vm_configured && self.local_drain_required(config) {
                config.draining_since.get_or_insert_with(now_seconds);
            }
            Ok(())
        })?;
        self.snapshot().await
    }
    /// Called only by the explicit disk-deletion confirmation. The supervisor
    /// performs cleanup from the durable request; the UI never deletes a VM.
    pub async fn recreate_worker(&self, policy: Policy) -> Result<Snapshot> {
        self.recreate_worker_versioned(policy, None).await
    }
    pub async fn recreate_worker_versioned(
        &self,
        mut policy: Policy,
        expected_revision: Option<u64>,
    ) -> Result<Snapshot> {
        let current = self.store.load()?;
        anyhow::ensure!(current.storage_locations.is_empty() && current.storage_operation.is_none() && current.storage_lifecycle.maintenance.is_none(), "Replacing a worker with configured storage disks is unavailable; existing files have been preserved");
        let host = self.snapshot().await?.resources;
        policy.enabled = false;
        validate_policy(&policy, &host).map_err(anyhow::Error::msg)?;
        self.store.update(|config| {
            if !config.storage_locations.is_empty() {
                crate::storage::require_location_support(config.vm_provider)?;
            }
            anyhow::ensure!(
                expected_revision.is_none_or(|r| r == config.remote.revision),
                "Settings changed locally or remotely; reload current settings before replacement"
            );
            anyhow::ensure!(
                !config.application_update_pending,
                "Cancel the application update before replacing the worker"
            );
            anyhow::ensure!(
                config.recreation.is_none(),
                "Worker replacement is already pending"
            );
            anyhow::ensure!(
                config.device_token.is_some(),
                "Connect this computer to a fleet first"
            );
            let vm = self.vm(config)?;
            anyhow::ensure!(
                vm.has_receipt()?,
                "The worker has no ownership receipt; it has been left untouched"
            );
            config.policy = policy;
            config.prepare_requested = false;
            config.remote.repair_required = false;
            config.remote.applying = false;
            config.recreation = Some(crate::store::WorkerRecreation {
                request_id: uuid::Uuid::new_v4(),
                access_removed: false,
                target_provider: Some(crate::VmProvider::native()),
            });
            config.format_version = config.format_version.max(2);
            config.draining_since.get_or_insert_with(now_seconds);
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
            config.remote.revision = config
                .remote
                .revision
                .checked_add(1)
                .context("Configuration revision exhausted")?;
            anyhow::ensure!(
                action != "prepare" || !config.application_update_pending,
                "Cancel the application update before preparing the worker"
            );
            if action == "prepare" || action == "resume" {
                if !config.storage_locations.is_empty()
                    && config.storage_operation.is_none()
                    && config.storage_lifecycle.maintenance.is_none()
                {
                    self.validate_storage(config)?;
                }
                anyhow::ensure!(
                    config.recreation.is_none(),
                    "Wait for worker replacement to finish before preparing or sharing"
                );
                anyhow::ensure!(
                    config.device_token.is_some(),
                    "Connect this computer to a fleet first"
                );
            }
            if matches!(action, "prepare" | "resume") {
                anyhow::ensure!(
                    !config.storage_lifecycle.disabled,
                    "Worker storage is disabled; configure storage before preparing the worker"
                );
                if let Some(operation) = &mut config.storage_lifecycle.maintenance {
                    operation.paused = false;
                }

                if let Some(operation) = &mut config.storage_operation {
                    operation.paused = false;
                }
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
                    if let Some(operation) = &mut config.storage_lifecycle.maintenance {
                        operation.paused = true;
                    }
                    if let Some(operation) = &mut config.storage_operation {
                        operation.paused = true;
                    }
                    config.policy.enabled = false;
                    config.prepare_requested = false;
                    if config.vm_created {
                        config.draining_since.get_or_insert_with(now_seconds);
                    }
                }
                "stop" => {
                    if let Some(operation) = &mut config.storage_lifecycle.maintenance {
                        operation.paused = true;
                    }
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
        self.store.check_runtime_authority()?;
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
        let response = checked(
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
        .await?;
        self.store.check_runtime_authority()?;
        Ok(response)
    }
    async fn set_status(&self, state: &str, reason: impl Into<String>) {
        let reason = reason.into();
        let mut runtime = self.runtime.lock().await;
        if runtime.state.as_deref() != Some(state) || runtime.reason.as_deref() != Some(&reason) {
            self.activity.record(
                if state == "error" { "error" } else { "info" },
                "agent",
                &reason,
            );
        }
        runtime.state = Some(state.into());
        runtime.reason = Some(reason);
    }
    pub async fn run(&self) -> Result<()> {
        let _lock = self.store.supervisor_lock()?;
        loop {
            if let Err(error) = self.tick().await {
                self.set_status("error", self.diagnostic_message(&error.to_string()))
                    .await;
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
        let result = self.tick_operation().await;
        if let Err(error) = &result {
            self.activity.record(
                "error",
                "agent",
                &self.diagnostic_message(&error.to_string()),
            );
        }
        self.activity.finish_step();
        result
    }
    fn diagnostic_message(&self, message: &str) -> String {
        let secrets = self
            .store
            .load()
            .ok()
            .and_then(|config| config.device_token)
            .into_iter()
            .collect::<Vec<_>>();
        crate::activity::redact(message, &secrets)
    }
    async fn tick_operation(&self) -> Result<()> {
        let config = self.store.load()?;
        if config.stop_requested || deadline_interrupts(&config) {
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
        let explicit_stop = self.store.load()?.stop_requested;
        self.runtime.lock().await.stopping_now = true;
        let result = async {
            let config = self.store.update(|current| {
                // A failed shutdown is retried even after the agent restarts.
                current.stop_requested = true;
                Ok(())
            })?;
            let vm = self.vm(&config)?;
            if config.vm_created || vm.has_receipt()? {
                let info = vm.info().await?;
                self.runtime.lock().await.worker = info.clone();
                if info.running {
                    vm.stop_now().await?;
                }
            }
            self.store.update(|current| {
                current.stop_requested = false;
                current.draining_since = None;
                if explicit_stop {
                    if let Some(operation) = &mut current.storage_operation {
                        operation.paused = true;
                    }
                }
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
            let lifecycle_pause = config
                .storage_lifecycle
                .maintenance
                .as_ref()
                .is_some_and(|op| op.paused);
            let storage_pause = config
                .storage_operation
                .as_ref()
                .is_some_and(|operation| operation.paused && operation.phase != "pending");
            if config.stop_requested
                || pause
                || storage_pause
                || lifecycle_pause
                || deadline_interrupts(&config)
            {
                return Ok(());
            }
        }
    }
    async fn tick_inner(&self) -> Result<()> {
        self.recover_configuration()?;
        let config = self.store.load()?;
        self.runtime.lock().await.application_update_ready = false;
        if config.device_token.is_none() {
            anyhow::ensure!(
                !config.application_update_pending
                    || (!config.vm_created && !self.vm(&config)?.has_receipt()?),
                "Reconnect this worker to its fleet before updating NodeHarbor"
            );
            self.runtime.lock().await.application_update_ready = config.application_update_pending;
            self.set_status(
                "paused",
                "Connect this computer to a fleet to prepare its worker",
            )
            .await;
            return Ok(());
        }
        if config
            .remote
            .pending
            .as_ref()
            .is_some_and(|p| p.edit.operation.is_some())
        {
            return self.process_remote_storage().await;
        }
        let config = self.resolve_initial_storage(&config).await?;
        let vm = self.vm(&config)?;
        let owned = config.vm_created || vm.has_receipt()?;
        if config.stop_requested {
            return self.finish_immediate_stop().await;
        }
        if config.storage_lifecycle.maintenance.is_some() {
            return self.maintenance_tick(&config, &vm).await;
        }
        self.cleanup_backups().await?;
        if config.storage_lifecycle.disabled {
            self.set_status(
                "paused",
                "Worker storage is disabled; host enrollment remains",
            )
            .await;
            let _ = self.heartbeat(&config).await;
            return Ok(());
        }
        if config.storage_operation.is_some() {
            return self.storage_operation_inner(&config, &vm).await;
        }
        if config.recreation.is_none() && self.missing_storage_tick(&config, &vm).await? {
            return Ok(());
        }
        if let Err(error) = self.validate_storage(&config) {
            if vm.has_receipt()? && vm.info().await?.running {
                vm.stop_now().await?;
            }
            self.set_status("error", error.to_string()).await;
            let _ = self.heartbeat(&config).await;
            return Err(error);
        }
        if config.recreation.is_some() {
            return self.recreate_inner(&config, &vm).await;
        }
        if config.remote.repair_required {
            let _ = self.heartbeat(&config).await;
            anyhow::bail!("Worker allocation is unverified. Inspect the stopped worker locally or replace it before sharing");
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
            let info = if owned {
                vm.info().await?
            } else {
                VmInfo::default()
            };
            self.runtime.lock().await.worker = info.clone();
            if info.running && !info.reachable {
                self.set_status(
                    "error",
                    "The worker is powered on but the VM runtime cannot reach it. Use Stop now, then Prepare worker to retry. If this repeats, check the host's VM networking.",
                )
                .await;
                return Ok(());
            }
            self.set_status(
                "preparing",
                "Preparing the Linux worker within your resource budget",
            )
            .await;
            let exists = info.installed;
            if !exists {
                let observation = self.observe_storage(&config);
                validate_policy(&config.policy, &observation.resources)
                    .map_err(anyhow::Error::msg)?;
                if config.storage_locations.is_empty() {
                    vm.create(
                        &config.policy.resources,
                        &self.store.directory,
                        crate::guest_files(),
                    )
                    .await?;
                } else {
                    self.create_storage_worker(&config, &vm).await?;
                }
            } else if config.allocated_resources.as_ref() != Some(&config.policy.resources) {
                // A retry may follow a failed launch or changed settings. Apply
                // the budget to the existing VM before recording it as allocated.
                if info.running {
                    vm.stop().await?;
                }
                if config.storage_locations.is_empty() {
                    vm.resize(&config.policy.resources).await?;
                } else {
                    vm.resize_compute(&config.policy.resources).await?;
                }
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
            self.finish_initial_storage(&config, &vm).await?;
            // Bootstrap credentials must be fresh after a potentially slow first image download.
            self.activity
                .begin_step("Requesting access to the fleet", Some(20));
            let bootstrap = self
                .request(&config, "/device/bootstrap", Some(json!({})))
                .await?;
            vm.configure(bootstrap).await?;
            self.store.update(|c| {
                c.prepare_requested = false;
                c.vm_configured = true;
                if c.vm_provider == crate::VmProvider::Multipass && c.storage_lifecycle.initializing
                {
                    c.storage_generation = c
                        .storage_generation
                        .checked_add(1)
                        .context("Storage generation overflow")?;
                    c.storage_lifecycle.initializing = false;
                }
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
        if info.stopped {
            self.cleanup_storage_copies(&config, &vm).await;
        }
        if config.application_update_pending && !info.running {
            self.runtime.lock().await.application_update_ready = true;
            self.set_status("paused", "Worker stopped for an application update")
                .await;
            return Ok(());
        }
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
        if config.remote.pending.is_some() && !info.running {
            self.apply_configuration().await?;
            let _ = self.heartbeat(&self.store.load()?).await;
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
            if config.storage_locations.is_empty() {
                vm.resize(&config.policy.resources).await?;
            } else {
                vm.resize_compute(&config.policy.resources).await?;
            }
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
        if info.running && self.local_drain_required(&config) {
            let current = self.begin_drain()?;
            if deadline_expired(&current) {
                return self.finish_immediate_stop().await;
            }
        } else if !info.running {
            self.clear_drain()?;
        }
        if config.application_update_pending {
            self.set_status(
                "draining",
                "Waiting for running jobs before updating NodeHarbor",
            )
            .await;
        }
        let heartbeat_error = self.heartbeat(&self.store.load()?).await.err();
        // Owner actions can arrive from another process during the request.
        let config = self.store.load()?;
        let resize_pending = config.vm_configured
            && config.allocated_resources.as_ref() != Some(&config.policy.resources);
        let observation = self.observe_storage(&config);
        let decision = evaluate(&config.policy, &observation);
        let allowed = decision.allowed
            && config.remote.pending.is_none()
            && !resize_pending
            && heartbeat_error.is_none()
            && !self.runtime.lock().await.remote_paused;
        if config.application_update_pending
            && decision.allowed
            && !resize_pending
            && !self.runtime.lock().await.remote_paused
        {
            // Automatic updates never use owner drain deadlines or evictions.
            // Unknown connectivity or inventory cannot be interpreted as idle.
            if info.reachable {
                vm.renew_lease().await?;
            }
            if let Some(error) = heartbeat_error {
                return Err(error);
            }
            let maintenance = self
                .request(&config, "/device/maintenance", Some(json!({})))
                .await?;
            self.remember_system_pods(&maintenance).await?;
            let bound = maintenance["workloads"]
                .as_u64()
                .context("Missing maintenance workload inventory")?;
            anyhow::ensure!(info.reachable, "Cannot inspect the worker before updating");
            let inventory = vm
                .workload_inventory(&system_pod_uids(&maintenance)?)
                .await?;
            let idle = bound == 0 && inventory.workloads.is_empty();
            let previously_idle = {
                let mut runtime = self.runtime.lock().await;
                runtime.workloads = inventory.into_visible();
                let previous = runtime.update_idle_observed;
                runtime.update_idle_observed = idle;
                previous
            };
            // Recheck after a supervisor interval to include assignments already
            // in flight when Kubernetes accepted the cordon.
            if idle && previously_idle {
                vm.stop().await?;
                let mut runtime = self.runtime.lock().await;
                runtime.worker.running = false;
                runtime.workloads.clear();
                runtime.application_update_ready = true;
            }
            return Ok(());
        }
        if info.running && !allowed {
            let current = self.begin_drain()?;
            if deadline_expired(&current) {
                return self.finish_immediate_stop().await;
            }
        }
        // An unreachable guest has an unknown workload count, not an empty one.
        // Keep the grace period, but never let a failed inspection cancel its deadline.
        let inventory = if info.running && info.reachable {
            let system_pods = self.runtime.lock().await.system_pod_uids.clone();
            vm.workload_inventory(&system_pods).await.ok()
        } else if info.running {
            None
        } else {
            Some(crate::WorkloadInventory::default())
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
            workloads: inventory
                .as_ref()
                .map_or(usize::MAX, |items| items.workloads.len()),
        });
        match transition {
            WorkerAction::Start => {
                self.runtime.lock().await.starting = Some(Starting::Resume);
                self.set_status(
                    "connecting",
                    "Starting the Linux worker and reconnecting to the private network",
                )
                .await;
                self.activity
                    .begin_step("Requesting access to the fleet", Some(20));
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
                self.remember_system_pods(&drain?).await?;
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
                self.remember_system_pods(&drain?).await?;
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
        {
            let mut runtime = self.runtime.lock().await;
            runtime.workloads = if runtime.worker.running {
                inventory
                    .map(crate::WorkloadInventory::into_visible)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        }
        if let Some(error) = heartbeat_error {
            return Err(error);
        }
        self.heartbeat(&config).await?;
        Ok(())
    }
    async fn remember_system_pods(&self, response: &Value) -> Result<()> {
        self.runtime.lock().await.system_pod_uids = system_pod_uids(response)?;
        Ok(())
    }
    async fn recreate_inner(&self, config: &Configuration, vm: &Vm) -> Result<()> {
        let request = config
            .recreation
            .as_ref()
            .context("No worker replacement is pending")?;
        let info = vm.info().await?;
        self.runtime.lock().await.worker = info.clone();
        if info.running {
            self.set_status(
                "draining",
                "Draining running work before replacing the worker disk",
            )
            .await;
            let drain = self.request(config, "/device/drain", Some(json!({}))).await;
            if info.reachable {
                vm.renew_lease().await?;
                let exclusions = drain
                    .as_ref()
                    .ok()
                    .map(system_pod_uids)
                    .transpose()?
                    .unwrap_or_default();
                let inventory = vm.workload_inventory(&exclusions).await?;
                let empty = inventory.workloads.is_empty();
                self.runtime.lock().await.workloads = inventory.into_visible();
                if empty && drain.is_ok() {
                    vm.stop().await?;
                    self.clear_drain()?;
                    let mut runtime = self.runtime.lock().await;
                    runtime.worker.running = false;
                    runtime.workloads.clear();
                }
            }
            drain?;
            return Ok(());
        }
        self.clear_drain()?;
        self.set_status(
            "replacing",
            "Removing the previous worker's access and disk",
        )
        .await;
        if !request.access_removed {
            self.request(
                config,
                "/device/reset",
                Some(json!({"requestId":request.request_id})),
            )
            .await?;
            self.store.update(|current| {
                current
                    .recreation
                    .as_mut()
                    .context("Worker replacement is no longer pending")?
                    .access_removed = true;
                Ok(())
            })?;
        }
        vm.remove().await?;
        self.store.update(|current| {
            current.vm_provider = request.target_provider.unwrap_or(config.vm_provider);
            if current.vm_provider == crate::VmProvider::Lima {
                current.format_version = current.format_version.max(2);
            }
            current.vm_created = false;
            current.vm_configured = false;
            current.allocated_resources = None;
            current.prepare_requested = false;
            current.policy.enabled = false;
            current.recreation = None;
            current.draining_since = None;
            Ok(())
        })?;
        {
            let mut runtime = self.runtime.lock().await;
            runtime.worker = VmInfo::default();
            runtime.workloads.clear();
        }
        self.set_status(
            "paused",
            "Previous worker removed. Prepare the replacement when you are ready",
        )
        .await;
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
            if !config.stop_requested && !self.local_drain_required(config) {
                config.draining_since = None;
            }
            Ok(())
        })?;
        Ok(())
    }
    async fn heartbeat(&self, config: &Configuration) -> Result<()> {
        let runtime = self.runtime.lock().await.clone();
        let observation = self.observe_storage(config);
        let permitted = evaluate(&config.policy, &observation).allowed
            && !config.application_update_pending
            && config.storage_operation.is_none()
            && config.storage_lifecycle.maintenance.is_none();
        let mut resources = config.policy.resources.clone();
        if config.storage_lifecycle.disabled
            || config.storage_lifecycle.missing.is_some()
            || config.storage_lifecycle.maintenance.is_some()
        {
            resources.disk_gib = 0;
        } else if !config.storage_locations.is_empty() {
            resources.disk_gib = config
                .storage_locations
                .iter()
                .map(|l| l.allocation_gib)
                .sum();
        }
        let reason = if config.storage_lifecycle.missing.is_some()
            || config.storage_lifecycle.maintenance.is_some()
        {
            "Storage maintenance; see this computer for details".to_owned()
        } else {
            runtime.reason.unwrap_or_default()
        };
        let permitted = permitted
            && !config.storage_lifecycle.disabled
            && config.storage_lifecycle.missing.is_none();
        let storage = self.storage_snapshot(config, self.discover_storage(config));
        let mut report =
            self.configuration_report_for(config, observation.resources.clone(), &storage);
        if !report.consent {
            report.policy = None;
            report.hardware = None;
            report.storage_inventory.clear();
            report.storage = None;
        }
        let value=self.request(config,"/heartbeat",Some(json!({"configuration":report,"storageGeneration":config.storage_generation,"state":runtime.state.unwrap_or_else(||"paused".into()),"reason":reason,"resources":resources,
            "allowCi":config.policy.allow_ci,"allowServices":config.policy.allow_services,"permitted":permitted && config.remote.pending.is_none() && !config.remote.repair_required}))).await?;
        self.runtime.lock().await.remote_paused = value["remotePaused"].as_bool().unwrap_or(false);
        if let Some(request) = value.get("configurationRequest").filter(|v| !v.is_null()) {
            let command: nodeharbor_core::configuration::ConfigurationCommand =
                serde_json::from_value(request.clone())
                    .context("Invalid configuration request from controller")?;
            if let Err(error) = self.receive_configuration(command.clone()) {
                self.reject_configuration(&command, &error.to_string())?;
            }
        }
        Ok(())
    }
}
fn system_pod_uids(response: &Value) -> Result<Vec<String>> {
    let Some(value) = response.get("systemPodUids") else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .context("The controller returned an invalid system pod inventory")?;
    anyhow::ensure!(
        items.len() <= 16,
        "The controller returned too many system pods"
    );
    items
        .iter()
        .map(|item| {
            let uid = item.as_str().context("Missing system pod identity")?;
            uuid::Uuid::parse_str(uid).context("Invalid system pod identity")?;
            Ok(uid.to_owned())
        })
        .collect()
}
fn now_seconds() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}
fn deadline_expired(config: &Configuration) -> bool {
    config.draining_since.is_some_and(|since| {
        now_seconds().saturating_sub(since) >= u64::from(config.policy.drain_seconds)
    })
}
fn deadline_interrupts(config: &Configuration) -> bool {
    // Storage drains coordinate admission before honoring their own deadline.
    // Do not interrupt that coordination or discard the durable maintenance phase.
    config.storage_operation.is_none()
        && config.storage_lifecycle.maintenance.is_none()
        && deadline_expired(config)
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

#[cfg(test)]
mod setup_notice_tests {
    #[tokio::test]
    async fn enrollment_notice_remains_visible_after_the_supervisor_reports_paused() {
        let dir = tempfile::tempdir().unwrap();
        let agent = super::Agent::open(dir.path()).unwrap();
        agent
            .store
            .update(|config| {
                config.setup_notice = Some("Please enroll again to prepare a Lima worker".into());
                Ok(())
            })
            .unwrap();
        agent.set_status("paused", "Sharing is switched off").await;
        assert!(agent
            .snapshot()
            .await
            .unwrap()
            .reason
            .contains("enroll again"));
    }
}
