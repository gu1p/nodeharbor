use super::*;
use nodeharbor_core::configuration::{
    validate_edit, ConfigurationCapabilities, ConfigurationCommand, ConfigurationReceipt,
    ConfigurationReport,
};

impl Agent {
    pub fn configuration_report(&self) -> Result<ConfigurationReport> {
        let config = self.store.load()?;
        let observation = crate::observe::observation(
            &self.store.directory,
            config
                .allocated_resources
                .as_ref()
                .map_or(0, |r| r.disk_gib),
        );
        Ok(self.configuration_report_for(&config, observation.resources))
    }
    pub(super) fn configuration_report_for(
        &self,
        config: &Configuration,
        mut hardware: Resources,
    ) -> ConfigurationReport {
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let home = self.store.directory.join("lima");
        let disk_path = home
            .join("worker/diffdisk")
            .canonicalize()
            .or_else(|_| home.canonicalize())
            .or_else(|_| self.store.directory.canonicalize())
            .ok();
        let worker_disk = disk_path.as_ref().and_then(|path| {
            disks
                .iter()
                .filter(|disk| path.starts_with(disk.mount_point()))
                .max_by_key(|disk| disk.mount_point().as_os_str().len())
        });
        let disk_growth = config.vm_provider == crate::VmProvider::Lima && worker_disk.is_some();
        if config.vm_provider == crate::VmProvider::Lima {
            hardware.disk_gib = worker_disk
                .map_or(0, |d| d.available_space() / 1073741824)
                .saturating_add(
                    config
                        .allocated_resources
                        .as_ref()
                        .map_or(0, |r| r.disk_gib),
                );
        }
        let disk_growth_reason = if disk_growth {
            "Existing-disk growth uses capacity on the application-managed Lima disk's filesystem. Shrinking requires local replacement."
        } else {
            "Disk growth requires local approval because this runtime does not report a verifiable host storage location. CPU and memory limits can still be edited remotely."
        };
        let storage_reason="Additional disks and selectable storage locations require the multiple-disk runtime feature. Additional disk placement is unavailable with the current runtime APIs.";
        let inventory = if config.remote.consent {
            disks.iter().map(|disk|json!({
                "label":disk.name().to_string_lossy(),"mountPoint":disk.mount_point().to_string_lossy(),
                "filesystem":disk.file_system().to_string_lossy(),"availableGib":disk.available_space()/1073741824,
                "totalGib":disk.total_space()/1073741824,"configuredGib":if config.vm_provider == crate::VmProvider::Lima && worker_disk.is_some_and(|owned| owned.mount_point() == disk.mount_point()) { config.allocated_resources.as_ref().map_or(0, |r| r.disk_gib) } else { 0 },"eligible":false,"reason":storage_reason
            })).collect()
        } else {
            Vec::new()
        };
        ConfigurationReport {
            worker_disk_location: if config.remote.consent
                && config.vm_provider == crate::VmProvider::Lima
            {
                Some(home.join("worker/diffdisk").to_string_lossy().into_owned())
            } else {
                None
            },
            consent: config.remote.consent,
            revision: config.remote.revision,
            policy: Some(config.policy.clone()),
            hardware: Some(hardware),
            allocated_resources: if config.remote.repair_required {
                None
            } else {
                config.allocated_resources.clone()
            },
            capabilities: ConfigurationCapabilities {
                disk_growth,
                disk_growth_reason: disk_growth_reason.into(),
                storage: false,
                storage_reason: storage_reason.into(),
                local_approval: vec!["startAtLogin".into()],
            },
            storage_inventory: inventory,
            receipts: config.remote.receipts.clone(),
        }
    }
    /// This API is exposed only by local IPC/CLI. It never takes a remote edit.
    pub async fn set_remote_consent(&self, enabled: bool) -> Result<Snapshot> {
        self.store.update(|config| {
            config.remote.consent = enabled;
            Ok(())
        })?;
        self.snapshot().await
    }
    pub fn receive_configuration(&self, command: ConfigurationCommand) -> Result<()> {
        let hardware = self
            .configuration_report()?
            .hardware
            .context("Capacity unavailable")?;
        self.store.update(|config| {
            if let Some(pending)=&config.remote.pending {
                anyhow::ensure!(pending.edit==command.edit,"Another configuration request or different values for this request ID are pending");
                return Ok(());
            }
            if let Some(receipt)=config.remote.receipts.iter().find(|r|r.request_id==command.edit.request_id) {
                // Terminal requests are never re-executed, even after local edits.
                anyhow::bail!("{}",receipt.error.as_deref().unwrap_or("This request is already complete; reload its acknowledgment"));
            }
            validate_edit(&command.edit,&config.policy,&hardware,config.remote.consent,config.remote.revision).map_err(anyhow::Error::msg)?;
            Self::validate_remote_runtime(config,&command)?;
            config.remote.record(ConfigurationReceipt {request_id:command.edit.request_id.clone(),status:"pending".into(),revision:config.remote.revision,
                at:chrono::Utc::now().to_rfc3339(),effective_policy:Some(config.policy.clone()),effective_resources:config.allocated_resources.clone(),error:None});
            config.remote.pending=Some(command);
            Ok(())
        })?;
        Ok(())
    }
    fn validate_remote_runtime(
        config: &Configuration,
        command: &ConfigurationCommand,
    ) -> Result<()> {
        anyhow::ensure!(
            !config.remote.repair_required,
            "Inspect the stopped worker locally or replace it before changing configuration"
        );
        anyhow::ensure!(
            config.recreation.is_none()
                && !config.prepare_requested
                && !config.application_update_pending
                && !config.stop_requested,
            "Wait for the current local worker operation to finish before changing configuration"
        );
        anyhow::ensure!(command.edit.policy.resources.disk_gib == config.policy.resources.disk_gib
            || config.vm_provider == crate::VmProvider::Lima,
            "Disk growth requires local approval because this runtime does not report its physical storage location");
        if let Some(allocated) = &config.allocated_resources {
            anyhow::ensure!(
                command.edit.policy.resources.disk_gib >= allocated.disk_gib,
                "Shrinking the worker disk requires local approval to replace the worker"
            );
        }
        anyhow::ensure!(
            !command.edit.policy.enabled || config.vm_configured,
            "Prepare the worker locally before enabling sharing"
        );
        Ok(())
    }
    pub(super) fn reject_configuration(
        &self,
        command: &ConfigurationCommand,
        error: &str,
    ) -> Result<()> {
        self.store.update(|config| {
            if config
                .remote
                .receipts
                .iter()
                .any(|r| r.request_id == command.edit.request_id)
            {
                return Ok(());
            }
            config.remote.record(ConfigurationReceipt {
                request_id: command.edit.request_id.clone(),
                status: "rejected".into(),
                revision: config.remote.revision,
                at: chrono::Utc::now().to_rfc3339(),
                effective_policy: Some(config.policy.clone()),
                effective_resources: config.allocated_resources.clone(),
                error: Some(error.into()),
            });
            Ok(())
        })?;
        Ok(())
    }
    async fn wait_configuration_changed(&self, command: &ConfigurationCommand) -> Result<()> {
        loop {
            let current = self.store.load()?;
            if !current.remote.consent
                || current.remote.revision != command.edit.expected_revision
                || current
                    .remote
                    .pending
                    .as_ref()
                    .is_none_or(|p| p.edit != command.edit)
            {
                anyhow::bail!(
                    "The owner changed settings or revoked remote configuration during application"
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    pub(super) async fn apply_configuration(&self, vm: &Vm) -> Result<()> {
        let Some(command) = self.store.load()?.remote.pending else {
            return Ok(());
        };
        let hardware = self
            .configuration_report()?
            .hardware
            .context("Capacity unavailable")?;
        let claim = self.store.update(|config| {
            validate_edit(
                &command.edit,
                &config.policy,
                &hardware,
                config.remote.consent,
                config.remote.revision,
            )
            .map_err(anyhow::Error::msg)?;
            Self::validate_remote_runtime(config, &command)?;
            // Persist uncertainty before touching the VM, including crash recovery.
            config.remote.applying = true;
            if config.vm_created
                && config.allocated_resources.as_ref() != Some(&command.edit.policy.resources)
            {
                config.remote.repair_required = true;
            }
            Ok(())
        });
        let result = async {
            let claimed = claim?;
            if claimed.remote.repair_required {
                tokio::select! {
                    biased;
                    result=self.wait_configuration_changed(&command)=>result?,
                    result=vm.resize(&command.edit.policy.resources)=>result?,
                }
            }
            self.store.update(|current| {
                anyhow::ensure!(
                    current.remote.consent
                        && current.remote.revision == command.edit.expected_revision
                        && current
                            .remote
                            .pending
                            .as_ref()
                            .is_some_and(|p| p.edit == command.edit),
                    "The owner changed settings while configuration was applying"
                );
                let revision =
                    current.remote.revision + u64::from(current.policy != command.edit.policy);
                current.remote.pending = None;
                current.remote.applying = false;
                current.remote.repair_required = false;
                if current.vm_created {
                    current.allocated_resources = Some(command.edit.policy.resources.clone());
                }
                current.policy = command.edit.policy.clone();
                current.remote.record(ConfigurationReceipt {
                    request_id: command.edit.request_id.clone(),
                    status: "applied".into(),
                    revision,
                    at: chrono::Utc::now().to_rfc3339(),
                    effective_policy: Some(current.policy.clone()),
                    effective_resources: current.allocated_resources.clone(),
                    error: None,
                });
                Ok(())
            })?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            self.store.update(|current| {
                let error=if current.remote.repair_required {
                    current.allocated_resources=None;
                    format!("{error}. Allocation is unverified; inspect the stopped worker locally or replace it. Sharing is blocked.")
                }else{error.to_string()};
                current.remote.applying=false;
                current.remote.pending=None;
                current.remote.record(ConfigurationReceipt {request_id:command.edit.request_id,status:"rejected".into(),revision:current.remote.revision,
                    at:chrono::Utc::now().to_rfc3339(),effective_policy:Some(current.policy.clone()),effective_resources:current.allocated_resources.clone(),error:Some(error)});
                Ok(())
            })?;
        }
        Ok(())
    }
    pub(super) fn recover_configuration(&self) -> Result<()> {
        let current = self.store.load()?;
        if current.remote.applying {
            self.store.update(|config| {
                config.remote.applying=false;
                config.remote.repair_required=true;
                config.allocated_resources=None;
                if let Some(command)=config.remote.pending.take() {
                    config.remote.record(ConfigurationReceipt {request_id:command.edit.request_id,status:"rejected".into(),revision:config.remote.revision,
                        at:chrono::Utc::now().to_rfc3339(),effective_policy:Some(config.policy.clone()),effective_resources:None,
                        error:Some("Configuration was interrupted. Inspect the stopped worker locally or replace it; sharing is blocked until recovery".into())});
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}
