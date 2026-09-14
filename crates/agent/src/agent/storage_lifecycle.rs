use super::*;
use crate::storage::{ChangePlan, Location, OperationStatus, Selection};
use crate::storage_lifecycle::*;

impl Agent {
    fn maintenance_update(
        &self,
        change: impl FnOnce(&mut Maintenance) -> Result<()>,
    ) -> Result<()> {
        self.store.update(|c| {
            change(
                c.storage_lifecycle
                    .maintenance
                    .as_mut()
                    .context("Storage maintenance was interrupted")?,
            )
        })?;
        Ok(())
    }

    fn maintenance_phase(&self, phase: Phase) -> Result<()> {
        self.maintenance_update(|op| {
            op.phase = phase;
            Ok(())
        })
    }

    pub(super) async fn maintenance_tick(&self, config: &Configuration, vm: &Vm) -> Result<()> {
        let op = config
            .storage_lifecycle
            .maintenance
            .as_ref()
            .context("No storage maintenance")?;
        if op.paused
            || op.error.is_some()
            || (op.review.kind == Kind::FailureRecovery
                && (!config.policy.enabled || !config.storage_lifecycle.recovery_enabled))
        {
            if vm.has_receipt()? && vm.info().await?.running {
                vm.stop_now().await?;
            }
            self.set_status(
                if op.error.is_some() {
                    "error"
                } else {
                    "paused"
                },
                op.error
                    .clone()
                    .unwrap_or_else(|| "Storage maintenance paused by owner".into()),
            )
            .await;
            // Local owner controls remain effective when the controller is down.
            let _ = self.heartbeat(config).await;
            return Ok(());
        }
        let result = self.maintenance_step(config, vm, op).await;
        if let Err(error) = &result {
            self.maintenance_update(|op| {op.error = Some(format!("Storage maintenance stopped: {error}. Resolve the problem, then Retry storage maintenance to resume this phase.")); Ok(())})?;
            if vm.has_receipt()? && vm.info().await?.running {
                vm.stop_now().await?;
            }
        }
        result
    }

