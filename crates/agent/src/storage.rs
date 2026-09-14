//! Host storage discovery and preflight. No VM images are created by this module.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub id: String,
    pub capacity_pool: String,
    pub label: String,
    pub mount_point: String,
    pub filesystem: String,
    pub available_gib: u64,
    pub configured_gib: u64,
    pub eligible: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub directory: String,
    pub allocation_gib: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Location {
    pub id: String,
    pub volume_id: String,
    pub directory: String,
    pub allocation_gib: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationStatus {
    #[serde(flatten)]
    pub location: Location,
    pub available: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub active_gib: u64,
    pub configured_gib: u64,
    pub recovery_enabled: bool,
    pub disabled: bool,
    pub excluded: Vec<LocationStatus>,
    pub backup_cleanup: Vec<crate::storage_lifecycle::Backup>,
    pub recovery_backup: Option<crate::storage_lifecycle::Backup>,
    pub default_directory: Option<String>,
    pub supported: bool,
    pub reason: String,
    pub volumes: Vec<Volume>,
    pub locations: Vec<LocationStatus>,
    pub revision: u64,
    pub operation: Option<OperationStatus>,
    pub retained_copies: Vec<LocationStatus>,
    pub system_disk: Option<SystemDisk>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemDisk {
    pub directory: String,
    pub allocation_gib: u64,
}

/// Reserve remaining possible system-image growth once for its entire host pool.
pub fn reserve_system_disk(
    volumes: &mut [Volume],
    directory: &Path,
    remaining_gib: u64,
) -> Result<()> {
    let volume = volume_for(directory, volumes)
        .context("The application system-disk volume is unavailable")?;
    anyhow::ensure!(
        volume.eligible,
        "The application system-disk volume is unsupported: {}",
        volume.reason
    );
    anyhow::ensure!(
        volume.available_gib >= remaining_gib.saturating_add(10),
        "Not enough space on the application volume for its system disk and 10 GiB host reserve"
    );
    let pool = volume.capacity_pool.clone();
    for volume in volumes
        .iter_mut()
        .filter(|volume| volume.capacity_pool == pool)
    {
        volume.available_gib = volume.available_gib.saturating_sub(remaining_gib);
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangePlan {
    #[serde(default)]
    pub maintenance: Option<crate::storage_lifecycle::Review>,
    pub revision: u64,
    pub locations: Vec<Location>,
    pub total_gib: u64,
    pub requires_restart: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStatus {
    pub phase: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub previous: Vec<Location>,
    pub target: Vec<Location>,
    pub generation: u64,
    pub phase: String,
    #[serde(default)]
    pub paused: bool,
}

pub fn unsupported_reason(provider: crate::VmProvider) -> &'static str {
    match provider {
        crate::VmProvider::Lima => "Selectable storage requires the packaged Lima runtime on a supported Mac or Linux host.",
        crate::VmProvider::Multipass => "Multipass does not support selectable locations or additional disks for an individual worker. Its daemon manages the disk location and does not report that path through its supported API.",
    }
}

/// Do not infer support from a filesystem or a writable directory. The public
/// attachment API must support the requested placement before any apply operation.
pub fn require_location_support(provider: crate::VmProvider) -> Result<()> {
    anyhow::ensure!(
        provider == crate::VmProvider::Lima && cfg!(any(target_os = "macos", target_os = "linux")),
        "{}",
        unsupported_reason(provider)
    );
    Ok(())
}

pub(crate) fn canonical_directory(path: &Path) -> Result<PathBuf> {
    anyhow::ensure!(path.is_absolute(), "Choose an absolute storage directory");
    anyhow::ensure!(
        !path
            .components()
            .any(|part| part == std::path::Component::ParentDir),
        "Storage directories cannot contain parent traversal"
    );
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            let path = path
                .canonicalize()
                .context("Storage directory is unavailable")?;
            anyhow::ensure!(path.is_dir(), "Storage location must be a directory");
            Ok(path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Only the final directory may be new. In particular, never recreate
            // a disappeared mount point beneath an ancestor on the system disk.
            let parent = path
                .parent()
                .context("Choose a storage directory")?
                .canonicalize()
                .context("Storage volume or parent directory is unavailable")?;
            anyhow::ensure!(parent.is_dir(), "Storage parent must be a directory");
            Ok(parent.join(path.file_name().context("Choose a storage directory")?))
        }
        Err(error) => Err(error).context("Cannot inspect storage directory"),
    }
}

pub(crate) fn volume_for<'a>(path: &Path, volumes: &'a [Volume]) -> Option<&'a Volume> {
    #[cfg(target_os = "macos")]
    if volumes
        .iter()
        .any(|volume| uuid::Uuid::parse_str(&volume.id).is_ok())
    {
        // macOS can expose the same st_dev through a firmlink and the sealed
        // system root. Resolve native inventory by the volume UUID of the path.
        let existing = path.ancestors().find(|parent| parent.exists())?;
        let info = macos_volume(existing).ok()?;
        return volumes.iter().find(|volume| volume.id == info.uuid);
    }
    volumes
        .iter()
        .filter_map(|volume| {
            let mount = Path::new(&volume.mount_point).canonicalize().ok()?;
            path.starts_with(&mount)
                .then_some((volume, mount.components().count()))
        })
        .max_by_key(|(_, depth)| *depth)
        .map(|(volume, _)| volume)
}

/// A new allocation plan. Existing allocated disks must be handled separately;
/// adding their logical sizes to free space could overcommit sparse images.
/// The temporary permission probe is removed before returning.
pub fn plan(
    selections: &[Selection],
    default_directory: &Path,
    default_gib: u64,
    volumes: &[Volume],
) -> Result<Vec<Location>> {
    plan_change(
        selections,
        default_directory,
        default_gib,
        volumes,
        &[],
        &BTreeMap::new(),
    )
}

/// Credits are actual allocated host bytes, never the logical size of a sparse image.
pub fn plan_change(
    selections: &[Selection],
    default_directory: &Path,
    default_gib: u64,
    volumes: &[Volume],
    previous: &[Location],
    allocated_bytes: &BTreeMap<String, u64>,
) -> Result<Vec<Location>> {
    anyhow::ensure!(
        selections.len() <= 16,
        "Choose at most 16 storage locations"
    );
    let defaults;
    let selections = if selections.is_empty() {
        defaults = [Selection {
            id: None,
            directory: default_directory.to_string_lossy().into(),
            allocation_gib: default_gib,
        }];
        &defaults[..]
    } else {
        selections
    };
    let mut result: Vec<Location> = Vec::new();
    let mut budgets: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    let mut ids = std::collections::BTreeSet::new();
    for selection in selections {
        anyhow::ensure!(
            (1..=1_048_576).contains(&selection.allocation_gib),
            "Choose an allocation between 1 and 1048576 GiB"
        );
        anyhow::ensure!(
            selection.directory.len() <= 4096,
            "Storage directory is too long"
        );
        let path = canonical_directory(Path::new(&selection.directory))?;
        anyhow::ensure!(
            !result.iter().any(|other| path.starts_with(&other.directory)
                || Path::new(&other.directory).starts_with(&path)),
            "Storage directories must not duplicate or overlap"
        );
        let volume = volume_for(&path, volumes).context("Storage volume is unavailable")?;
        anyhow::ensure!(volume.eligible, "{}: {}", volume.label, volume.reason);
        anyhow::ensure!(
            matches!(
                volume.filesystem.to_lowercase().as_str(),
                "apfs" | "ext4" | "xfs"
            ),
            "Unsupported host filesystem: {}",
            volume.filesystem
        );
        anyhow::ensure!(
            !volume.id.is_empty() && !volume.capacity_pool.is_empty(),
            "Storage volume identity is unavailable"
        );
        let budget = budgets
            .entry(&volume.capacity_pool)
            .or_insert((0, volume.available_gib));
        let old = selection
            .id
            .as_ref()
            .map(|id| {
                anyhow::ensure!(ids.insert(id.clone()), "A disk cannot be selected twice");
                let old = previous
                    .iter()
                    .find(|old| &old.id == id)
                    .context("Unknown storage disk identity")?;
                Ok::<_, anyhow::Error>(old)
            })
            .transpose()?;
        let credit = old
            .filter(|old| old.directory == path.to_string_lossy() && old.volume_id == volume.id)
            .and_then(|old| allocated_bytes.get(&old.id))
            .copied()
            .unwrap_or(0)
            / (1024 * 1024 * 1024);
        budget.0 = budget
            .0
            .checked_add(selection.allocation_gib.saturating_sub(credit))
            .context("Storage allocation exceeds the supported size")?;
        budget.1 = budget.1.min(volume.available_gib);
        anyhow::ensure!(
            budget.0 <= budget.1.saturating_sub(10),
            "Not enough free space on {}; leave 10 GiB free in each storage pool",
            volume.label
        );
        let probe_directory = if path.exists() {
            path.as_path()
        } else {
            path.parent().context("Missing storage parent")?
        };
        anyhow::ensure!(
            !std::fs::metadata(probe_directory)?.permissions().readonly(),
            "Storage directory is read-only"
        );
        let probe = tempfile::Builder::new()
            .prefix(".nodeharbor-permission-")
            .tempfile_in(probe_directory)
            .context("No permission to write in the selected storage directory")?;
        probe
            .as_file()
            .sync_all()
            .context("Cannot write to the selected storage volume")?;
        result.push(Location {
            id: old.map(|old| old.id.clone()).unwrap_or_else(|| {
                format!("nh{}", &uuid::Uuid::new_v4().simple().to_string()[..9])
            }),
            volume_id: volume.id.clone(),
            directory: path.to_string_lossy().into(),
            allocation_gib: selection.allocation_gib,
        });
    }
    anyhow::ensure!(
        result
            .iter()
            .map(|location| location.allocation_gib)
            .sum::<u64>()
            >= 15,
        "Choose at least 15 GiB of combined worker storage"
    );
    // Existing attachment order remains stable. New disks append to the pool.
    result.sort_by_key(|location| {
        previous
            .iter()
            .position(|old| old.id == location.id)
            .unwrap_or(previous.len())
    });
    Ok(result)
}

pub fn inspect_locations(locations: &[Location], volumes: &[Volume]) -> Vec<LocationStatus> {
    locations.iter().map(|location| {
        let available = Path::new(&location.directory).canonicalize().ok().filter(|path| path.is_dir())
            .and_then(|path| volume_for(&path, volumes))
            .is_some_and(|volume| volume.id == location.volume_id && volume.eligible);
        LocationStatus {
            location: location.clone(), available,
            reason: if available { String::new() } else { "Storage volume or directory is unavailable or unsupported. Reconnect the original volume at its saved location; storage will not be redirected.".into() },
        }
    }).collect()
}

/// Physical system-image blocks on the application volume. Lima 2.2 uses
/// `disk`; older owned instances may still use `diffdisk`.
pub fn allocated_system_bytes(directory: &Path) -> Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for name in ["disk", "diffdisk"] {
            let path = directory.join("lima/worker").join(name);
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).context("Cannot inspect the worker system disk"),
            };
            anyhow::ensure!(
                file.metadata()?.is_file()
                    && volume_identity_file(&file)? == volume_identity(directory)?,
                "The worker system disk is not on its application volume"
            );
            return Ok(file.metadata()?.blocks().saturating_mul(512));
        }
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(0)
}

