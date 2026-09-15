use super::sharing_settings::SharingSettings;
use super::*;
use crate::storage::{ChangePlan, Operation, OperationStatus, Selection};

impl Agent {
    /// A local, read-only review. This contract can also be used by an opted-in
    /// owner configuration transport; host paths are never added to heartbeats.
    pub(super) async fn preview_storage_growth(
        &self,
        selections: Vec<Selection>,
        prepared: Option<super::storage_layout::Prepared>,
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
            if prepared.is_none() {
                agent.reserve_boot_storage(&config, &mut inventory)?;
            }
            let default = inventory
                .default_directory
                .context("The runtime does not report its managed storage directory")?;
            let physical = crate::storage::allocated_bytes(
                &directory,
                &config.device_id,
                &config.storage_locations,
            );
            let locations = if let Some(prepared) = &prepared {
                prepared.data.clone()
            } else {
                crate::storage::plan_disks(
                    &selections,
                    Path::new(&default),
                    config.policy.resources.disk_gib,
                    &inventory.volumes,
                    &config.storage_locations,
                    &physical,
                )?
            };
            Ok(ChangePlan {
                layout: None,
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
        let _operation = self.operation.lock().await;
        self.apply_storage_inner(plan).await
    }

    pub(super) async fn apply_storage_inner(&self, plan: ChangePlan) -> Result<Snapshot> {
        self.apply_storage_settings(plan, None).await
    }

    pub(super) async fn apply_storage_settings(
        &self,
        plan: ChangePlan,
        settings: Option<SharingSettings>,
    ) -> Result<Snapshot> {
        if plan.maintenance.is_some() {
            return self
                .apply_storage_maintenance_settings(plan, settings)
                .await;
        }
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
                expected_volume_id: Some(location.volume_id.clone()),
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
        let layout = self.checked_layout(&config, &plan)?;
        let data = layout.data_locations(&checked.locations)?;
        self.store.update(|current| {
            if let Some(settings) = &settings {
                settings.validate(current)?;
            }
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
            current.format_version = 7;
            current.storage_boot_gib = layout.system_gib;
            current.storage_revision = current
                .storage_revision
                .checked_add(1)
                .context("Storage revision overflow")?;
            if checked.requires_restart {
                current.storage_operation = Some(Operation {
                    layout: Some(layout.clone()),
                    previous: current.storage_locations.clone(),
                    target: data.clone(),
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
                current.storage_layout = Some(layout.clone());
                current.storage_lifecycle.configured_locations = data.clone();
                current.storage_locations = data.clone();
                current.policy.resources.disk_gib = checked.total_gib;
            }
            if let Some(settings) = &settings {
                settings.apply(current);
            }
            Ok(())
        })?;
        self.snapshot().await
    }

    pub(super) fn discover_storage(&self, config: &Configuration) -> crate::storage::Inventory {
        let mut inventory = crate::storage::inventory(
            config.vm_provider,
            &self.store.directory,
            &config.storage_locations,
            0,
        );
        if let Some(volumes) = &self.storage_volumes {
            inventory.volumes = volumes.clone();
        }
        inventory.locations =
            crate::storage::inspect_locations(&config.storage_locations, &inventory.volumes);
        inventory.layout = config.storage_layout.clone();
        if config.vm_provider == crate::VmProvider::Lima {
            inventory.system_disk = Some(crate::storage::SystemDisk {
                directory: config
                    .runtime_home(&self.store.directory)
                    .join("worker")
                    .to_string_lossy()
                    .into(),
                allocation_gib: config.system_gib(),
            });
            let directory = config
                .storage_layout
                .as_ref()
                .and_then(|layout| {
                    config
                        .storage_locations
                        .iter()
                        .find(|l| l.id == layout.system_location_id)
                })
                .map(|l| Path::new(&l.directory))
                .unwrap_or(&self.store.directory);
            if let Some(id) =
                crate::storage::volume_for(directory, &inventory.volumes).map(|v| v.id.clone())
            {
                if let Some(volume) = inventory.volumes.iter_mut().find(|v| v.id == id) {
                    volume.configured_gib =
                        volume.configured_gib.saturating_add(config.system_gib());
                }
            }
        }
        inventory
    }

    pub(super) fn reserve_boot_storage(
        &self,
        config: &Configuration,
        inventory: &mut crate::storage::Inventory,
    ) -> Result<()> {
        let home = config.runtime_home(&self.store.directory);
        let allocated = if self.vm(config)?.has_receipt()? {
            crate::storage::allocated_system_bytes_at(&home)?
        } else {
            0
        };
        let directory = if let Some(layout) = &config.storage_layout {
            &config
                .storage_locations
                .iter()
                .chain(
                    config
                        .storage_operation
                        .iter()
                        .flat_map(|op| op.target.iter()),
                )
                .chain(
                    config
                        .storage_lifecycle
                        .maintenance
                        .iter()
                        .flat_map(|op| op.target.iter()),
                )
                .find(|l| l.id == layout.system_location_id)
                .context("The VM system location is missing")?
                .directory
        } else {
            self.store
                .directory
                .to_str()
                .context("Invalid settings path")?
        };
        let remaining = crate::storage_layout::remaining_bytes(config.system_gib(), allocated)?;
        crate::storage::reserve_system_bytes(
            &mut inventory.volumes,
            Path::new(directory),
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
            current.format_version = 7;
            current.storage_boot_gib = 16;
            let layout = plan.layout.clone().context("Missing VM storage layout")?;
            current.storage_locations = layout.data_locations(&plan.locations)?;
            current.storage_layout = Some(layout);
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
        if let Some(layout) = &config.storage_layout {
            crate::runtime_storage::prepare_home(layout, &config.device_id)?;
        }
        let adapter = vm.storage_adapter(&self.store.directory)?;
        for location in &config.storage_locations {
            // Creation reopens only an exactly matching owned image. Its receipt
            // and volume checks protect retries after an interrupted first boot.
            adapter.create(location).await?;
        }
        let vm = self.prepared_storage_vm(config, vm).await?;
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

    pub(super) async fn prepared_storage_vm(&self, config: &Configuration, vm: &Vm) -> Result<Vm> {
        self.storage_runtime_preflight().await?;
        if let Some(layout) = &config.storage_layout {
            crate::runtime_storage::prepare_home(layout, &config.device_id)?;
        }
        let image = if self.runner.is_none() {
            if let Some(layout) = &config.storage_layout {
                self.set_status(
                    "preparing",
                    "Downloading and verifying the VM image on the selected drive",
                )
                .await;
                let runtime: Value =
                    serde_json::from_str(include_str!("../../../../runtime/lima.json"))?;
                let image = &runtime["images"][std::env::consts::ARCH];
                Some(
                    crate::runtime_image::download(
                        layout,
                        &config.device_id,
                        image["location"]
                            .as_str()
                            .context("Missing pinned VM image")?,
                        image["digest"]
                            .as_str()
                            .context("Missing pinned VM image digest")?,
                        &|| {
                            let current = self.store.load()?;
                            anyhow::ensure!(
                                (current.prepare_requested
                                    || current
                                        .storage_lifecycle
                                        .maintenance
                                        .as_ref()
                                        .is_some_and(|op| !op.paused && op.error.is_none()))
                                    && !current.stop_requested
                                    && current.storage_layout == config.storage_layout,
                                "VM preparation canceled by owner"
                            );
                            Ok(())
                        },
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        Ok(image.map_or_else(|| vm.clone(), |image| vm.clone().with_boot_image(image)))
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
        if let Some(layout) = &config.storage_layout {
            self.verify_runtime_placement(layout, &config.device_id)
                .await?;
            if self.runner.is_none() {
                crate::runtime_image::cleanup(layout, &config.device_id)?;
            }
        }
        self.store.update(|current| {
            anyhow::ensure!(
                current.storage_locations == config.storage_locations,
                "Storage choices changed during preparation"
            );
            current.format_version = 7;
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
        inventory.retained_runtime_directories = config
            .runtime_retained
            .iter()
            .map(|r| r.runtime_directory.clone())
            .collect();
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
        // Saved selections remain visible while the verified, applied pool is
        // still being changed. Do not advertise them as active worker capacity.
        let pending = config
            .storage_operation
            .as_ref()
            .and_then(|op| op.layout.as_ref().map(|layout| (layout, &op.target)))
            .or_else(|| {
                config
                    .storage_lifecycle
                    .maintenance
                    .as_ref()
                    .and_then(|op| op.layout.as_ref().map(|layout| (layout, &op.target)))
            });
        inventory.pending_update = pending.and_then(|(layout, target)| {
            let locations = layout.total_locations(target).ok()?;
            Some(crate::storage::PendingUpdate {
                total_gib: locations
                    .iter()
                    .map(|location| location.allocation_gib)
                    .sum(),
                locations,
                layout: layout.clone(),
            })
        });
        if let Some(relocation) = &config.runtime_relocation {
            let message = format!("Original VM retained at {} until the selected-drive VM is verified and cleanup finishes", relocation.source);
            if let Some(operation) = &mut inventory.operation {
                operation.message.push_str(&format!(". {message}"));
            } else {
                inventory.operation = Some(OperationStatus {
                    phase: "cleanup".into(),
                    message,
                });
            }
        }
        if let Ok(total) = config.total_locations() {
            for status in &mut inventory.locations {
                if let Some(location) = total.iter().find(|l| l.id == status.location.id) {
                    status.location.allocation_gib = location.allocation_gib;
                }
            }
        }
        inventory
    }

    pub(super) fn validate_storage(&self, config: &Configuration) -> Result<()> {
        if config.storage_locations.is_empty() {
            return Ok(());
        }
        crate::storage::require_location_support(config.vm_provider)?;
        if self.runner.is_none() && (config.vm_created || self.vm(config)?.has_receipt()?) {
            if let Some(layout) = &config.storage_layout {
                crate::runtime_storage::validate_home(layout, &config.device_id)?;
            }
        }
        let mut inventory = self.discover_storage(config);
        self.reserve_boot_storage(config, &mut inventory)?;
        let selections = config
            .storage_locations
            .iter()
            .map(|location| Selection {
                expected_volume_id: Some(location.volume_id.clone()),
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
        let checked = crate::storage::plan_disks(
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
                config.total_storage_gib().saturating_add(10)
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
            if operation
                .layout
                .as_ref()
                .is_some_and(|l| config.storage_layout.as_ref() != Some(l))
            {
                for location in &operation.previous {
                    adapter.validate(location)?;
                }
            } else {
                self.validate_storage(config)?;
            }
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
            if let Some(layout) = &operation.layout {
                if self.relocate_runtime(config, vm, layout).await? {
                    return Ok(());
                }
            }
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
        if let Some(layout) = &operation.layout {
            self.verify_runtime_placement(layout, &config.device_id)
                .await?;
        }
        self.cleanup_runtime_relocation(config, vm).await?;
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
            current.format_version = 7;
            current.policy.resources.disk_gib = current.total_storage_gib();
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
        let mut target_config = config.clone();
        target_config.storage_locations = operation.target.clone();
        if let Some(layout) = &operation.layout {
            target_config.storage_layout = Some(layout.clone());
        }
        let mut inventory = self.discover_storage(&target_config);
        self.reserve_boot_storage(&target_config, &mut inventory)?;
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
                expected_volume_id: Some(location.volume_id.clone()),
                id: Some(location.id.clone()),
                directory: location.directory.clone(),
                allocation_gib: location.allocation_gib,
            })
            .collect::<Vec<_>>();
        let checked = crate::storage::plan_disks(
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
