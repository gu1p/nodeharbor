use super::*;
use crate::storage::{ChangePlan, Location, Selection};
use crate::storage_layout::Layout;
use crate::storage_lifecycle::ReviewOptions;

#[derive(Clone)]
pub(super) struct Prepared {
    pub layout: Layout,
    pub total: Vec<Location>,
    pub data: Vec<Location>,
}

impl Agent {
    pub(super) async fn relocate_runtime(
        &self,
        config: &Configuration,
        vm: &Vm,
        target: &Layout,
    ) -> Result<bool> {
        if config.storage_layout.as_ref() == Some(target) {
            return Ok(false);
        }
        anyhow::ensure!(
            vm.has_receipt()? && vm.info().await?.stopped,
            "Stop the owned VM before relocating its storage"
        );
        let source = config.runtime_home(&self.store.directory);
        if source == target.home() {
            crate::runtime_storage::validate_home(target, &config.device_id)?;
            self.store.update(|c| {
                c.storage_layout = Some(target.clone());
                Ok(())
            })?;
            return Ok(true);
        }
        let source_volume_id = crate::storage::volume_identity(&source)?;
        let relocation = crate::runtime_storage::Relocation {
            source: source.to_string_lossy().into(),
            source_volume_id,
            target: target.clone(),
            switched: false,
        };
        self.store.update(|c| {
            if let Some(saved) = &c.runtime_relocation {
                anyhow::ensure!(
                    !saved.switched
                        && saved.source == relocation.source
                        && saved.source_volume_id == relocation.source_volume_id
                        && saved.target == *target,
                    "Finish the previous VM relocation before moving it again"
                );
            } else {
                c.runtime_relocation = Some(relocation.clone());
            }
            Ok(())
        })?;
        if self.runner.is_none() {
            crate::runtime_storage::prepare_home(target, &config.device_id)?;
            let destination = Vm::managed(
                &config.device_id,
                &self.store.directory,
                Vm::native_runner_at(crate::VmProvider::Lima, &target.home())?,
            )?;
            let info = destination.info().await?;
            anyhow::ensure!(
                !info.installed || info.stopped,
                "Stop the destination VM before resuming relocation"
            );
        }
        let store = self.store.clone();
        let owner = config.device_id.clone();
        let target = target.clone();
        let disks = config.storage_locations.clone();
        self.set_status(
            "preparing",
            "Moving the VM onto the selected drive; the original remains intact until verification",
        )
        .await;
        let expected = target.clone();
        tokio::task::spawn_blocking(move || {
            crate::runtime_storage::copy_runtime(&source, &target, &owner, &disks, &|| {
                let c = store.load()?;
                store.check_runtime_authority()?;
                anyhow::ensure!(
                    !c.stop_requested
                        && !c.storage_operation.as_ref().is_some_and(|op| op.paused)
                        && !c
                            .storage_lifecycle
                            .maintenance
                            .as_ref()
                            .is_some_and(|op| op.paused)
                        && c.runtime_relocation
                            .as_ref()
                            .is_some_and(|r| !r.switched && r.target == target),
                    "VM relocation paused by owner; the original storage is preserved"
                );
                Ok(())
            })
        })
        .await??;
        self.verify_runtime_placement(&expected, &config.device_id)
            .await?;
        self.store.check_runtime_authority()?;
        self.store.update(|c| {
            anyhow::ensure!(
                !c.stop_requested
                    && !c.storage_operation.as_ref().is_some_and(|op| op.paused)
                    && !c
                        .storage_lifecycle
                        .maintenance
                        .as_ref()
                        .is_some_and(|op| op.paused),
                "VM relocation paused by owner"
            );
            let movement = c
                .runtime_relocation
                .as_mut()
                .context("VM relocation was canceled")?;
            anyhow::ensure!(
                movement.target == expected && !movement.switched,
                "VM relocation changed"
            );
            movement.switched = true;
            c.storage_layout = Some(expected.clone());
            c.storage_boot_gib = expected.system_gib;
            c.format_version = 7;
            Ok(())
        })?;
        Ok(true)
    }

    pub(super) async fn verify_runtime_placement(
        &self,
        layout: &Layout,
        owner: &str,
    ) -> Result<()> {
        // Injected runners model host commands in deterministic integration tests.
        // All application/CLI constructors use the native filesystem proof.
        if self.runner.is_none() {
            crate::runtime_storage::verify_boot_image(layout, owner).await?;
        }
        Ok(())
    }