    async fn maintenance_step(
        &self,
        config: &Configuration,
        vm: &Vm,
        op: &Maintenance,
    ) -> Result<()> {
        let whole_vm = config.vm_provider == crate::VmProvider::Multipass
            || op.review.kind != Kind::ResizeRemove;
        let backup_required = op.review.kind == Kind::ResizeRemove;
        let info = vm.info().await?;
        self.runtime.lock().await.worker = info.clone();
        self.set_status("preparing", format!("Storage maintenance: {:?}", op.phase))
            .await;
        self.heartbeat(config).await?;
        match op.phase {
            Phase::Drain => {
                anyhow::ensure!(
                    vm.has_receipt()? || !info.installed,
                    "Storage maintenance requires the existing worker ownership receipt"
                );
                let drain = self
                    .request(config, "/device/drain", Some(json!({})))
                    .await?;
                self.begin_drain()?;
                if info.running {
                    anyhow::ensure!(
                        info.reachable,
                        "Cannot inspect running work before storage maintenance"
                    );
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
                if backup_required {
                    if !info.running {
                        vm.start().await?;
                    }
                    vm.prepare_storage_guest().await?;
                } else if info.running {
                    vm.stop_now().await?;
                }
                self.clear_drain()?;
                self.maintenance_phase(if backup_required {
                    Phase::Backup
                } else {
                    Phase::Reset
                })?;
            }
            Phase::Backup => {
                if !info.running {
                    vm.start().await?;
                }
                vm.prepare_storage_guest().await?;
                let usage = vm.backup_inspect().await?;
                let needed = required_capacity(
                    usage["dataBytes"]
                        .as_u64()
                        .context("Cannot inspect stopped worker data")?,
                    config.vm_provider == crate::VmProvider::Multipass,
                )?;
                anyhow::ensure!(
                    op.total_gib >= needed,
                    "Data now requires {needed} GiB; source images are preserved"
                );
                let backup = if let Some(backup) = &op.backup {
                    backup.clone()
                } else {
                    let location = op
                        .review
                        .backup
                        .as_ref()
                        .context("No reviewed temporary backup location")?;
                    let path = crate::storage::canonical_directory(Path::new(&location.directory))?;
                    if !path.exists() {
                        std::fs::create_dir(&path)?;
                    }
                    let file = tempfile::Builder::new()
                        .prefix(".nodeharbor-backup-")
                        .tempfile_in(path)?;
                    secure_backup(file.path())?;
                    anyhow::ensure!(
                        crate::storage::volume_identity_file(file.as_file())? == location.volume_id,
                        "The backup volume changed; source images preserved"
                    );
                    let (_, path) = file.keep()?;
                    #[cfg(unix)]
                    std::fs::File::open(path.parent().context("Missing backup directory")?)?
                        .sync_all()?;
                    let backup = Backup {
                        path: path.to_string_lossy().into(),
                        volume_id: location.volume_id.clone(),
                        bytes: 0,
                        sha256: None,
                        verified: false,
                    };
                    self.maintenance_update(|op| {
                        op.backup = Some(backup.clone());
                        Ok(())
                    })?;
                    backup
                };
                open_backup(&backup)?;
                let mut options = std::fs::OpenOptions::new();
                options.write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW);
                }
                let output = options.open(&backup.path)?;
                anyhow::ensure!(
                    crate::storage::volume_identity_file(&output)? == backup.volume_id,
                    "The original backup volume changed"
                );
                output.set_len(0)?;
                vm.backup_stream("backup", None, Some(output), op.review.temporary_bytes)
                    .await?;
                let checksum_backup = backup.clone();
                let (hash, bytes) = vm
                    .with_owner_lease(async move {
                        tokio::task::spawn_blocking(move || {
                            checksum(open_backup(&checksum_backup)?)
                        })
                        .await?
                    })
                    .await?;
                vm.backup_stream("verify", Some(open_backup(&backup)?), None, 0)
                    .await?;
                self.maintenance_update(|op| {
                    let backup = op.backup.as_mut().context("Backup record disappeared")?;
                    backup.sha256 = Some(hash);
                    backup.bytes = bytes;
                    backup.verified = true;
                    op.phase = Phase::BackupVerified;
                    Ok(())
                })?;
            }
            Phase::BackupVerified => {
                verified_file(op.backup.clone().context("No verified backup")?, vm).await?;
                if !whole_vm {
                    if !info.running {
                        vm.start().await?;
                    }
                    vm.prepare_storage_guest().await?;
                    let pool = config
                        .storage_lifecycle
                        .pool_id
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| config.device_id.clone());
                    vm.storage_command("retire", Some(json!({"poolId":pool})))
                        .await?;
                }
                if vm.info().await?.running {
                    vm.stop().await?;
                }
                self.maintenance_phase(if whole_vm {
                    Phase::Reset
                } else {
                    Phase::Delete
                })?;
            }
            Phase::Reset => {
                anyhow::ensure!(!info.running, "Stop the worker before removing its access");
                if backup_required {
                    verified_file(op.backup.clone().context("No verified backup")?, vm).await?;
                }
                self.request(
                    config,
                    "/device/reset",
                    Some(json!({"requestId":op.request_id})),
                )
                .await?;
                self.maintenance_phase(Phase::Delete)?;
            }
            Phase::Delete => {
                anyhow::ensure!(
                    !info.running,
                    "Stop the worker before deleting source images"
                );
                if backup_required {
                    verified_file(op.backup.clone().context("No verified backup")?, vm).await?;
                    if config.vm_provider == crate::VmProvider::Multipass && info.installed {
                        // Free space now includes the completed backup. Recheck
                        // after drain, immediately before the original is deleted.
                        let space = vm.replacement_space().await?;
                        check_replacement_space(
                            &space,
                            op.total_gib,
                            None,
                            &self.discover_storage(config),
                        )?;
                    }
                }
                if whole_vm {
                    vm.remove().await?;
                } else {
                    vm.set_storage_attachments(&[]).await?;
                }
                if config.vm_provider == crate::VmProvider::Lima {
                    let adapter = vm.storage_adapter(&self.store.directory)?;
                    let inventory = self.discover_storage(config);
                    self.store.update(|c| {
                        for old in &op.previous {
                            if !c.storage_lifecycle.retired.contains(old) {
                                c.storage_lifecycle.retired.push(old.clone());
                            }
                        }
                        Ok(())
                    })?;
                    for old in &op.previous {
                        let present = crate::storage::inspect_locations(
                            std::slice::from_ref(old),
                            &inventory.volumes,
                        )[0]
                        .available;
                        if !present && op.review.kind != Kind::ResizeRemove {
                            continue;
                        }
                        adapter.remove_owned(old).await?;
                        self.store.update(|c| {
                            c.storage_lifecycle.retired.retain(|l| l != old);
                            c.storage_retained.retain(|l| l != old);
                            Ok(())
                        })?;
                    }
                }
                self.maintenance_phase(if op.review.kind == Kind::DeleteAll {
                    Phase::Cleanup
                } else {
                    Phase::Create
                })?;
            }
            Phase::Create => {
                if backup_required {
                    verified_file(op.backup.clone().context("No verified backup")?, vm).await?;
                }
                let mut resources = config.policy.resources.clone();
                resources.disk_gib = op.total_gib;
                if config.vm_provider == crate::VmProvider::Lima {
                    let adapter = vm.storage_adapter(&self.store.directory)?;
                    if !info.running {
                        for target in &op.target {
                            adapter.create(target).await?;
                        }
                        if info.installed {
                            vm.set_storage_configuration(
                                &op.target,
                                &[],
                                &op.pool_id.to_string(),
                                op.generation,
                                false,
                            )
                            .await?;
                            vm.start().await?;
                        } else {
                            vm.create_with_storage(
                                &resources,
                                &self.store.directory,
                                crate::guest_files(),
                                &op.target,
                                &op.pool_id.to_string(),
                                op.generation,
                            )
                            .await?;
                        }
                    }
                    vm.prepare_storage_guest().await?;
                    let request = crate::lima::storage_request(
                        &config.device_id,
                        &op.pool_id.to_string(),
                        &op.target,
                        &[],
                        op.generation,
                    );
                    vm.storage_command("apply", Some(request)).await?;
                } else if !info.installed {
                    vm.create(&resources, &self.store.directory, crate::guest_files())
                        .await?;
                } else if !info.running {
                    vm.start().await?;
                }
                self.maintenance_phase(if backup_required {
                    Phase::Restore
                } else {
                    Phase::Verify
                })?;
            }
            Phase::Restore => {
                let file =
                    verified_file(op.backup.clone().context("No verified backup")?, vm).await?;
                if !info.running {
                    vm.start().await?;
                }
                vm.backup_stream("restore", Some(file), None, 0).await?;
                self.maintenance_phase(Phase::Verify)?;
            }
            Phase::Verify => {
                if !info.running {
                    vm.start().await?;
                }
                if backup_required {
                    vm.backup_stream(
                        "verify",
                        Some(
                            verified_file(op.backup.clone().context("No verified backup")?, vm)
                                .await?,
                        ),
                        None,
                        0,
                    )
                    .await?;
                }
                if config.vm_provider == crate::VmProvider::Lima {
                    vm.storage_command("restored", Some(json!({"poolId":op.pool_id})))
                        .await?;
                    let state = vm.storage_command("check", None).await?;
                    anyhow::ensure!(
                        state["poolId"] == op.pool_id.to_string(),
                        "Replacement pool identity changed"
                    );
                    super::storage_operations::verify_pool_identity(
                        &config.device_id,
                        &op.pool_id.to_string(),
                        op.generation,
                        &op.target,
                        &state,
                    )?;
                }
                self.maintenance_phase(Phase::Bootstrap)?;
            }
            Phase::Bootstrap => {
                if !info.running {
                    vm.start().await?;
                }
                if config.vm_provider == crate::VmProvider::Multipass {
                    vm.quiesce_storage().await?;
                    vm.reset_restored_credentials().await?;
                }
                let bootstrap = self
                    .request(config, "/device/bootstrap", Some(json!({})))
                    .await?;
                vm.configure(bootstrap).await?;
                self.maintenance_phase(Phase::Capacity)?;
            }
            Phase::Capacity => {
                if !info.running {
                    vm.start().await?;
                }
                vm.renew_lease().await?;
                vm.replacement_capacity(op.total_gib).await?;
                vm.stop().await?;
                self.maintenance_phase(Phase::Cleanup)?;
            }
            Phase::Cleanup => {
                anyhow::ensure!(
                    !info.running,
                    "Stop the worker before finishing storage maintenance"
                );
                self.store.update(|c| {
                    let deleting = op.review.kind == Kind::DeleteAll;
                    if c.storage_lifecycle.configured_locations.is_empty() {
                        c.storage_lifecycle.configured_locations = op.previous.clone();
                    }
                    if deleting {
                        c.storage_lifecycle.configured_locations.clear();
                    } else if op.review.kind != Kind::FailureRecovery {
                        c.storage_lifecycle.configured_locations = configured_after_change(
                            &c.storage_lifecycle.configured_locations,
                            &c.storage_locations,
                            &op.target,
                        );
                    }
                    c.storage_locations = op.target.clone();
                    c.storage_generation = op.generation;
                    c.storage_lifecycle.pool_id = (!deleting).then_some(op.pool_id);
                    c.storage_lifecycle.disabled = deleting;
                    c.storage_lifecycle.missing = None;
                    c.vm_created = !deleting;
                    c.vm_configured = !deleting;
                    c.prepare_requested = false;
                    if deleting {
                        c.policy.enabled = false;
                        c.allocated_resources = None;
                    } else {
                        c.policy.resources.disk_gib = op.total_gib;
                        c.allocated_resources = Some(c.policy.resources.clone());
                    }
                    if let Some(backup) = &op.backup {
                        c.storage_lifecycle.cleanup.push(backup.clone());
                    }
                    c.storage_lifecycle.maintenance = None;
                    c.storage_revision += 1;
                    c.draining_since = None;
                    Ok(())
                })?;
                self.cleanup_backups().await?;
                self.set_status("paused", if op.review.kind == Kind::DeleteAll {"Worker storage deleted and disabled; host enrollment remains"} else {"Storage verified; fresh health qualification is required before new assignments"}).await;
            }
        }
        Ok(())
    }

    pub(super) async fn cleanup_backups(&self) -> Result<()> {
        let config = self.store.load()?;
        for backup in &config.storage_lifecycle.cleanup {
            let path = Path::new(&backup.path);
            let removed = !path.exists()
                && path
                    .parent()
                    .and_then(|parent| std::fs::File::open(parent).ok())
                    .and_then(|file| crate::storage::volume_identity_file(&file).ok())
                    .is_some_and(|id| id == backup.volume_id);
            if removed {
                self.store.update(|c| {
                    c.storage_lifecycle
                        .cleanup
                        .retain(|b| b.path != backup.path);
                    Ok(())
                })?;
                continue;
            }
            if let Ok(file) = open_backup(backup) {
                drop(file);
                if std::fs::remove_file(&backup.path).is_ok() {
                    self.store.update(|c| {
                        c.storage_lifecycle
                            .cleanup
                            .retain(|b| b.path != backup.path);
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn missing_storage_tick(
        &self,
        config: &Configuration,
        vm: &Vm,
    ) -> Result<bool> {
        if config.storage_generation == 0
            || config.storage_locations.is_empty()
            || config.storage_lifecycle.initializing
        {
            return Ok(false);
        }
        let now = now_seconds();
        if config
            .storage_lifecycle
            .missing
            .as_ref()
            .is_some_and(|m| now.saturating_sub(m.last_check) < 10)
        {
            return Ok(true);
        }
        let inventory = self.discover_storage(config);
        let adapter = vm.storage_adapter(&self.store.directory)?;
        let missing: Vec<_> = config
            .storage_locations
            .iter()
            .filter(|l| {
                !crate::storage::inspect_locations(std::slice::from_ref(l), &inventory.volumes)[0]
                    .available
                    || adapter.validate(l).is_err()
            })
            .cloned()
            .collect();
        if missing.is_empty() && config.storage_lifecycle.missing.is_none() {
            return Ok(false);
        }
        if vm.has_receipt()? && vm.info().await?.running {
            vm.stop_now().await?;
        }
        self.runtime.lock().await.worker.running = false;
        self.set_status(
            "error",
            "Original worker storage is unavailable; assignments are stopped",
        )
        .await;
        let current = self.store.update(|c| {
            let saved = c.storage_lifecycle.missing.get_or_insert(Missing {
                since: now,
                last_check: now,
                locations: missing.clone(),
                error: None,
            });
            saved.last_check = now;
            if !missing.is_empty() {
                saved.locations = missing.clone();
            }
            Ok(())
        })?;
        // The stopped worker must remain stopped until non-acceptance has
        // reached the controller, including when the original volume returns.
        self.heartbeat(&current).await?;
        if missing.is_empty() {
            if !config.policy.enabled
                || config
                    .storage_lifecycle
                    .missing
                    .as_ref()
                    .is_some_and(|m| m.error.is_some())
            {
                return Ok(true);
            }
            // No forced activation, partial LVM activation or repair. Failure
            // leaves the original pool stopped for owner inspection.
            let validate = async {
                self.validate_storage(config)?;
                vm.start().await?;
                vm.prepare_storage_guest().await?;
                let state = vm.storage_command("activate", None).await?;
                let pool = config
                    .storage_lifecycle
                    .pool_id
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| config.device_id.clone());
                super::storage_operations::verify_pool_identity(
                    &config.device_id,
                    &pool,
                    config.storage_generation,
                    &config.storage_locations,
                    &state,
                )?;
                let bootstrap = self
                    .request(config, "/device/bootstrap", Some(json!({})))
                    .await?;
                vm.configure(bootstrap).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = validate {
                if vm.info().await?.running {
                    vm.stop_now().await?;
                }
                self.store.update(|c| {c.storage_lifecycle.missing.as_mut().context("Missing recovery record")?.error = Some(format!("Returned pool could not be validated: {error}. Inspect the original storage before retrying.")); Ok(())})?;
                return Err(error);
            }
            self.store.update(|c| {
                c.storage_lifecycle.missing = None;
                Ok(())
            })?;
            return Ok(true);
        }
        let saved = current
            .storage_lifecycle
            .missing
            .as_ref()
            .context("Missing recovery timing")?;
        let remaining: Vec<_> = config
            .storage_locations
            .iter()
            .filter(|l| !missing.contains(l))
            .cloned()
            .collect();
        let total: u64 = remaining.iter().map(|l| l.allocation_gib).sum();
        match recovery_decision(config.storage_lifecycle.recovery_enabled, config.policy.enabled && !config.application_update_pending, saved.since, now, total, saved.error.is_some()) {
            RecoveryDecision::Rebuild => {
                let selections = remaining.iter().map(|l| Selection {id:Some(l.id.clone()), directory:l.directory.clone(), allocation_gib:l.allocation_gib}).collect::<Vec<_>>();
                let mut volumes = inventory; self.reserve_boot_storage(config, &mut volumes)?;
                crate::storage::plan_change(&selections, &self.store.directory.join("storage"), total, &volumes.volumes, &remaining, &crate::storage::allocated_bytes(&self.store.directory, &config.device_id, &remaining))?;
                let plan = ChangePlan {revision:config.storage_revision, locations:remaining, total_gib:total, requires_restart:true,
                    maintenance:Some(Review {kind:Kind::FailureRecovery, backup:None, minimum_gib:15, temporary_bytes:0,
                        deletions:vec!["All local data from the entire previous pool".into()], downtime:"Rebuilding on remaining selected disks; workloads follow their Kubernetes retry policies".into()})};
                self.store.update(|c| {
                    anyhow::ensure!(c.policy.enabled && c.storage_lifecycle.recovery_enabled, "Owner paused automatic recovery");
                    let mut op = new_maintenance(c, &plan); op.phase = Phase::Reset;
                    c.storage_lifecycle.maintenance = Some(op); c.storage_revision += 1; Ok(())
                })?;
            }
            RecoveryDecision::Insufficient => self.set_status("error", format!("Only {total} GiB remains on selected disks; at least 15 GiB plus the host reserve is required. Reconnect the original storage.")).await,
            RecoveryDecision::Paused => self.set_status("paused", "Storage recovery paused by owner").await,
            _ => {}
        }
        Ok(true)
    }
    pub async fn set_storage_recovery(&self, enabled: bool) -> Result<Snapshot> {
        self.store.update(|c| {
            c.storage_lifecycle.recovery_enabled = enabled;
            Ok(())
        })?;
        self.snapshot().await
    }

    pub async fn retry_storage_maintenance(&self) -> Result<Snapshot> {
        self.store.update(|c| {
            c.storage_revision = c
                .storage_revision
                .checked_add(1)
                .context("Storage revision overflow")?;
            if let Some(operation) = &mut c.storage_operation {
                operation.paused = false;
                return Ok(());
            }
            if let Some(missing) = &mut c.storage_lifecycle.missing {
                missing.error = None;
                missing.last_check = 0;
                if c.storage_lifecycle.maintenance.is_none() {
                    return Ok(());
                }
            }
            let operation = c
                .storage_lifecycle
                .maintenance
                .as_mut()
                .context("No storage operation needs a retry")?;
            operation.error = None;
            operation.paused = false;
            Ok(())
        })?;
        self.snapshot().await
    }

    pub async fn preview_storage(&self, selections: Vec<Selection>) -> Result<ChangePlan> {
        self.preview_storage_request(selections, ReviewOptions::default())
            .await
    }

    pub async fn preview_storage_request(
        &self,
        mut selections: Vec<Selection>,
        options: ReviewOptions,
    ) -> Result<ChangePlan> {
        let config = self.store.load()?;
        anyhow::ensure!(
            config.storage_lifecycle.maintenance.is_none()
                && config.storage_operation.is_none()
                && config.recreation.is_none()
                && !config.application_update_pending,
            "Wait for current worker maintenance to finish"
        );
        let vm = self.vm(&config)?;
        let requires_restart = config.vm_created || vm.has_receipt()?;
        if options.delete_all {
            let requires_restart = requires_restart
                || !config.storage_lifecycle.retired.is_empty()
                || !config.storage_retained.is_empty()
                || config.storage_locations.iter().any(|location| {
                    crate::lima_storage::disk_paths(
                        &self.store.directory.join("lima"),
                        &config.device_id,
                        location,
                    )
                    .is_ok_and(|paths| std::fs::symlink_metadata(paths.directory).is_ok())
                });
            anyhow::ensure!(
                selections.is_empty() && options.single_disk_gib.is_none(),
                "Deletion cannot include replacement storage"
            );
            return Ok(ChangePlan {
                revision: config.storage_revision,
                locations: vec![],
                total_gib: 0,
                requires_restart,
                maintenance: Some(Review {
                    kind: Kind::DeleteAll,
                    backup: None,
                    minimum_gib: 0,
                    temporary_bytes: 0,
                    deletions: vec![
                        "All worker data and the owned worker VM; host enrollment remains".into(),
                    ],
                    downtime:
                        "Worker storage remains disabled until you explicitly configure it again"
                            .into(),
                }),
            });
        }
        anyhow::ensure!(
            config.storage_locations.is_empty()
                || !selections.is_empty()
                || options.restore_disk.is_some(),
            "Use Delete all worker storage to remove the last disk"
        );
        if let Some(id) = &options.restore_disk {
            let old = config
                .storage_lifecycle
                .configured_locations
                .iter()
                .find(|l| &l.id == id)
                .context("Unknown excluded disk")?;
            anyhow::ensure!(
                crate::storage::inspect_locations(
                    std::slice::from_ref(old),
                    &self.discover_storage(&config).volumes
                )[0]
                .available,
                "Reconnect the original excluded volume before restoring its capacity"
            );
            anyhow::ensure!(
                !config
                    .storage_locations
                    .iter()
                    .any(|l| l.directory == old.directory),
                "This disk already contributes capacity"
            );
            selections = config
                .storage_locations
                .iter()
                .map(|l| Selection {
                    id: Some(l.id.clone()),
                    directory: l.directory.clone(),
                    allocation_gib: l.allocation_gib,
                })
                .collect();
            selections.push(Selection {
                id: None,
                directory: old.directory.clone(),
                allocation_gib: old.allocation_gib,
            });
        }
        let multipass = config.vm_provider == crate::VmProvider::Multipass;
        let reduction = config.storage_locations.iter().any(|old| {
            !selections
                .iter()
                .any(|s| s.id.as_ref() == Some(&old.id) && s.allocation_gib >= old.allocation_gib)
        });
        let returned = selections.iter().any(|selection| {
            config
                .storage_lifecycle
                .configured_locations
                .iter()
                .any(|old| old.directory == selection.directory)
                && !config
                    .storage_locations
                    .iter()
                    .any(|old| old.directory == selection.directory)
        });
        if !options.delete_all
            && !reduction
            && options.single_disk_gib.is_none()
            && options.restore_disk.is_none()
            && !returned
        {
            return self.preview_storage_growth(selections).await;
        }
        let mut inventory = self.discover_storage(&config);
        let (locations, total_gib) = if multipass {
            anyhow::ensure!(
                selections.is_empty(),
                "Multipass does not support selectable locations or additional disks"
            );
            let size = options
                .single_disk_gib
                .context("Choose a single worker disk allocation")?;
            anyhow::ensure!(
                (15..=1_048_576).contains(&size),
                "Choose at least 15 GiB of worker storage"
            );
            (vec![], size)
        } else {
            crate::storage::require_location_support(config.vm_provider)?;
            self.reserve_boot_storage(&config, &mut inventory)?;
            let physical = crate::storage::allocated_bytes(
                &self.store.directory,
                &config.device_id,
                &config.storage_locations,
            );
            let locations = crate::storage::plan_change(
                &selections,
                &self.store.directory.join("storage"),
                config.policy.resources.disk_gib,
                &inventory.volumes,
                &config.storage_locations,
                &physical,
            )?;
            let size = locations.iter().map(|l| l.allocation_gib).sum();
            (locations, size)
        };
        let (minimum_gib, temporary_bytes, backup) = if requires_restart {
            anyhow::ensure!(vm.has_receipt()? && vm.info().await?.reachable, "Start the existing worker so NodeHarbor can inspect its data before reviewing a smaller allocation");
            let usage = vm.backup_inspect().await?;
            let minimum = required_capacity(
                usage["dataBytes"]
                    .as_u64()
                    .context("Worker data usage is unavailable")?,
                multipass,
            )?;
            anyhow::ensure!(total_gib >= minimum, "Worker data requires at least {minimum} GiB including filesystem overhead and operating space; nothing was changed");
            let temporary = usage["backupBytes"]
                .as_u64()
                .context("Backup size is unavailable")?
                .checked_add(GIB)
                .context("Backup size overflow")?;
            let replacement = if multipass {
                Some(vm.replacement_space().await?)
            } else {
                None
            };
            if let Some(space) = &replacement {
                check_replacement_space(space, total_gib, None, &inventory)?;
            }
            let backup = self.select_backup(
                &config,
                &locations,
                &inventory,
                options.temporary_directory.as_deref(),
                temporary,
                replacement.as_ref().map(|space| (space, total_gib)),
            )?;
            (minimum, temporary, Some(backup))
        } else {
            (15, 0, None)
        };
        Ok(ChangePlan {revision: config.storage_revision, locations, total_gib, requires_restart,
            maintenance: Some(Review {kind: Kind::ResizeRemove, backup, minimum_gib, temporary_bytes,
                deletions: vec!["Previous managed storage images are replaced after backup verification; worker data is preserved".into()],
                downtime: "Drain, verified backup, disk replacement, restore and Kubernetes capacity check; duration depends on data size and disk speed".into()})})
    }

    fn select_backup(
        &self,
        config: &Configuration,
        target: &[Location],
        inventory: &crate::storage::Inventory,
        requested: Option<&str>,
        bytes: u64,
        replacement: Option<(&ReplacementSpace, u64)>,
    ) -> Result<BackupLocation> {
        let candidates: Vec<String> = if let Some(path) = requested {
            vec![path.into()]
        } else if config.storage_locations.is_empty() {
            vec![self.store.directory.to_string_lossy().into()]
        } else {
            config
                .storage_locations
                .iter()
                .map(|l| l.directory.clone())
                .collect()
        };
        let physical = crate::storage::allocated_bytes(
            &self.store.directory,
            &config.device_id,
            &config.storage_locations,
        );
        for directory in candidates {
            let path = crate::storage::canonical_directory(Path::new(&directory))?;
            let lima_home = self.store.directory.join("lima");
            let mut forbidden = vec![lima_home.clone()];
            for old in config
                .storage_locations
                .iter()
                .chain(&config.storage_lifecycle.retired)
                .chain(&config.storage_retained)
            {
                forbidden.push(
                    crate::lima_storage::disk_paths(&lima_home, &config.device_id, old)?.directory,
                );
            }
            if let Some((space, _)) = replacement {
                forbidden.push(space.directory.clone());
            }
            anyhow::ensure!(!forbidden.iter().any(|dir| path.starts_with(dir)), "Choose a backup folder outside the managed VM and image directories that will be replaced");
            let Some(volume) = crate::storage::volume_for(&path, &inventory.volumes) else {
                continue;
            };
            let growth: u64 = target
                .iter()
                .filter(|l| {
                    inventory
                        .volumes
                        .iter()
                        .any(|v| v.id == l.volume_id && v.capacity_pool == volume.capacity_pool)
                })
                .map(|l| {
                    l.allocation_gib
                        .saturating_sub(physical.get(&l.id).copied().unwrap_or(0) / GIB)
                })
                .sum();
            if volume.available_gib
                >= bytes
                    .div_ceil(GIB)
                    .saturating_add(10)
                    .saturating_add(growth)
            {
                if let Some((space, total)) = replacement {
                    if check_replacement_space(
                        space,
                        total,
                        Some((&volume.capacity_pool, bytes)),
                        inventory,
                    )
                    .is_err()
                    {
                        continue;
                    }
                }
                return Ok(BackupLocation {
                    directory: path.to_string_lossy().into(),
                    volume_id: volume.id.clone(),
                });
            }
        }
        anyhow::bail!("Choose a temporary backup folder with at least {} GiB free plus the 10 GiB host reserve and remaining image growth; nothing was changed", bytes.div_ceil(GIB))
    }

    pub(super) async fn apply_storage_maintenance(&self, plan: ChangePlan) -> Result<Snapshot> {
        let config = self.store.load()?;
        anyhow::ensure!(
            config.storage_revision == plan.revision,
            "Storage choices changed; review again"
        );
        let review = plan
            .maintenance
            .as_ref()
            .context("Missing storage review")?;
        anyhow::ensure!(
            review.kind != Kind::FailureRecovery,
            "Failure recovery is controlled by the persisted owner policy"
        );
        let selections = plan
            .locations
            .iter()
            .map(|l| Selection {
                id: config
                    .storage_locations
                    .iter()
                    .any(|old| old.id == l.id)
                    .then(|| l.id.clone()),
                directory: l.directory.clone(),
                allocation_gib: l.allocation_gib,
            })
            .collect();
        let checked = self
            .preview_storage_request(
                selections,
                ReviewOptions {
                    delete_all: review.kind == Kind::DeleteAll,
                    temporary_directory: review.backup.as_ref().map(|b| b.directory.clone()),
                    single_disk_gib: (config.vm_provider == crate::VmProvider::Multipass
                        && review.kind != Kind::DeleteAll)
                        .then_some(plan.total_gib),
                    restore_disk: None,
                },
            )
            .await?;
        let current = checked
            .maintenance
            .as_ref()
            .context("Storage impact changed; review again")?;
        anyhow::ensure!(
            checked.total_gib == plan.total_gib
                && checked.requires_restart == plan.requires_restart
                && current.kind == review.kind
                && current.backup == review.backup
                && current.minimum_gib <= review.minimum_gib
                && current.temporary_bytes <= review.temporary_bytes
                && checked
                    .locations
                    .iter()
                    .zip(&plan.locations)
                    .all(|(a, b)| a.directory == b.directory
                        && a.volume_id == b.volume_id
                        && a.allocation_gib == b.allocation_gib)
                && checked.locations.len() == plan.locations.len(),
            "Storage requirements changed; review again"
        );
        self.store.update(|c| {
            anyhow::ensure!(
                c.storage_revision == plan.revision && c.storage_lifecycle.maintenance.is_none(),
                "Storage choices changed; review again"
            );
            c.storage_revision += 1;
            c.format_version = 4;
            if !plan.requires_restart {
                c.storage_lifecycle.disabled = review.kind == Kind::DeleteAll;
                if !c.storage_lifecycle.disabled {
                    c.storage_lifecycle.initializing = true;
                    c.storage_lifecycle.pool_id = Some(uuid::Uuid::new_v4());
                }
                c.storage_locations = plan.locations.clone();
                c.storage_lifecycle.configured_locations = plan.locations.clone();
                if c.storage_lifecycle.disabled {
                    c.policy.enabled = false;
                    c.prepare_requested = false;
                } else {
                    c.policy.resources.disk_gib = plan.total_gib;
                }
            } else {
                c.storage_lifecycle.maintenance = Some(new_maintenance(c, &plan));
                c.draining_since.get_or_insert_with(now_seconds);
                if review.kind == Kind::DeleteAll {
                    c.policy.enabled = false;
                    c.prepare_requested = false;
                }
            }
            Ok(())
        })?;
        self.snapshot().await
    }

    pub(super) fn lifecycle_snapshot(
        &self,
        c: &Configuration,
        inventory: &mut crate::storage::Inventory,
    ) {
        let lifecycle = &c.storage_lifecycle;
        inventory.disabled = lifecycle.disabled;
        inventory.recovery_enabled = lifecycle.recovery_enabled;
        inventory.active_gib = if lifecycle.disabled
            || c.remote.repair_required
            || c.storage_operation.is_some()
            || lifecycle.missing.is_some()
            || lifecycle.maintenance.is_some()
            || !c.vm_created
        {
            0
        } else if c.storage_locations.is_empty() {
            c.policy.resources.disk_gib
        } else {
            c.storage_locations.iter().map(|l| l.allocation_gib).sum()
        };
        inventory.configured_gib = if lifecycle.configured_locations.is_empty() {
            if lifecycle.disabled {
                0
            } else {
                c.policy.resources.disk_gib
            }
        } else {
            lifecycle
                .configured_locations
                .iter()
                .map(|l| l.allocation_gib)
                .sum()
        };
        let excluded: Vec<_> = lifecycle
            .configured_locations
            .iter()
            .filter(|old| {
                !c.storage_locations
                    .iter()
                    .any(|l| l.directory == old.directory && l.volume_id == old.volume_id)
            })
            .cloned()
            .collect();
        inventory.excluded = crate::storage::inspect_locations(&excluded, &inventory.volumes);
        inventory.backup_cleanup = lifecycle.cleanup.clone();
        inventory
            .retained_copies
            .extend(crate::storage::inspect_locations(
                &lifecycle.retired,
                &inventory.volumes,
            ));
        if let Some(op) = &lifecycle.maintenance {
            inventory.recovery_backup = op.backup.clone().map(|mut backup| {
                if let Ok(file) = open_backup(&backup) {
                    if let Ok(metadata) = file.metadata() {
                        backup.bytes = metadata.len();
                    }
                }
                backup
            });
            inventory.operation = Some(OperationStatus {
                phase: format!("{:?}", op.phase),
                message: op.error.clone().unwrap_or_else(|| {
                    if op.paused {
                        "Storage maintenance paused by owner".into()
                    } else {
                        format!(
                            "Storage maintenance: {:?}. {}",
                            op.phase, op.review.downtime
                        )
                    }
                }),
            });
        } else if let Some(missing) = &lifecycle.missing {
            inventory.operation = Some(OperationStatus {phase:"missing".into(), message: missing.error.clone().unwrap_or_else(|| format!("Waiting for original storage: {}. {}", missing.locations.iter().map(|l| l.directory.as_str()).collect::<Vec<_>>().join(", "), if lifecycle.recovery_enabled {"After two minutes NodeHarbor may discard the entire old pool and rebuild on remaining selected disks."} else {"Automatic recovery is off; reconnect the original disks."}))});
        }
    }
}

fn new_maintenance(config: &Configuration, plan: &ChangePlan) -> Maintenance {
    let mut target = plan.locations.clone();
    for location in &mut target {
        location.id = format!("nh{}", &uuid::Uuid::new_v4().simple().to_string()[..9]);
    }
    let mut previous = config.storage_locations.clone();
    for old in config
        .storage_lifecycle
        .retired
        .iter()
        .chain(&config.storage_retained)
    {
        if (plan
            .maintenance
            .as_ref()
            .is_some_and(|review| review.kind == Kind::DeleteAll)
            || plan.locations.iter().any(|target| {
                target.directory == old.directory && target.volume_id == old.volume_id
            }))
            && !previous.contains(old)
        {
            previous.push(old.clone());
        }
    }
    Maintenance {
        request_id: uuid::Uuid::new_v4(),
        pool_id: uuid::Uuid::new_v4(),
        generation: config.storage_generation + 1,
        review: plan
            .maintenance
            .clone()
            .expect("reviewed storage operation"),
        previous,
        target,
        total_gib: plan.total_gib,
        phase: Phase::Drain,
        backup: None,
        paused: false,
        error: None,
    }
}

async fn verified_file(backup: Backup, vm: &Vm) -> Result<std::fs::File> {
    let verification =
        async move { tokio::task::spawn_blocking(move || verified_backup(&backup)).await? };
    if vm.info().await?.running {
        vm.with_owner_lease(verification).await
    } else {
        verification.await
    }
}

fn check_replacement_space(
    space: &ReplacementSpace,
    total_gib: u64,
    backup: Option<(&str, u64)>,
    inventory: &crate::storage::Inventory,
) -> Result<()> {
    let volume = crate::storage::volume_for(&space.directory, &inventory.volumes).context(
        "Cannot verify free space on the Multipass daemon's disk volume; nothing was changed",
    )?;
    let backup_bytes = backup
        .filter(|(pool, _)| *pool == volume.capacity_pool)
        .map_or(0, |(_, bytes)| bytes);
    let required = replacement_space_required(total_gib, space.reclaimed_bytes, backup_bytes);
    anyhow::ensure!(!volume.id.is_empty() && volume.available_gib >= required,"The Multipass disk volume needs {required} GiB free for replacement and the host reserve; available space is {} GiB. Source storage is preserved",volume.available_gib);
    Ok(())
}
