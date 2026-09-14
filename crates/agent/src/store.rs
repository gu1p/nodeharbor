use anyhow::{Context, Result};
use fs2::FileExt;
use nodeharbor_core::Policy;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecreation {
    pub request_id: Uuid,
    pub access_removed: bool,
    #[serde(default)]
    pub target_provider: Option<crate::VmProvider>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub format_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_notice: Option<String>,
    #[serde(default)]
    pub remote: nodeharbor_core::configuration::RemoteConfiguration,
    #[serde(default = "updates_enabled")]
    pub automatic_updates: bool,
    #[serde(default)]
    pub application_update_pending: bool,
    #[serde(default)]
    pub vm_provider: crate::VmProvider,
    pub device_id: String,
    pub name: String,
    pub policy: Policy,
    pub controller_url: Option<String>,
    pub device_token: Option<String>,
    #[serde(default)]
    pub prepare_requested: bool,
    #[serde(default)]
    pub stop_requested: bool,
    #[serde(default)]
    pub draining_since: Option<u64>,
    #[serde(default)]
    pub vm_created: bool,
    #[serde(default)]
    pub vm_configured: bool,
    #[serde(default)]
    pub allocated_resources: Option<nodeharbor_core::Resources>,
    #[serde(default)]
    pub storage_locations: Vec<crate::storage::Location>,
    #[serde(default)]
    pub storage_revision: u64,
    #[serde(default)]
    pub storage_generation: u64,
    #[serde(default)]
    pub storage_operation: Option<crate::storage::Operation>,
    #[serde(default)]
    pub storage_retained: Vec<crate::storage::Location>,
    #[serde(default)]
    pub storage_boot_gib: u64,
    #[serde(default)]
    pub storage_lifecycle: crate::storage_lifecycle::Lifecycle,
    #[serde(default)]
    pub recreation: Option<WorkerRecreation>,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            format_version: if cfg!(target_os = "linux") { 2 } else { 1 },
            setup_notice: None,
            remote: Default::default(),
            automatic_updates: true,
            application_update_pending: false,
            vm_provider: if cfg!(target_os = "linux") {
                crate::VmProvider::Lima
            } else {
                crate::VmProvider::Multipass
            },
            device_id: Uuid::new_v4().to_string(),
            name: sysinfo::System::host_name().unwrap_or_else(|| "My computer".into()),
            policy: Policy::default(),
            controller_url: None,
            device_token: None,
            prepare_requested: false,
            stop_requested: false,
            draining_since: None,
            vm_created: false,
            vm_configured: false,
            allocated_resources: None,
            storage_locations: Vec::new(),
            storage_revision: 0,
            storage_generation: 0,
            storage_operation: None,
            storage_retained: Vec::new(),
            storage_boot_gib: 0,
            storage_lifecycle: Default::default(),
            recreation: None,
        }
    }
}
fn updates_enabled() -> bool {
    true
}
#[derive(Clone)]
pub struct Store {
    pub directory: PathBuf,
    configuration_request: Option<nodeharbor_core::configuration::ConfigurationEdit>,
}
impl Store {
    pub fn default_directory() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("Cannot locate this user’s settings directory")?
            .join("nodeharbor"))
    }
    pub fn open(directory: &Path) -> Result<Self> {
        Self::open_for_platform(directory, std::env::consts::OS)
    }
    fn open_for_platform(directory: &Path, platform: &str) -> Result<Self> {
        let provider = if platform == "linux" {
            crate::VmProvider::Lima
        } else {
            Configuration::default().vm_provider
        };
        Self::open_inner(directory, provider, platform == "linux")
    }
    /// Explicit protocol fixtures must not inherit the machine's native runtime.
    /// Native MultipassRunner still rejects Linux execution independently.
    pub(crate) fn open_with_provider(
        directory: &Path,
        provider: crate::VmProvider,
    ) -> Result<Self> {
        Self::open_inner(directory, provider, false)
    }
    fn open_inner(
        directory: &Path,
        provider: crate::VmProvider,
        reset_obsolete_linux: bool,
    ) -> Result<Self> {
        fs::create_dir_all(directory).context("Cannot create the NodeHarbor settings directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let store = Self {
            directory: directory.to_path_buf(),
            configuration_request: None,
        };
        let _lock = store.lock()?;
        if !store.path().exists() {
            store.write(&Configuration {
                vm_provider: provider,
                format_version: if provider == crate::VmProvider::Lima {
                    2
                } else {
                    1
                },
                ..Configuration::default()
            })?;
        }
        let config = store.load()?;
        if reset_obsolete_linux && config.vm_provider == crate::VmProvider::Multipass {
            // Never replace the active supervisor's identity or touch its VM.
            let _supervisor = store.supervisor_lock()?;
            let mut archive = tempfile::NamedTempFile::new_in(directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                archive
                    .as_file()
                    .set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            std::io::copy(&mut File::open(store.path())?, &mut archive)?;
            archive.as_file().sync_all()?;
            archive.persist_noclobber(
                directory.join(format!("obsolete-multipass-{}.json", Uuid::new_v4())),
            )?;
            store.write(&Configuration {
                vm_provider: crate::VmProvider::Lima,
                format_version: 2,
                setup_notice: Some("The previous Linux Multipass setup is obsolete and its settings were archived privately. Please enroll again and select drives to prepare a new Lima worker. Sharing is disabled.".into()),
                ..Configuration::default()
            })?;
        }
        Ok(store)
    }
    fn path(&self) -> PathBuf {
        self.directory.join("config.json")
    }
    fn lock(&self) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join("config.lock"))?;
        file.lock_exclusive()?;
        Ok(file)
    }
    pub fn load(&self) -> Result<Configuration> {
        let file = File::open(self.path()).context("Cannot read NodeHarbor settings")?;
        let config: Configuration = serde_json::from_reader(file)
            .context("NodeHarbor settings are damaged; the original file has been preserved")?;
        anyhow::ensure!(
            matches!(config.format_version, 1..=6),
            "These settings require a newer NodeHarbor application"
        );
        anyhow::ensure!(
            config.vm_provider != crate::VmProvider::Lima || config.format_version >= 2,
            "The worker runtime requires versioned settings; the original file has been preserved"
        );
        anyhow::ensure!(
            config.storage_locations.is_empty() || config.format_version >= 3,
            "Storage locations require versioned settings; the original file has been preserved"
        );
        anyhow::ensure!(
            (config.storage_operation.is_none()
                && config.storage_generation == 0
                && config.storage_revision == 0
                && config.storage_retained.is_empty()
                && config.storage_boot_gib == 0)
                || config.format_version >= 4,
            "Storage operations require versioned settings; the original file has been preserved"
        );
        Uuid::parse_str(&config.device_id).context("Invalid saved device identity")?;
        Ok(config)
    }
    pub fn update(
        &self,
        change: impl FnOnce(&mut Configuration) -> Result<()>,
    ) -> Result<Configuration> {
        let _lock = self.lock()?;
        let mut config = self.load()?;
        self.check_configuration_authority(&config)?;
        let before = config.clone();
        change(&mut config)?;
        // All local entry points, including CLI and another app process, advance
        // the same revision under the existing cross-process settings lock.
        if before.policy != config.policy
            || before.remote.consent != config.remote.consent
            || before.device_id != config.device_id
            || before.vm_provider != config.vm_provider
            || before.controller_url != config.controller_url
            || before.device_token != config.device_token
            || before.storage_revision != config.storage_revision
            || before.storage_locations != config.storage_locations
            || before.storage_lifecycle.recovery_enabled
                != config.storage_lifecycle.recovery_enabled
            || before.remote.revision != config.remote.revision
            || before.recreation.is_some() != config.recreation.is_some()
            || before.application_update_pending != config.application_update_pending
        {
            config.remote.revision = before
                .remote
                .revision
                .checked_add(1)
                .context("Configuration revision exhausted; local recovery required")?;
            if self.configuration_request.is_some() {
                config.remote.execution_revision = Some(config.remote.revision);
                if before.storage_revision != config.storage_revision {
                    config.remote.storage_started = true;
                }
                if let Some(receipt) = config
                    .remote
                    .receipts
                    .iter_mut()
                    .find(|r| r.status == "pending")
                {
                    receipt.revision = config.remote.revision;
                }
            } else if let Some(command) = config.remote.pending.take() {
                if config.remote.storage_started {
                    Self::pause_remote_storage(&mut config);
                }
                config.remote.execution_revision = None;
                config.remote.storage_started = false;
                config
                    .remote
                    .record(nodeharbor_core::configuration::ConfigurationReceipt {
                        result: None,
                        effective_storage: None,
                        request_id: command.edit.request_id,
                        status: "rejected".into(),
                        revision: config.remote.revision,
                        at: chrono::Utc::now().to_rfc3339(),
                        effective_policy: Some(config.policy.clone()),
                        effective_resources: if config.remote.applying
                            || config.storage_operation.is_some()
                            || config.storage_lifecycle.maintenance.is_some()
                        {
                            None
                        } else {
                            config.allocated_resources.clone()
                        },
                        error: Some(
                            "The owner changed settings or consent; reload current settings".into(),
                        ),
                    });
            }
        }
        self.write(&config)?;
        Ok(config)
    }
    /// Scope the existing storage lifecycle to a single authenticated request.
    /// Every durable write checks the latest owner consent under the file lock.
    pub(crate) fn for_configuration(
        &self,
        edit: &nodeharbor_core::configuration::ConfigurationEdit,
    ) -> Self {
        Self {
            directory: self.directory.clone(),
            configuration_request: Some(edit.clone()),
        }
    }
    pub(crate) fn check_configuration_authority(&self, config: &Configuration) -> Result<()> {
        if let Some(edit) = &self.configuration_request {
            anyhow::ensure!(config.remote.consent
                && config.remote.pending.as_ref().is_some_and(|p| &p.edit == edit)
                && config.remote.revision == config.remote.execution_revision.unwrap_or(edit.expected_revision),
                "The owner changed settings or revoked remote configuration; unapplied changes are canceled");
        }
        Ok(())
    }
    pub(crate) fn check_runtime_authority(&self) -> Result<()> {
        if self.configuration_request.is_some() {
            self.check_configuration_authority(&self.load()?)?;
        }
        Ok(())
    }
    pub(crate) fn pause_remote_storage(config: &mut Configuration) {
        if config
            .storage_operation
            .as_ref()
            .is_some_and(|op| op.phase == "pending")
        {
            config.storage_operation = None;
        } else if let Some(op) = &mut config.storage_operation {
            op.paused = true;
        }
        if config
            .storage_lifecycle
            .maintenance
            .as_ref()
            .is_some_and(|op| op.phase == crate::storage_lifecycle::Phase::Drain)
        {
            config.storage_lifecycle.maintenance = None;
        } else if let Some(op) = &mut config.storage_lifecycle.maintenance {
            op.paused = true;
        }
    }
    fn write(&self, config: &Configuration) -> Result<()> {
        let mut config = config.clone();
        if config.remote.consent
            || config.remote.pending.is_some()
            || !config.remote.receipts.is_empty()
        {
            // Earlier agents understand local storage journals but cannot
            // enforce the remote request's consent and revision while applying.
            config.format_version = config.format_version.max(6);
        }
        if serde_json::to_value(&config.storage_lifecycle)?
            != serde_json::to_value(crate::storage_lifecycle::Lifecycle::default())?
        {
            config.format_version = config.format_version.max(5);
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer_pretty(&mut temporary, &config)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.path())
            .context("Cannot replace NodeHarbor settings")?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
    pub fn supervisor_lock(&self) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join("supervisor.lock"))?;
        file.try_lock_exclusive()
            .context("NodeHarbor’s background worker is already running")?;
        Ok(file)
    }
}

