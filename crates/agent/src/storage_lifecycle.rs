//! Durable replacement and recovery records. Host paths stay in local settings.
use crate::storage::Location;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub const GIB: u64 = 1 << 30;

/// A backup must survive deletion of the managed VM and image directories.
/// Resolve aliases even when a forbidden directory has not been created yet.
pub fn require_backup_outside(backup: &Path, replaced: &Path) -> Result<()> {
    anyhow::ensure!(
        replaced.is_absolute()
            && !replaced
                .components()
                .any(|part| part == std::path::Component::ParentDir),
        "Cannot verify the managed storage directory"
    );
    let backup = crate::storage::canonical_directory(backup)?;
    let mut ancestor = replaced;
    let mut missing = Vec::new();
    let mut resolved = loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                break ancestor
                    .canonicalize()
                    .context("Cannot resolve managed storage directory")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .context("Cannot resolve managed storage directory")?,
                );
                ancestor = ancestor
                    .parent()
                    .context("Cannot resolve managed storage directory")?;
            }
            Err(error) => return Err(error).context("Cannot inspect managed storage directory"),
        }
    };
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    anyhow::ensure!(
        !backup.starts_with(resolved),
        "Choose a backup folder outside the managed VM and image directories that will be replaced"
    );
    Ok(())
}

/// Read-only runtime evidence; never included in ordinary heartbeats.
pub struct ReplacementSpace {
    pub directory: std::path::PathBuf,
    pub reclaimed_bytes: u64,
}

/// An explicit active-pool edit does not forget other disks excluded by recovery.
pub fn configured_after_change(
    configured: &[Location],
    previous_active: &[Location],
    target: &[Location],
) -> Vec<Location> {
    let mut result = target.to_vec();
    result.extend(
        configured
            .iter()
            .filter(|old| {
                !previous_active.iter().chain(target).any(|location| {
                    location.directory == old.directory && location.volume_id == old.volume_id
                })
            })
            .cloned(),
    );
    result
}

pub fn replacement_space_required(target_gib: u64, reclaimed_bytes: u64, backup_bytes: u64) -> u64 {
    target_gib
        .saturating_sub(reclaimed_bytes / GIB)
        .saturating_add(backup_bytes.div_ceil(GIB))
        .saturating_add(10)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewOptions {
    #[serde(default)]
    pub delete_all: bool,
    pub temporary_directory: Option<String>,
    pub single_disk_gib: Option<u64>,
    pub restore_disk: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    ResizeRemove,
    DeleteAll,
    FailureRecovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupLocation {
    pub directory: String,
    pub volume_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub kind: Kind,
    pub backup: Option<BackupLocation>,
    pub minimum_gib: u64,
    pub temporary_bytes: u64,
    pub deletions: Vec<String>,
    pub downtime: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub path: String,
    pub volume_id: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub verified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Drain,
    Backup,
    BackupVerified,
    Reset,
    Delete,
    Create,
    Restore,
    Verify,
    Bootstrap,
    Capacity,
    Cleanup,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Maintenance {
    pub request_id: uuid::Uuid,
    pub pool_id: uuid::Uuid,
    pub generation: u64,
    pub review: Review,
    pub previous: Vec<Location>,
    pub target: Vec<Location>,
    pub total_gib: u64,
    pub phase: Phase,
    pub backup: Option<Backup>,
    pub paused: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Missing {
    pub since: u64,
    pub last_check: u64,
    pub locations: Vec<Location>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Lifecycle {
    pub initializing: bool,
    pub disabled: bool,
    pub recovery_enabled: bool,
    pub configured_locations: Vec<Location>,
    pub pool_id: Option<uuid::Uuid>,
    pub maintenance: Option<Maintenance>,
    pub missing: Option<Missing>,
    pub retired: Vec<Location>,
    pub cleanup: Vec<Backup>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryDecision {
    Wait,
    Paused,
    Rebuild,
    Insufficient,
    Failed,
}

pub fn recovery_decision(
    consent: bool,
    owner_allowed: bool,
    since: u64,
    now: u64,
    remaining_gib: u64,
    failed: bool,
) -> RecoveryDecision {
    if !owner_allowed {
        return RecoveryDecision::Paused;
    }
    if failed {
        return RecoveryDecision::Failed;
    }
    if !consent || now.saturating_sub(since) < 120 {
        return RecoveryDecision::Wait;
    }
    if remaining_gib < 15 {
        return RecoveryDecision::Insufficient;
    }
    RecoveryDecision::Rebuild
}

pub fn required_capacity(data_bytes: u64, system_disk: bool) -> Result<u64> {
    let bytes = data_bytes
        .checked_mul(5)
        .context("Worker data size overflow")?
        .div_ceil(4);
    let size = bytes
        .div_ceil(GIB)
        .checked_add(if system_disk { 8 } else { 0 })
        .context("Worker allocation overflow")?;
    Ok(size.max(15))
}

pub fn open_backup(backup: &Backup) -> Result<File> {
    let path = Path::new(&backup.path);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .context("Verified backup is unavailable; reconnect its original volume")?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "Backup is not a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1
                && metadata.uid() == unsafe { libc::geteuid() },
            "Backup must be private to the owner"
        );
    }
    anyhow::ensure!(
        crate::storage::volume_identity_file(&file)? == backup.volume_id,
        "The original backup volume has changed"
    );
    Ok(file)
}

pub fn secure_backup(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        // A selected folder can inherit broad ACLs. Restrict the empty backup
        // before streaming any data, using Windows' supported ACL utility.
        let system = std::path::PathBuf::from(
            std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("System32");
        let identity = std::process::Command::new(system.join("whoami.exe"))
            .args(["/user", "/fo", "csv", "/nh"])
            .output()?;
        anyhow::ensure!(
            identity.status.success(),
            "Cannot identify the backup owner"
        );
        let identity = String::from_utf8(identity.stdout)?;
        let sid = identity
            .trim()
            .split(',')
            .next_back()
            .context("No backup owner")?
            .trim()
            .trim_matches('"');
        anyhow::ensure!(
            sid.starts_with("S-1-")
                && sid
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S'),
            "Invalid Windows backup owner"
        );
        let result = std::process::Command::new(system.join("icacls.exe"))
            .arg(path)
            .args(["/inheritance:r", "/grant:r", &format!("*{sid}:(F)")])
            .output()?;
        anyhow::ensure!(
            result.status.success(),
            "Cannot make the temporary worker backup private"
        );
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

pub fn checksum(mut file: File) -> Result<(String, u64)> {
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .context("Backup size overflow")?;
    }
    Ok((format!("{:x}", hash.finalize()), size))
}

pub fn verified_backup(backup: &Backup) -> Result<File> {
    anyhow::ensure!(
        backup.verified && backup.sha256.is_some(),
        "No verified backup authorizes source deletion"
    );
    let (hash, size) = checksum(open_backup(backup)?)?;
    anyhow::ensure!(
        Some(&hash) == backup.sha256.as_ref() && size == backup.bytes,
        "Backup checksum failed; preserve the source images and verified recovery record"
    );
    open_backup(backup)
}