    pub(super) async fn cleanup_runtime_relocation(
        &self,
        config: &Configuration,
        vm: &Vm,
    ) -> Result<()> {
        let Some(saved) = &config.runtime_relocation else {
            return Ok(());
        };
        if self.runner.is_some() {
            return Ok(());
        }
        anyhow::ensure!(
            saved.switched
                && config.storage_layout.as_ref() == Some(&saved.target)
                && vm.has_receipt()?
                && vm.info().await?.stopped,
            "Finish verifying and stop the selected VM before removing its original copy"
        );
        self.verify_runtime_placement(&saved.target, &config.device_id)
            .await?;
        let source = Path::new(&saved.source);
        let anchor = if source.exists() {
            source
        } else {
            source.parent().context("Missing original VM volume")?
        };
        anyhow::ensure!(
            crate::storage::volume_identity(anchor)? == saved.source_volume_id,
            "Reconnect the original VM volume to finish relocation cleanup"
        );
        if source.exists() {
            crate::runtime_storage::verify_source_receipt(
                source,
                &saved.target,
                &config.device_id,
            )?;
            if source.join("worker").exists() {
                crate::runtime_storage::verify_source(source, &saved.target, &config.device_id)?;
                let runner = Vm::native_runner_at(crate::VmProvider::Lima, source)?;
                let old = Vm::managed(&config.device_id, &self.store.directory, runner.clone())?;
                let info = old.info().await?;
                anyhow::ensure!(
                    info.stopped,
                    "The original VM must remain stopped during cleanup"
                );
                self.store.check_runtime_authority()?;
                let output = runner
                    .run(&["delete".into(), "worker".into()], None, 60)
                    .await?;
                anyhow::ensure!(
                    output.success && !old.info().await?.installed,
                    "The original VM copy could not be removed: {}",
                    output.stderr
                );
            }
            // The source receipt scopes this private runtime tree. Directory
            // symlinks in _disks are removed without following their targets.
            crate::runtime_storage::remove_source_home(source, &saved.target, &config.device_id)?;
        }
        self.store.update(|c| {
            anyhow::ensure!(
                c.runtime_relocation.as_ref().is_some_and(|r| r.switched
                    && r.target == saved.target
                    && r.source == saved.source),
                "VM relocation changed during cleanup"
            );
            c.runtime_relocation = None;
            Ok(())
        })?;
        Ok(())
    }

    pub(super) async fn cleanup_retired_runtimes(&self, config: &Configuration) {
        if self.runner.is_some() || config.stop_requested {
            return;
        }
        for layout in &config.runtime_retained {
            let result: Result<()> = async {
                anyhow::ensure!(
                    config
                        .storage_layout
                        .as_ref()
                        .is_none_or(|active| active.home() != layout.home()),
                    "Cannot remove the active VM runtime"
                );
                let home = layout.home();
                if home.exists() {
                    crate::runtime_storage::validate_home(layout, &config.device_id)?;
                    let runner = Vm::native_runner_at(crate::VmProvider::Lima, &home)?;
                    let old =
                        Vm::managed(&config.device_id, &self.store.directory, runner.clone())?;
                    let info = old.info().await?;
                    if info.installed {
                        self.store.check_runtime_authority()?;
                        if !info.stopped {
                            let stopped = runner
                                .run(
                                    &["stop".into(), "--force".into(), "worker".into()],
                                    None,
                                    120,
                                )
                                .await?;
                            anyhow::ensure!(
                                stopped.success && old.info().await?.stopped,
                                "The retired VM did not stop; its storage was preserved"
                            );
                        }
                        self.store.check_runtime_authority()?;
                        let deleted = runner
                            .run(&["delete".into(), "worker".into()], None, 60)
                            .await?;
                        anyhow::ensure!(
                            deleted.success && !old.info().await?.installed,
                            "The retired VM could not be deleted; cleanup remains pending"
                        );
                    }
                    crate::runtime_storage::remove_retired_home(layout, &config.device_id)?;
                } else {
                    let parent = home
                        .parent()
                        .context("The retired VM volume is unavailable")?;
                    anyhow::ensure!(
                        crate::storage::volume_identity(parent)? == layout.volume_id,
                        "Reconnect the retired VM volume to finish cleanup"
                    );
                }
                self.store.update(|c| {
                    c.runtime_retained.retain(|saved| saved != layout);
                    Ok(())
                })?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                self.activity.record(
                    "warn",
                    "agent",
                    &format!(
                        "Retired VM cleanup pending at {}: {error}",
                        layout.runtime_directory
                    ),
                );
            }
        }
    }