#[cfg(test)]
mod linux_setup_tests {
    use super::*;

    #[test]
    fn linux_store_starts_with_lima_and_sharing_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_for_platform(dir.path(), "linux").unwrap();
        let config = store.load().unwrap();
        assert_eq!(config.vm_provider, crate::VmProvider::Lima);
        assert!(config.format_version >= 2);
        assert!(!config.policy.enabled);
        assert!(config.device_token.is_none());
        assert_eq!(
            Store::open_for_platform(dir.path(), "linux")
                .unwrap()
                .load()
                .unwrap()
                .device_id,
            config.device_id
        );
    }

    #[test]
    fn obsolete_linux_configuration_is_archived_once_and_requires_enrollment() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = Configuration {
            vm_provider: crate::VmProvider::Multipass,
            ..Configuration::default()
        };
        old.policy.enabled = true;
        old.device_token = Some("fixture-token".into());
        let original = serde_json::to_vec(&old).unwrap();
        fs::write(dir.path().join("config.json"), &original).unwrap();
        let store = Store::open_for_platform(dir.path(), "linux").unwrap();
        let config = store.load().unwrap();
        assert_eq!(config.vm_provider, crate::VmProvider::Lima);
        assert!(!config.policy.enabled);
        assert!(config.device_token.is_none());
        assert_ne!(config.device_id, old.device_id);
        assert!(config
            .setup_notice
            .as_deref()
            .unwrap()
            .contains("enroll again"));
        let archives: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("obsolete-multipass-")
            })
            .collect();
        assert_eq!(archives.len(), 1);
        assert_eq!(fs::read(archives[0].path()).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(archives[0].path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            Store::open_for_platform(dir.path(), "linux")
                .unwrap()
                .load()
                .unwrap()
                .device_id,
            config.device_id
        );
    }
}