pub fn allocated_bytes(
    directory: &Path,
    device: &str,
    locations: &[Location],
) -> BTreeMap<String, u64> {
    locations
        .iter()
        .filter_map(|location| {
            let paths =
                crate::lima_storage::disk_paths(&directory.join("lima"), device, location).ok()?;
            let receipt: crate::lima_storage::DiskReceipt =
                serde_json::from_reader(std::fs::File::open(&paths.receipt).ok()?).ok()?;
            receipt.verify(device, location).ok()?;
            let metadata = std::fs::symlink_metadata(paths.image).ok()?;
            if !metadata.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                Some((location.id.clone(), metadata.blocks().saturating_mul(512)))
            }
            #[cfg(not(unix))]
            {
                None
            }
        })
        .collect()
}

pub fn inventory(
    provider: crate::VmProvider,
    directory: &Path,
    locations: &[Location],
    boot_gib: u64,
) -> Inventory {
    let default_directory = match provider {
        crate::VmProvider::Lima => directory
            .canonicalize()
            .ok()
            .map(|path| path.join("storage").to_string_lossy().into_owned()),
        crate::VmProvider::Multipass => None,
    };
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut volumes: Vec<_> = disks.iter().map(|disk| {
        let filesystem = disk.file_system().to_string_lossy().to_lowercase();
        let mut volume = Volume {
            id: String::new(), capacity_pool: String::new(), label: disk.name().to_string_lossy().into(),
            mount_point: disk.mount_point().to_string_lossy().into(), filesystem,
            available_gib: disk.available_space() / (1024 * 1024 * 1024), configured_gib: 0,
            eligible: false, reason: "Selectable worker storage is unsupported on this host/runtime combination".into(),
        };
        #[cfg(target_os = "macos")]
        match macos_volume(disk.mount_point()) {
            Ok(info) => {
                volume.id = info.uuid;
                volume.capacity_pool = info.pool;
                volume.eligible = volume.filesystem == "apfs" && !disk.is_read_only()
                    && !volume.id.is_empty() && info.ownership;
                volume.reason = if volume.eligible { String::new() } else { "Requires a writable APFS volume with ownership enabled and a stable volume identity".into() };
            }
            Err(_) => volume.reason = "Volume identity could not be read. Storage selection is unavailable.".into(),
        }
        #[cfg(target_os = "linux")]
        {
            if let Ok(identity) = linux_volume(disk.mount_point()) {
                volume.id = identity.clone();
                volume.capacity_pool = identity;
                volume.eligible = matches!(volume.filesystem.as_str(), "ext4" | "xfs") && !disk.is_read_only();
                volume.reason = if volume.eligible { String::new() } else { "Requires a writable local ext4 or XFS volume with a stable identity".into() };
            }
        }
        #[cfg(target_os = "windows")]
        {
            volume.id = windows_volume(disk.mount_point()).unwrap_or_default();
            volume.capacity_pool = volume.id.clone();
            volume.reason = unsupported_reason(provider).into();
        }
        volume.configured_gib = locations.iter().filter(|location| !volume.id.is_empty() && location.volume_id == volume.id)
            .fold(0_u64, |total, location| total.saturating_add(location.allocation_gib));
        volume
    }).collect();
    // A boot image's allocation belongs to the actual application volume. Do not
    // attribute a Multipass daemon's allocation to the user's settings directory.
    if provider == crate::VmProvider::Lima {
        if let Ok(path) = directory.canonicalize() {
            if let Some(mount) =
                volume_for(&path, &volumes).map(|volume| volume.mount_point.clone())
            {
                if let Some(volume) = volumes
                    .iter_mut()
                    .find(|volume| volume.mount_point == mount)
                {
                    volume.configured_gib = volume.configured_gib.saturating_add(boot_gib);
                }
            }
        }
    }
    let statuses = inspect_locations(locations, &volumes);
    Inventory {
        active_gib: locations
            .iter()
            .map(|location| location.allocation_gib)
            .sum(),
        configured_gib: locations
            .iter()
            .map(|location| location.allocation_gib)
            .sum(),
        recovery_enabled: false,
        disabled: false,
        excluded: Vec::new(),
        backup_cleanup: Vec::new(),
        recovery_backup: None,
        default_directory,
        supported: require_location_support(provider).is_ok(),
        reason: if require_location_support(provider).is_ok() {
            String::new()
        } else {
            unsupported_reason(provider).into()
        },
        volumes,
        locations: statuses,
        revision: 0,
        operation: None,
        retained_copies: Vec::new(),
        system_disk: (provider == crate::VmProvider::Lima).then(|| SystemDisk {
            directory: directory.join("lima/worker").to_string_lossy().into(),
            allocation_gib: boot_gib,
        }),
    }
}

