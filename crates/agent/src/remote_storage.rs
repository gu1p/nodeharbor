//! Authenticated configuration transport over the existing local storage lifecycle.
use super::*;
use nodeharbor_core::configuration::{
    ConfigurationCommand, ConfigurationOperation, ConfigurationReceipt,
};

impl Agent {
    pub(super) async fn process_remote_storage(&self) -> Result<()> {
        let command = self
            .store
            .load()?
            .remote
            .pending
            .context("No pending configuration")?;
        let mut authorized = self.clone();
        authorized.store = self.store.for_configuration(&command.edit);
        let result = tokio::select! {
            biased;
            result = self.wait_configuration_changed(&command) => result.map(|()| None),
            result = authorized.remote_storage_step(&command) => result,
        };
        match result {
            Ok(Some(value)) => {
                if let Err(error) = authorized.finish_remote_storage(&command, value, None) {
                    self.fail_remote_storage(&command, &error.to_string())
                        .await?;
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.fail_remote_storage(&command, &error.to_string())
                    .await?
            }
        }
        // Completion and rejection are durable even if this heartbeat is offline.
        let _ = self.heartbeat(&self.store.load()?).await;
        Ok(())
    }

    async fn remote_storage_step(
        &self,
        command: &ConfigurationCommand,
    ) -> Result<Option<Option<Value>>> {
        let config = self.store.load()?;
        self.store.check_configuration_authority(&config)?;
        use ConfigurationOperation::*;
        match command
            .edit
            .operation
            .as_ref()
            .context("Missing storage operation")?
        {
            StoragePreview {
                selections,
                options,
            } => {
                let plan = self
                    .preview_storage_request(
                        serde_json::from_value(json!(selections))?,
                        serde_json::from_value(options.clone())?,
                    )
                    .await?;
                return Ok(Some(Some(json!(plan))));
            }
            StorageRecovery { enabled } => {
                self.set_storage_recovery(*enabled).await?;
                return Ok(Some(None));
            }
            StorageApply { plan } if !config.remote.storage_started => {
                Self::validate_remote_runtime(&config, command)?;
                self.apply_storage_inner(serde_json::from_value(plan.clone())?)
                    .await?;
            }
            StorageRetry {} if !config.remote.storage_started => {
                self.retry_storage_maintenance().await?;
            }
            _ => {
                let vm = self.vm(&config)?;
                if let Some(operation) = &config.storage_operation {
                    anyhow::ensure!(
                        !operation.paused,
                        "Storage change is paused; review its current state before retrying"
                    );
                    self.storage_operation_inner(&config, &vm).await?;
                } else if let Some(operation) = &config.storage_lifecycle.maintenance {
                    anyhow::ensure!(
                        !operation.paused && operation.error.is_none(),
                        "Storage maintenance needs attention: {}",
                        operation.error.as_deref().unwrap_or("paused by owner")
                    );
                    self.maintenance_tick(&config, &vm).await?;
                } else if config.storage_lifecycle.missing.is_some() {
                    self.missing_storage_tick(&config, &vm).await?;
                }
            }
        }
        let current = self.store.load()?;
        self.store.check_configuration_authority(&current)?;
        if current.storage_operation.is_some()
            || current.storage_lifecycle.maintenance.is_some()
            || current.storage_lifecycle.missing.is_some()
        {
            Ok(None)
        } else {
            Ok(Some(None))
        }
    }

    fn finish_remote_storage(
        &self,
        command: &ConfigurationCommand,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<()> {
        self.store.update(|config| {
            let storage = json!(self.storage_snapshot(config, self.discover_storage(config)));
            if config
                .remote
                .pending
                .as_ref()
                .is_some_and(|p| p.edit == command.edit)
            {
                config.remote.pending = None;
                config.remote.storage_started = false;
                config.remote.execution_revision = None;
            }
            config.remote.record(ConfigurationReceipt {
                request_id: command.edit.request_id.clone(),
                status: if error.is_some() {
                    "rejected"
                } else if result.is_some() {
                    "reviewed"
                } else {
                    "applied"
                }
                .into(),
                revision: config.remote.revision,
                at: chrono::Utc::now().to_rfc3339(),
                effective_policy: Some(config.policy.clone()),
                effective_resources: Self::configuration_effective_resources(config),
                effective_storage: config.remote.consent.then_some(storage),
                result,
                error,
            });
            Ok(())
        })?;
        Ok(())
    }

    async fn fail_remote_storage(&self, command: &ConfigurationCommand, error: &str) -> Result<()> {
        let config = self.store.update(|c| {
            if c.remote
                .pending
                .as_ref()
                .is_some_and(|p| p.edit == command.edit)
                && c.remote.storage_started
            {
                Store::pause_remote_storage(c);
            }
            Ok(())
        })?;
        // An interrupted disk phase is never resumed without another explicit
        // local action or a fresh, owner-authorized remote retry.
        let mut error = format!("{error}. Inspect storage status and preserved copies, resolve the problem, then retry maintenance or review again.");
        if config.storage_operation.is_some() || config.storage_lifecycle.maintenance.is_some() {
            let vm = self.vm(&config)?;
            if vm.has_receipt()? {
                if let Err(stop) = vm.stop_now().await {
                    error.push_str(&format!(" Worker stop also failed: {stop}"));
                }
            }
        }
        self.finish_remote_storage(command, None, Some(error.clone()))?;
        self.set_status("error", error).await;
        Ok(())
    }
}

/// A storage/compute command already in flight may fail partially. No following
/// command is allowed to start after consent or the owner's choices change.
pub(super) struct ConfigurationRunner {
    pub store: Store,
    pub inner: Arc<dyn crate::Runner>,
}
#[async_trait::async_trait]
impl crate::Runner for ConfigurationRunner {
    fn provider(&self) -> crate::VmProvider {
        self.inner.provider()
    }
    async fn replacement_space(
        &self,
        name: &str,
    ) -> Result<crate::storage_lifecycle::ReplacementSpace> {
        self.store.check_runtime_authority()?;
        let result = self.inner.replacement_space(name).await?;
        self.store.check_runtime_authority()?;
        Ok(result)
    }
    async fn stream(
        &self,
        args: &[String],
        input: Option<std::fs::File>,
        output: Option<std::fs::File>,
        limit: u64,
    ) -> Result<()> {
        self.store.check_runtime_authority()?;
        self.inner.stream(args, input, output, limit).await?;
        self.store.check_runtime_authority()
    }
    async fn run(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<crate::CommandOutput> {
        self.store.check_runtime_authority()?;
        let result = self.inner.run(args, stdin, timeout).await?;
        self.store.check_runtime_authority()?;
        Ok(result)
    }
    async fn run_with_progress(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
        progress: crate::ProgressSink,
    ) -> Result<crate::CommandOutput> {
        self.store.check_runtime_authority()?;
        let result = self
            .inner
            .run_with_progress(args, stdin, timeout, progress)
            .await?;
        self.store.check_runtime_authority()?;
        Ok(result)
    }
}
