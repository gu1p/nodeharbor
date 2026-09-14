use super::*;
use crate::storage::{ChangePlan, Operation, OperationStatus, Selection};

impl Agent {
    /// A local, read-only review. This contract can also be used by an opted-in
    /// owner configuration transport; host paths are never added to heartbeats.
    pub(super) async fn preview_storage_growth(
        &self,
        selections: Vec<Selection>,
    ) -> Result<ChangePlan> {
        let config = self.store.load()?;
        crate::storage::require_location_support(config.vm_provider)?;
        anyhow::ensure!(
            config.storage_operation.is_none() && config.storage_lifecycle.maintenance.is_none(),
            "Wait for the current storage change to finish"
        );
        let requires_restart = config.vm_created || self.vm(&config)?.has_receipt()?;
        let directory = self.store.directory.clone();
        let agent = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut inventory = agent.discover_storage(&config);
            agent.reserve_boot_storage(&config, &mut inventory)?;
            let default = inventory
                .default_directory
                .context("The runtime does not report its managed storage directory")?;
            let physical = crate::storage::allocated_bytes(
                &directory,
                &config.device_id,
                &config.storage_locations,
            );
            let locations = crate::storage::plan_change(
                &selections,
                Path::new(&default),
                config.policy.resources.disk_gib,
                &inventory.volumes,
                &config.storage_locations,
                &physical,
            )?;
            Ok(ChangePlan {
                maintenance: None,
                revision: config.storage_revision,
                total_gib: locations
                    .iter()
                    .map(|location| location.allocation_gib)
                    .sum(),
                locations,
                requires_restart,
            })
        })
        .await?
    }

    /// Save setup choices or enqueue a durable maintenance operation. The
    /// supervisor owns VM changes; this method never starts or stops a worker.
    pub async fn apply_storage(&self, plan: ChangePlan) -> Result<Snapshot> {
        if plan.maintenance.is_some() {
            return self.apply_storage_maintenance(plan).await;
        }
        let _operation = self.operation.lock().await;
        let config = self.store.load()?;
        anyhow::ensure!(
            config.storage_revision == plan.revision,
            "Storage choices changed; review the changes again"
        );
        anyhow::ensure!(
            !config.application_update_pending
                && config.recreation.is_none()
                && config.storage_operation.is_none()
                && config.storage_lifecycle.maintenance.is_none(),
            "Wait for current worker maintenance to finish"
        );
        let selections = plan
            .locations
            .iter()
            .map(|location| Selection {
                id: config
                    .storage_locations
                    .iter()
                    .any(|old| old.id == location.id)
                    .then(|| location.id.clone()),
                directory: location.directory.clone(),
                allocation_gib: location.allocation_gib,
            })
            .collect();
        let mut checked = self.preview_storage(selections).await?;
        anyhow::ensure!(
            checked.maintenance.is_none(),
            "Replacement storage requires a verified backup review"
        );
        anyhow::ensure!(
            checked.requires_restart == plan.requires_restart,
            "The required worker restart changed; review storage again"
        );
        self.storage_runtime_preflight().await?;
        anyhow::ensure!(
            checked.total_gib == plan.total_gib && checked.locations.len() == plan.locations.len(),
            "Storage review no longer matches the requested allocation"
        );
        let mut ids = std::collections::BTreeSet::new();
        for location in &mut checked.locations {
            let reviewed = plan
                .locations
                .iter()
                .find(|reviewed| {
                    reviewed.directory == location.directory
                        && reviewed.volume_id == location.volume_id
                        && reviewed.allocation_gib == location.allocation_gib
                })
                .context("Storage volume changed; review the changes again")?;
            crate::lima_storage::disk_paths(
                &self.store.directory.join("lima"),
                &config.device_id,
                reviewed,
            )?;
            anyhow::ensure!(
                ids.insert(reviewed.id.clone()),
                "A disk cannot be selected twice"
            );
            location.id = reviewed.id.clone();
        }
        self.store.update(|current| {
            anyhow::ensure!(
                current.storage_revision == plan.revision && current.storage_operation.is_none(),
                "Storage choices changed; review the changes again"
            );
            anyhow::ensure!(
                !current.application_update_pending && current.recreation.is_none(),
                "Wait for current worker maintenance to finish"
            );
            anyhow::ensure!(
                checked.requires_restart
                    == (current.vm_created || self.vm(current)?.has_receipt()?),
                "The required worker restart changed; review storage again"
            );
            current.format_version = 4;
            current.storage_boot_gib = self.boot_disk_gib(current);
            current.storage_revision = current
                .storage_revision
                .checked_add(1)
                .context("Storage revision overflow")?;
            if checked.requires_restart {
                current.storage_operation = Some(Operation {
                    previous: current.storage_locations.clone(),
                    target: checked.locations.clone(),
                    generation: current
                        .storage_generation
                        .checked_add(1)
                        .context("Storage generation overflow")?,
                    phase: "pending".into(),
                    paused: false,
                });
                current.draining_since.get_or_insert_with(now_seconds);
            } else {
                current.storage_lifecycle.initializing = true;
                if current.storage_lifecycle.disabled {
                    current.storage_lifecycle.pool_id = Some(uuid::Uuid::new_v4());
                }
                current.storage_lifecycle.disabled = false;
                current.storage_lifecycle.configured_locations = checked.locations.clone();
                current.storage_locations = checked.locations.clone();
                current.policy.resources.disk_gib = checked.total_gib;
            }
            Ok(())
        })?;
        self.snapshot().await
    }

    fn boot_disk_gib(&self, config: &Configuration) -> u64 {
        if config.storage_boot_gib > 0 {
            config.storage_boot_gib
        } else {
            config
                .allocated_resources
                .as_ref()
                .map_or(16, |resources| resources.disk_gib)
        }
    }

    pub(super) fn discover_storage(&self, config: &Configuration) -> crate::storage::Inventory {
        let mut inventory = crate::storage::inventory(
            config.vm_provider,
            &self.store.directory,
            &config.storage_locations,
            self.boot_disk_gib(config),
        );
        if let Some(volumes) = &self.storage_volumes {
            inventory.volumes = volumes.clone();
            inventory.locations =
                crate::storage::inspect_locations(&config.storage_locations, volumes);
        }
        inventory
    }

    pub(super) fn reserve_boot_storage(
        &self,
        config: &Configuration,
        inventory: &mut crate::storage::Inventory,
    ) -> Result<()> {
        let allocated = if self.vm(config)?.has_receipt()? {
            crate::storage::allocated_system_bytes(&self.store.directory)? / (1024 * 1024 * 1024)
        } else {
            0
        };
        let remaining = self.boot_disk_gib(config).saturating_sub(allocated);
        crate::storage::reserve_system_disk(
            &mut inventory.volumes,
            &self.store.directory,
            remaining,
        )
    }

    pub(super) async fn storage_runtime_preflight(&self) -> Result<()> {
        if self.runner.is_none() {
            crate::runtime_platform::preflight().await?;
            let executable = std::env::current_exe()?;
            anyhow::ensure!(
                crate::runtime_platform::bundled_program(std::env::consts::OS, &executable)?
                    .is_file(),
                "The packaged Lima runtime is unavailable; repair the NodeHarbor installation"
            );
        }
        Ok(())
    }

    pub(super) async fn resolve_initial_storage(
        &self,
        config: &Configuration,
    ) -> Result<Configuration> {
        if config.vm_provider != crate::VmProvider::Lima
            || config.storage_lifecycle.disabled
            || config.storage_lifecycle.maintenance.is_some()
            || !config.prepare_requested
            || config.vm_created
            || self.vm(config)?.has_receipt()?
            || !config.storage_locations.is_empty()
        {
            return Ok(config.clone());
        }
        let plan = self.preview_storage(Vec::new()).await?;
        self.storage_runtime_preflight().await?;
        self.store.update(|current| {
            anyhow::ensure!(
                current.storage_revision == plan.revision
                    && current.storage_locations.is_empty()
                    && !current.vm_created,
                "Storage settings changed during preparation; retry"
            );
            current.format_version = 4;
            current.storage_boot_gib = 16;
            current.storage_locations = plan.locations;
            current.storage_revision = current
                .storage_revision
                .checked_add(1)
                .context("Storage revision overflow")?;
            Ok(())
        })
    }

    pub(super) async fn create_storage_worker(
        &self,
        config: &Configuration,
        vm: &Vm,
    ) -> Result<()> {
        self.storage_runtime_preflight().await?;
        self.validate_storage(config)?;
        let adapter = vm.storage_adapter(&self.store.directory)?;
        for location in &config.storage_locations {
            // Creation reopens only an exactly matching owned image. Its receipt
            // and volume checks protect retries after an interrupted first boot.
            adapter.create(location).await?;
        }
        vm.create_with_storage(
            &config.policy.resources,
            &self.store.directory,
            crate::guest_files(),
            &config.storage_locations,
            &config
                .storage_lifecycle
                .pool_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| config.device_id.clone()),
            config
                .storage_generation
                .checked_add(1)
                .context("Storage generation overflow")?,
        )
        .await
    }

    pub(super) async fn finish_initial_storage(
        &self,
        config: &Configuration,
        vm: &Vm,
    ) -> Result<()> {
        if config.storage_locations.is_empty()
            || (config.storage_generation > 0 && !config.storage_lifecycle.initializing)
        {
            return Ok(());
        }
        vm.prepare_storage_guest().await?;
        let generation = config
            .storage_generation
            .checked_add(1)
            .context("Storage generation overflow")?;
        let pool_id = config
            .storage_lifecycle
            .pool_id
            .map(|p| p.to_string())
            .unwrap_or_else(|| config.device_id.clone());
        let request = crate::lima::storage_request(
            &config.device_id,
            &pool_id,
            &config.storage_locations,
            &[],
            generation,
        );
        vm.storage_command("apply", Some(request)).await?;
        vm.storage_command("migrate", None).await?;
        let state = vm.storage_command("check", None).await?;
        verify_pool_identity(
            &config.device_id,
            &pool_id,
            generation,
            &config.storage_locations,
            &state,
        )?;
        self.store.update(|current| {
            anyhow::ensure!(
                current.storage_locations == config.storage_locations,
                "Storage choices changed during preparation"
            );
            current.format_version = 4;
            current.storage_generation = generation;
            current.storage_lifecycle.initializing = false;
            Ok(())
        })?;
        Ok(())
    }

    pub(super) fn local_drain_required(&self, config: &Configuration) -> bool {
        config.remote.pending.is_some()
            || !evaluate(&config.policy, &self.observe_storage(config)).allowed
            || (config.vm_configured
                && config.allocated_resources.as_ref() != Some(&config.policy.resources))
    }

    pub(super) fn storage_snapshot(
        &self,
        config: &Configuration,
        mut inventory: crate::storage::Inventory,
    ) -> crate::storage::Inventory {
        inventory.revision = config.storage_revision;
        if config.storage_generation == 0
            && !config.vm_created
            && self.validate_storage(config).is_ok()
        {
            for location in &mut inventory.locations {
                location.available = true;
                location.reason = "Will be created when the worker is prepared".into();
            }
        }
        inventory.retained_copies =
            crate::storage::inspect_locations(&config.storage_retained, &inventory.volumes);
        inventory.operation = config
            .storage_operation
            .as_ref()
            .map(|operation| OperationStatus {
                phase: operation.phase.clone(),
                message: if operation.paused {
                    "Storage change paused. Start sharing or prepare the worker to continue."
                } else {
                    match operation.phase.as_str() {
                        "pending" => "Waiting for running work before changing storage",
                        "disks" => {
                            "Preparing and verifying storage files while the worker is stopped"
                        }
                        "guest" => "Applying and checking the combined worker storage",
                        _ => "Verifying the storage change before completing it",
                    }
                }
                .into(),
            });
        self.lifecycle_snapshot(config, &mut inventory);
        inventory
    }

    pub(super) fn validate_storage(&self, config: &Configuration) -> Result<()> {
        if config.storage_locations.is_empty() {
            return Ok(());
        }
        crate::storage::require_location_support(config.vm_provider)?;
        let mut inventory = self.discover_storage(config);
        self.reserve_boot_storage(config, &mut inventory)?;
        let selections = config
            .storage_locations
            .iter()
            .map(|location| Selection {
                id: Some(location.id.clone()),
                directory: location.directory.clone(),
                allocation_gib: location.allocation_gib,
            })
            .collect::<Vec<_>>();
        let physical = crate::storage::allocated_bytes(
            &self.store.directory,
            &config.device_id,
            &config.storage_locations,
        );
        let checked = crate::storage::plan_change(
            &selections,
            &self.store.directory.join("storage"),
            config.policy.resources.disk_gib,
            &inventory.volumes,
            &config.storage_locations,
            &physical,
        )?;
        anyhow::ensure!(checked == config.storage_locations, "The original storage volume is unavailable or has been replaced; storage will not be redirected");
        if config.storage_generation > 0 && !config.storage_lifecycle.initializing {
            anyhow::ensure!(
                inventory
                    .locations
                    .iter()
                    .all(|location| location.available),
                "A configured storage volume is unavailable; reconnect the original volume"
            );
            let adapter = self.vm(config)?.storage_adapter(&self.store.directory)?;
            for location in &config.storage_locations {
                adapter.validate(location)?;
            }
        }
        Ok(())
    }

    pub(super) fn observe_storage(&self, config: &Configuration) -> nodeharbor_core::Observation {
        let allocated = config
            .allocated_resources
            .as_ref()
            .map_or(0, |resources| resources.disk_gib);
        let mut observation = crate::observe::observation(&self.store.directory, allocated);
        if !config.storage_locations.is_empty() {
            // Per-volume validation accounts for sparse growth and shared space.
            // Do not substitute free space beside the app's settings directory.
            observation.resources.disk_gib = if self.validate_storage(config).is_ok() {
                config
                    .storage_locations
                    .iter()
                    .map(|location| location.allocation_gib)
                    .sum::<u64>()
                    .saturating_add(10)
            } else {
                0
            };
        }
        observation
    }

    pub(super) async fn storage_operation_inner(
        &self,
        config: &Configuration,
        vm: &Vm,
    ) -> Result<()> {
        anyhow::ensure!(
            vm.has_receipt()?,
            "Storage maintenance requires the existing worker ownership receipt"
        );
        let operation = config
            .storage_operation
            .as_ref()
            .context("No storage change is pending")?;
        anyhow::ensure!(
            !operation.target.is_empty()
                && operation.target.len() <= 16
                && matches!(
                    operation.phase.as_str(),
                    "pending" | "disks" | "guest" | "verified"
                ),
            "Invalid saved storage operation"
        );
        self.storage_runtime_preflight().await?;
        let adapter = vm.storage_adapter(&self.store.directory)?;
        let info = vm.info().await?;
        self.runtime.lock().await.worker = info.clone();
        anyhow::ensure!(
            info.installed,
            "The existing worker is unavailable; its storage has been preserved"
        );
        if operation.paused {
            self.set_status("paused", "Storage maintenance paused by owner")
                .await;
            // A controller outage cannot prevent the owner from stopping work.
            let heartbeat = self.heartbeat(config).await;
            if info.running {
                if operation.phase == "pending" {
                    if heartbeat.is_err() {
                        vm.stop_now().await?;
                        return Ok(());
                    }
                    self.begin_drain()?;
                    let drain = self
                        .request(config, "/device/drain", Some(json!({})))
                        .await?;
                    if !info.reachable {
                        return Ok(());
                    }
                    vm.renew_lease().await?;
                    if !vm
                        .workload_inventory(&system_pod_uids(&drain)?)
                        .await?
                        .workloads
                        .is_empty()
                        && !deadline_expired(&self.store.load()?)
                    {
                        return Ok(());
                    }
                }
                vm.stop().await?;
                self.clear_drain()?;
            }
            self.set_status(
                "paused",
                "Storage change paused. Start sharing or prepare the worker to continue.",
            )
            .await;
            return Ok(());
        }
        self.set_status(
            "preparing",
            "Storage maintenance; new assignments are disabled",
        )
        .await;
        if let Err(error) = self.heartbeat(config).await {
            if info.running {
                vm.stop_now().await?;
                self.runtime.lock().await.worker.running = false;
            }
            return Err(error);
        }
        if operation.phase == "pending" {
            self.validate_storage(config)?;
            // A stopped worker must also be cordoned before it can boot for
            // maintenance; its old service may still be enabled in the guest.
            let drain = self
                .request(config, "/device/drain", Some(json!({})))
                .await?;
            self.remember_system_pods(&drain).await?;
            if info.running {
                self.set_status(
                    "draining",
                    "Waiting for running work before changing storage",
                )
                .await;
                self.begin_drain()?;
                anyhow::ensure!(
                    info.reachable,
                    "Cannot inspect the worker before changing storage"
                );
                vm.renew_lease().await?;
                let inventory = vm.workload_inventory(&system_pod_uids(&drain)?).await?;
                let idle = inventory.workloads.is_empty();
                self.runtime.lock().await.workloads = inventory.into_visible();
                if !idle && !deadline_expired(&self.store.load()?) {
                    return Ok(());
                }
                vm.quiesce_storage().await?;
                vm.stop().await?;
            }
            self.clear_drain()?;
            self.storage_phase("disks")?;
            return Ok(());
        }
        if operation.phase == "disks" {
            anyhow::ensure!(
                info.stopped,
                "Stop the worker before preparing storage files"
            );
            self.set_status(
                "preparing",
                "Preparing and verifying the selected storage files",
            )
            .await;
            self.validate_storage_targets(config, operation)?;
            for target in &operation.target {
                if adapter.inspect(target).await.is_ok() {
                    continue;
                }
                match operation.previous.iter().find(|old| old.id == target.id) {
                    None => {
                        adapter.create(target).await?;
                    }
                    Some(old) if old.directory == target.directory => {
                        adapter.grow(old, target).await?;
                    }
                    Some(old) => {
                        let mut moved = target.clone();
                        moved.allocation_gib = old.allocation_gib;
                        if adapter.inspect(&moved).await.is_err() {
                            adapter.stage_move(old, &moved).await?;
                            adapter.commit_move(old, &moved).await?;
                        }
                        if moved.allocation_gib != target.allocation_gib {
                            adapter.grow(&moved, target).await?;
                        }
                    }
                }
            }
            vm.set_storage_configuration(
                &operation.target,
                &operation.previous,
                &config
                    .storage_lifecycle
                    .pool_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| config.device_id.clone()),
                operation.generation,
                true,
            )
            .await?;
            self.storage_phase("guest")?;
            return Ok(());
        }
        if operation.phase == "guest" {
            self.set_status(
                "preparing",
                "Applying and checking the combined worker storage",
            )
            .await;
            let mut target_config = config.clone();
            target_config.storage_locations = operation.target.clone();
            target_config.storage_generation = operation.generation;
            self.validate_storage(&target_config)?;
            for target in &operation.target {
                adapter.inspect(target).await?;
            }
            if !info.running {
                vm.start().await?;
            }
            vm.prepare_storage_guest().await?;
            let request = crate::lima::storage_request(
                &config.device_id,
                &config
                    .storage_lifecycle
                    .pool_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| config.device_id.clone()),
                &operation.target,
                &operation.previous,
                operation.generation,
            );
            vm.storage_command("apply", Some(request)).await?;
            vm.storage_command("migrate", None).await?;
            let state = vm.storage_command("check", None).await?;
            verify_pool_identity(
                &config.device_id,
                &config
                    .storage_lifecycle
                    .pool_id
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| config.device_id.clone()),
                operation.generation,
                &operation.target,
                &state,
            )?;
            // Retain existing access and identity; the normal bootstrap/configure
            // path updates Kubernetes locations and reinstates its service.
            let bootstrap = self
                .request(config, "/device/bootstrap", Some(json!({})))
                .await?;
            vm.configure(bootstrap).await?;
            vm.storage_command("check", None).await?;
            vm.stop().await?;
            self.storage_phase("verified")?;
            return Ok(());
        }
        anyhow::ensure!(
            info.stopped,
            "Stop the worker before completing the storage change"
        );
        self.store.update(|current| {
            let pending = current
                .storage_operation
                .as_ref()
                .context("Storage operation was interrupted")?;
            anyhow::ensure!(
                pending.generation == operation.generation && pending.phase == "verified",
                "Storage operation changed"
            );
            current.storage_lifecycle.configured_locations =
                crate::storage_lifecycle::configured_after_change(
                    &current.storage_lifecycle.configured_locations,
                    &current.storage_locations,
                    &operation.target,
                );
            current.storage_locations = operation.target.clone();
            current.storage_generation = operation.generation;
            current.policy.resources.disk_gib = operation
                .target
                .iter()
                .map(|location| location.allocation_gib)
                .sum();
            if let Some(resources) = &mut current.allocated_resources {
                resources.disk_gib = current.policy.resources.disk_gib;
            }
            current.storage_operation = None;
            for old in &operation.previous {
                if operation
                    .target
                    .iter()
                    .any(|target| target.id == old.id && target.directory != old.directory)
                    && !current.storage_retained.contains(old)
                {
                    current.storage_retained.push(old.clone());
                }
            }
            current.storage_revision = current
                .storage_revision
                .checked_add(1)
                .context("Storage revision overflow")?;
            current.draining_since = None;
            Ok(())
        })?;
        self.cleanup_storage_copies(&self.store.load()?, vm).await;
        self.set_status(
            "paused",
            "Storage updated and verified; your sharing rules still apply",
        )
        .await;
        Ok(())
    }

    fn storage_phase(&self, phase: &str) -> Result<()> {
        self.store.update(|config| {
            config
                .storage_operation
                .as_mut()
                .context("Storage operation was interrupted")?
                .phase = phase.into();
            Ok(())
        })?;
        Ok(())
    }

    fn validate_storage_targets(
        &self,
        config: &Configuration,
        operation: &Operation,
    ) -> Result<()> {
        let mut inventory = self.discover_storage(config);
        self.reserve_boot_storage(config, &mut inventory)?;
        let mut physical = crate::storage::allocated_bytes(
            &self.store.directory,
            &config.device_id,
            &operation.target,
        );
        let original = crate::storage::allocated_bytes(
            &self.store.directory,
            &config.device_id,
            &operation.previous,
        );
        for target in &operation.target {
            if operation.previous.iter().any(|old| {
                old.id == target.id
                    && old.directory == target.directory
                    && old.volume_id == target.volume_id
            }) {
                if let Some(bytes) = original.get(&target.id) {
                    physical.entry(target.id.clone()).or_insert(*bytes);
                }
            }
        }
        let selections = operation
            .target
            .iter()
            .map(|location| Selection {
                id: Some(location.id.clone()),
                directory: location.directory.clone(),
                allocation_gib: location.allocation_gib,
            })
            .collect::<Vec<_>>();
        let checked = crate::storage::plan_change(
            &selections,
            &self.store.directory.join("storage"),
            30,
            &inventory.volumes,
            &operation.target,
            &physical,
        )?;
        anyhow::ensure!(checked == operation.target, "A selected storage volume is unavailable or was replaced; storage will not be redirected");
        Ok(())
    }

    pub(super) async fn cleanup_storage_copies(&self, config: &Configuration, vm: &Vm) {
        if config.storage_retained.is_empty() || config.storage_generation == 0 {
            return;
        }
        let Ok(adapter) = vm.storage_adapter(&self.store.directory) else {
            return;
        };
        for old in &config.storage_retained {
            let Some(active) = config
                .storage_locations
                .iter()
                .find(|location| location.id == old.id && location.directory != old.directory)
            else {
                continue;
            };
            if adapter.inspect(active).await.is_err() {
                continue;
            }
            if adapter.remove_retained(old).await.is_ok() {
                let _ = self.store.update(|current| {
                    current.storage_retained.retain(|location| location != old);
                    Ok(())
                });
            }
        }
    }
}

pub(super) fn verify_pool_identity(
    device: &str,
    pool_id: &str,
    generation: u64,
    locations: &[crate::storage::Location],
    state: &Value,
) -> Result<()> {
    let total: u64 = locations
        .iter()
        .map(|location| location.allocation_gib << 30)
        .sum();
    let capacity = state["capacityBytes"]
        .as_u64()
        .context("Worker storage capacity is unavailable")?;
    anyhow::ensure!(
        state["deviceId"] == device
            && state["poolId"] == pool_id
            && state["generation"] == generation
            && state["migrationComplete"] == true,
        "Worker storage does not match the saved owner and configuration"
    );
    let disks = state["disks"]
        .as_array()
        .context("Worker storage membership is unavailable")?;
    anyhow::ensure!(
        disks.len() == locations.len()
            && locations
                .iter()
                .all(|location| disks.iter().any(|disk| disk["id"] == location.id
                    && disk["allocationBytes"] == (location.allocation_gib << 30))),
        "The worker is missing configured storage disks"
    );
    anyhow::ensure!(
        capacity <= total && capacity >= total.saturating_mul(85) / 100,
        "Actual worker storage capacity differs from the combined allocation"
    );
    Ok(())
}
