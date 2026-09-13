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
    pub recreation: Option<WorkerRecreation>,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            format_version: 1,
            automatic_updates: true,
            application_update_pending: false,
            vm_provider: crate::VmProvider::Multipass,
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
}
impl Store {
    pub fn default_directory() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("Cannot locate this user’s settings directory")?
            .join("nodeharbor"))
    }
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory).context("Cannot create the NodeHarbor settings directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let store = Self {
            directory: directory.to_path_buf(),
        };
        let _lock = store.lock()?;
        if !store.path().exists() {
            store.write(&Configuration::default())?;
        }
        store.load()?;
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
            matches!(config.format_version, 1 | 2),
            "These settings require a newer NodeHarbor application"
        );
        anyhow::ensure!(
            config.vm_provider != crate::VmProvider::Lima || config.format_version == 2,
            "The worker runtime requires versioned settings; the original file has been preserved"
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
        change(&mut config)?;
        self.write(&config)?;
        Ok(config)
    }
    fn write(&self, config: &Configuration) -> Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer_pretty(&mut temporary, config)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.path())
            .context("Cannot replace NodeHarbor settings")?;
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
