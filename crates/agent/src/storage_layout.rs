//! Owner allocations include every VM disk. Runtime adapters receive only the
//! derived data-disk allocations; this boundary keeps the two budgets distinct.
use crate::storage::Location;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const GIB: u64 = 1 << 30;
pub const SYSTEM_GIB: u64 = 16;
pub const MINIMUM_TOTAL_GIB: u64 = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layout {
    pub version: u32,
    pub system_location_id: String,
    pub volume_id: String,
    pub runtime_directory: String,
    pub system_gib: u64,
}

impl Layout {
    pub fn resolve(owner: &str, locations: &[Location], previous: Option<&Self>) -> Result<Self> {
        let owner = uuid::Uuid::parse_str(owner)?;
        let system_gib = previous.map_or(SYSTEM_GIB, |p| p.system_gib);
        let total = locations
            .iter()
            .try_fold(0_u64, |n, l| n.checked_add(l.allocation_gib))
            .context("Storage allocation overflow")?;
        anyhow::ensure!(
            total >= system_gib + 14,
            "Choose at least {} GiB total VM storage, including {} GiB for the operating system",
            system_gib + 14,
            system_gib
        );
        let system = previous.and_then(|p| locations.iter().find(|l| l.id == p.system_location_id && l.allocation_gib > system_gib))
            .or_else(|| locations.iter().find(|l| l.allocation_gib > system_gib))
            .context("One selected location needs enough space for the VM operating system and at least 1 GiB of data storage")?;
        let runtime_directory =
            Path::new(&system.directory).join(format!(".nh{}", &owner.simple().to_string()[..8]));
        Ok(Self {
            version: 1,
            system_location_id: system.id.clone(),
            volume_id: system.volume_id.clone(),
            runtime_directory: runtime_directory.to_string_lossy().into(),
            system_gib,
        })
    }

    pub fn data_locations(&self, total: &[Location]) -> Result<Vec<Location>> {
        self.transform(total, false)
    }
    pub fn total_locations(&self, data: &[Location]) -> Result<Vec<Location>> {
        self.transform(data, true)
    }
    fn transform(&self, locations: &[Location], add: bool) -> Result<Vec<Location>> {
        anyhow::ensure!(
            self.version == 1 && self.system_gib >= SYSTEM_GIB,
            "Unsupported VM storage layout"
        );
        let mut locations = locations.to_vec();
        let system = locations
            .iter_mut()
            .find(|l| l.id == self.system_location_id)
            .context("The selected VM system location is missing")?;
        anyhow::ensure!(
            system.volume_id == self.volume_id,
            "The VM system volume changed"
        );
        system.allocation_gib = if add {
            system.allocation_gib.checked_add(self.system_gib)
        } else {
            system
                .allocation_gib
                .checked_sub(self.system_gib)
                .filter(|v| *v > 0)
        }
        .context("The VM system location has an insufficient allocation")?;
        Ok(locations)
    }

    pub fn validate_path(&self) -> Result<()> {
        let path = Path::new(&self.runtime_directory);
        anyhow::ensure!(
            path.is_absolute()
                && !path
                    .components()
                    .any(|p| p == std::path::Component::ParentDir),
            "Choose an absolute VM storage directory without parent traversal"
        );
        // Lima and OpenSSH both use pathname Unix sockets, including a random
        // suffix on SSH's temporary socket. Check bytes, not Unicode characters.
        let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
        for suffix in [
            "worker/ssh.sock.1234567890123456",
            "_networks/user-v2/user-v2_ep.sock",
            #[cfg(target_os = "linux")]
            "_networks/user-v2/user-v2_qemu.sock",
        ] {
            anyhow::ensure!(path.join(suffix).as_os_str().len() < limit, "The selected folder path is too long for the VM runtime. Choose a shorter folder on the same drive");
        }
        Ok(())
    }

    pub fn home(&self) -> PathBuf {
        PathBuf::from(&self.runtime_directory)
    }
}

pub fn remaining_bytes(allocation_gib: u64, allocated_bytes: u64) -> Result<u64> {
    Ok(allocation_gib
        .checked_mul(GIB)
        .context("Storage allocation overflow")?
        .saturating_sub(allocated_bytes))
}

impl crate::Configuration {
    pub fn resolve_storage_layout(&self, locations: &[Location]) -> Result<Layout> {
        if let Some(previous) = &self.storage_layout {
            return Layout::resolve(&self.device_id, locations, Some(previous));
        }
        let mut previous = Layout::resolve(&self.device_id, locations, None)?;
        previous.system_gib = self.system_gib();
        Layout::resolve(&self.device_id, locations, Some(&previous))
    }
    pub fn runtime_home(&self, settings: &Path) -> PathBuf {
        self.storage_layout
            .as_ref()
            .map_or_else(|| settings.join("lima"), Layout::home)
    }
    pub fn system_gib(&self) -> u64 {
        self.storage_layout.as_ref().map_or_else(
            || {
                if self.storage_boot_gib > 0 {
                    self.storage_boot_gib
                } else if self.vm_created && self.storage_locations.is_empty() {
                    self.allocated_resources
                        .as_ref()
                        .map_or(SYSTEM_GIB, |r| r.disk_gib.max(SYSTEM_GIB))
                } else {
                    SYSTEM_GIB
                }
            },
            |l| l.system_gib,
        )
    }
    pub fn total_locations(&self) -> Result<Vec<Location>> {
        if self.storage_locations.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(layout) = &self.storage_layout {
            layout.total_locations(&self.storage_locations)
        } else {
            // A legacy worker already uses this extra disk. Expose its actual
            // total without shrinking or moving anything during configuration IO.
            let mut locations = self.storage_locations.clone();
            locations[0].allocation_gib = locations[0]
                .allocation_gib
                .checked_add(self.system_gib())
                .context("Storage allocation overflow")?;
            Ok(locations)
        }
    }
    pub fn total_storage_gib(&self) -> u64 {
        self.storage_locations
            .iter()
            .map(|l| l.allocation_gib)
            .sum::<u64>()
            .saturating_add(if self.storage_locations.is_empty() {
                0
            } else {
                self.system_gib()
            })
    }
}