    pub async fn preview_storage_request(
        &self,
        selections: Vec<Selection>,
        options: ReviewOptions,
    ) -> Result<ChangePlan> {
        let config = self.store.load()?;
        if config.vm_provider != crate::VmProvider::Lima || options.delete_all {
            return self
                .preview_storage_request_inner(selections, options, None)
                .await;
        }
        anyhow::ensure!(
            config.storage_operation.is_none() && config.storage_lifecycle.maintenance.is_none(),
            "Wait for current worker maintenance to finish"
        );
        let mut selections = selections;
        if let Some(id) = &options.restore_disk {
            let old = config
                .storage_lifecycle
                .configured_locations
                .iter()
                .find(|l| &l.id == id)
                .context("Unknown excluded disk")?;
            selections = config
                .total_locations()?
                .iter()
                .map(|l| Selection {
                    id: Some(l.id.clone()),
                    expected_volume_id: Some(l.volume_id.clone()),
                    directory: l.directory.clone(),
                    allocation_gib: l.allocation_gib,
                })
                .collect();
            selections.push(Selection {
                id: None,
                expected_volume_id: Some(old.volume_id.clone()),
                directory: old.directory.clone(),
                allocation_gib: old.allocation_gib,
            });
        }
        anyhow::ensure!(
            config.storage_locations.is_empty() || !selections.is_empty(),
            "Use Delete all worker storage to remove the last disk"
        );
        let inventory = self.discover_storage(&config);
        let previous = config.total_locations()?;
        let mut physical = crate::storage::allocated_bytes(
            &self.store.directory,
            &config.device_id,
            &config.storage_locations,
        );
        if let Some(layout) = &config.storage_layout {
            let bytes = crate::storage::allocated_system_bytes_at(&layout.home())?;
            let credit = physical
                .entry(layout.system_location_id.clone())
                .or_default();
            *credit = credit.saturating_add(bytes);
        }
        let total = crate::storage::plan_change(
            &selections,
            Path::new(
                inventory
                    .default_directory
                    .as_deref()
                    .context("The runtime does not report its storage directory")?,
            ),
            config.policy.resources.disk_gib,
            &inventory.volumes,
            &previous,
            &physical,
        )?;
        let layout = config.resolve_storage_layout(&total)?;
        layout.validate_path()?;
        let data = layout.data_locations(&total)?;
        let prepared = Prepared {
            layout,
            total,
            data,
        };
        let physical_selections = prepared
            .data
            .iter()
            .map(|l| Selection {
                id: config
                    .storage_locations
                    .iter()
                    .any(|old| old.id == l.id)
                    .then(|| l.id.clone()),
                expected_volume_id: Some(l.volume_id.clone()),
                directory: l.directory.clone(),
                allocation_gib: l.allocation_gib,
            })
            .collect();
        let mut options = options;
        options.restore_disk = None; // The checked selection above includes the returned member.
        let mut plan = self
            .preview_storage_request_inner(physical_selections, options, Some(prepared.clone()))
            .await?;
        plan.locations = prepared.total;
        plan.total_gib = plan.locations.iter().map(|l| l.allocation_gib).sum();
        if let Some(review) = &mut plan.maintenance {
            review.minimum_gib = review
                .minimum_gib
                .saturating_add(prepared.layout.system_gib);
        }
        plan.layout = Some(prepared.layout);
        Ok(plan)
    }

    pub(super) fn checked_layout(
        &self,
        config: &Configuration,
        plan: &ChangePlan,
    ) -> Result<Layout> {
        let layout = plan
            .layout
            .as_ref()
            .context("Review storage again to include the VM operating system in its allocation")?;
        let expected = config.resolve_storage_layout(&plan.locations)?;
        anyhow::ensure!(
            *layout == expected,
            "The VM storage layout changed; review again"
        );
        layout.validate_path()?;
        Ok(layout.clone())
    }
}