#[cfg(target_os = "linux")]
fn linux_volume(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    linux_device_uuid(std::fs::metadata(path)?.dev())
}

#[cfg(target_os = "linux")]
fn linux_device_uuid(device: u64) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    for entry in std::fs::read_dir("/dev/disk/by-uuid")? {
        let entry = entry?;
        if std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.rdev() == device) {
            return Ok(entry.file_name().to_string_lossy().into_owned());
        }
    }
    anyhow::bail!("Storage volume UUID is unavailable")
}

/// Check the volume behind an already opened descriptor before writing image
/// bytes. A vanished mount must never turn into storage on its parent volume.
pub fn volume_identity_file(file: &std::fs::File) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        #[repr(C)]
        struct Buffer {
            length: u32,
            uuid: [u8; 16],
        }
        let mut attributes = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: 0,
            volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
            dirattr: 0,
            fileattr: 0,
            forkattr: 0,
        };
        let mut buffer = Buffer {
            length: 0,
            uuid: [0; 16],
        };
        // SAFETY: the borrowed descriptor is live and output matches ATTR_VOL_UUID.
        let result = unsafe {
            libc::fgetattrlist(
                file.as_raw_fd(),
                std::ptr::from_mut(&mut attributes).cast(),
                std::ptr::from_mut(&mut buffer).cast(),
                std::mem::size_of::<Buffer>(),
                0,
            )
        };
        anyhow::ensure!(
            result == 0
                && buffer.length as usize == std::mem::size_of::<Buffer>()
                && buffer.uuid != [0; 16],
            "Cannot verify the opened storage volume: {}",
            std::io::Error::last_os_error()
        );
        Ok(uuid::Uuid::from_bytes(buffer.uuid).to_string())
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        linux_device_uuid(file.metadata()?.dev())
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFinalPathNameByHandleW(
                file: *mut std::ffi::c_void,
                path: *mut u16,
                length: u32,
                flags: u32,
            ) -> u32;
        }
        let mut path = vec![0u16; 32768];
        // SAFETY: the borrowed handle remains live and the output buffer has
        // exactly the supplied capacity. VOLUME_NAME_GUID returns stable IDs.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                path.as_mut_ptr(),
                path.len() as u32,
                1,
            )
        };
        anyhow::ensure!(
            length > 0 && (length as usize) < path.len(),
            "Cannot verify the opened backup volume"
        );
        let path = String::from_utf16(&path[..length as usize])?;
        let end = path
            .find("}\\")
            .context("Backup has no stable Windows volume identity")?
            + 2;
        Ok(path[..end].to_owned())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = file;
        anyhow::bail!("File-backed storage placement is unsupported on this host")
    }
}

pub fn volume_identity(path: &Path) -> Result<String> {
    volume_identity_file(&std::fs::File::open(path).context("Storage location is unavailable")?)
}

#[cfg(target_os = "windows")]
fn windows_volume(path: &Path) -> Result<String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumeNameForVolumeMountPointW(
            mount: *const u16,
            volume: *mut u16,
            length: u32,
        ) -> i32;
    }
    let mut mount: Vec<u16> = path.as_os_str().encode_wide().collect();
    if !mount.ends_with(&[b'\\' as u16]) {
        mount.push(b'\\' as u16);
    }
    mount.push(0);
    let mut volume = [0_u16; 128];
    // SAFETY: both buffers are valid; mount is terminated and output length is exact.
    anyhow::ensure!(
        unsafe {
            GetVolumeNameForVolumeMountPointW(
                mount.as_ptr(),
                volume.as_mut_ptr(),
                volume.len() as u32,
            )
        } != 0,
        "Cannot read Windows volume identity"
    );
    Ok(String::from_utf16(
        &volume[..volume
            .iter()
            .position(|c| *c == 0)
            .context("Invalid volume identity")?],
    )?)
}

#[cfg(target_os = "macos")]
struct MacVolume {
    uuid: String,
    pool: String,
    ownership: bool,
}

#[cfg(target_os = "macos")]
fn macos_volume(path: &Path) -> Result<MacVolume> {
    use std::{
        ffi::{CStr, CString},
        os::unix::ffi::OsStrExt,
    };
    #[repr(C)]
    struct UuidBuffer {
        length: u32,
        uuid: [u8; 16],
    }
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut buffer = UuidBuffer {
        length: 0,
        uuid: [0; 16],
    };
    // SAFETY: valid NUL-terminated path, initialized attrlist, and a writable
    // buffer exactly matching getattrlist's length + ATTR_VOL_UUID layout.
    let result = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            std::ptr::from_mut(&mut attributes).cast(),
            std::ptr::from_mut(&mut buffer).cast(),
            std::mem::size_of::<UuidBuffer>(),
            0,
        )
    };
    anyhow::ensure!(
        result == 0,
        "Cannot read volume UUID: {}",
        std::io::Error::last_os_error()
    );
    anyhow::ensure!(
        buffer.length as usize == std::mem::size_of::<UuidBuffer>() && buffer.uuid != [0; 16],
        "Volume UUID is unavailable"
    );
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: statfs initializes stat on a successful return; no fields are read
    // before that return is checked. Mount source is a NUL-terminated OS string.
    anyhow::ensure!(
        unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } == 0,
        "Cannot inspect storage filesystem"
    );
    let stat = unsafe { stat.assume_init() };
    let source = unsafe { CStr::from_ptr(stat.f_mntfromname.as_ptr()) }.to_str()?;
    // APFS synthesized volumes in one current mount-table container share space.
    // This pool key is never persisted as a volume identity across reboots.
    let container = source
        .strip_prefix("/dev/disk")
        .and_then(|rest| rest.split_once('s'))
        .map(|(number, _)| number)
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()));
    let uuid = uuid::Uuid::from_bytes(buffer.uuid).to_string();
    let pool = container
        .map(|number| format!("apfs-container-disk{number}"))
        .unwrap_or_else(|| uuid.clone());
    Ok(MacVolume {
        uuid,
        pool,
        ownership: stat.f_flags & libc::MNT_IGNORE_OWNERSHIP as u32 == 0,
    })
}
