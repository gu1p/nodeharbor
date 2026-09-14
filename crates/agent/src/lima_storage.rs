//! Owned file-backed Lima disks. The caller owns the durable maintenance journal
//! and worker ownership check; this adapter also refuses running-worker writes.
use crate::{storage::Location, Runner, VmProvider};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Debug)]
pub struct DiskPaths {
    pub directory: PathBuf,
    pub image: PathBuf,
    pub receipt: PathBuf,
    pub link: PathBuf,
}

fn validate_location(device: &str, location: &Location) -> Result<()> {
    uuid::Uuid::parse_str(device).context("Invalid storage owner identity")?;
    let id = location.id.as_bytes();
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 11
            && id[0].is_ascii_alphabetic()
            && id.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-'),
        "Invalid worker disk identifier"
    );
    anyhow::ensure!(
        !location.volume_id.is_empty(),
        "Missing storage volume identity"
    );
    anyhow::ensure!(
        (1..=1_048_576).contains(&location.allocation_gib),
        "Invalid worker disk allocation"
    );
    absolute_path(Path::new(&location.directory))
}

fn absolute_path(path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.is_absolute() && !path.components().any(|p| p == Component::ParentDir),
        "Worker storage requires an absolute path without parent traversal"
    );
    Ok(())
}

pub fn disk_paths(home: &Path, device: &str, location: &Location) -> Result<DiskPaths> {
    absolute_path(home)?;
    validate_location(device, location)?;
    let directory = Path::new(&location.directory)
        .join(".nodeharbor")
        .join(device)
        .join(&location.id);
    Ok(DiskPaths {
        image: directory.join("datadisk"),
        receipt: directory.join("receipt.json"),
        link: home.join("_disks").join(&location.id),
        directory,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiskReceipt {
    version: u32,
    device_id: String,
    disk_id: String,
    volume_id: String,
    allocation_gib: u64,
}

impl DiskReceipt {
    pub fn new(device: &str, location: &Location) -> Result<Self> {
        validate_location(device, location)?;
        Ok(Self {
            version: 1,
            device_id: device.into(),
            disk_id: location.id.clone(),
            volume_id: location.volume_id.clone(),
            allocation_gib: location.allocation_gib,
        })
    }
    pub fn verify(&self, device: &str, location: &Location) -> Result<()> {
        validate_location(device, location)?;
        anyhow::ensure!(
            self.version == 1
                && self.device_id == device
                && self.disk_id == location.id
                && self.volume_id == location.volume_id
                && self.allocation_gib == location.allocation_gib,
            "Disk ownership receipt does not match this owner, volume, or allocation"
        );
        Ok(())
    }
}

#[derive(Debug)]
pub struct DiskInspection {
    pub size_bytes: u64,
    pub allocated_bytes: u64,
    pub format: String,
    pub image: PathBuf,
}

pub struct LimaStorage {
    runner: Arc<dyn Runner>,
    home: PathBuf,
    device: String,
    volume_identity: Arc<VolumeIdentity>,
}

pub type VolumeIdentity = dyn Fn(&File) -> Result<String> + Send + Sync;

impl LimaStorage {
    pub fn new(runner: Arc<dyn Runner>, home: PathBuf, device: &str) -> Result<Self> {
        anyhow::ensure!(
            cfg!(unix),
            "File-backed worker disks require macOS or Linux"
        );
        anyhow::ensure!(
            runner.provider() == VmProvider::Lima,
            "This storage adapter requires Lima"
        );
        uuid::Uuid::parse_str(device)?;
        absolute_path(&home)?;
        Ok(Self {
            runner,
            home,
            device: device.into(),
            volume_identity: Arc::new(crate::storage::volume_identity_file),
        })
    }

    /// Inject a filesystem identity reader for deterministic runtime tests.
    /// Production callers use the native descriptor-based reader from `new`.
    pub fn with_volume_identity(mut self, reader: Arc<VolumeIdentity>) -> Self {
        self.volume_identity = reader;
        self
    }

    async fn command(&self, args: Vec<String>) -> Result<String> {
        let result = self.runner.run(&args, None, 300).await?;
        anyhow::ensure!(
            result.success,
            "Lima storage operation failed: {}",
            result.stderr
        );
        Ok(result.stdout)
    }

    async fn stopped(&self) -> Result<()> {
        let output = self
            .command(vec![
                "list".into(),
                "--json".into(),
                "--filter=.name == \"worker\"".into(),
            ])
            .await?;
        if !output.trim().is_empty() {
            let state: serde_json::Value = serde_json::from_str(output.trim())?;
            anyhow::ensure!(
                state["name"] == "worker" && state["status"] == "Stopped",
                "Stop the worker before changing its storage"
            );
        }
        Ok(())
    }

    fn lock(&self) -> Result<File> {
        private_directory(&self.home, true)?;
        let file = private_open(&self.home.join(".nodeharbor-storage.lock"), true)?;
        fs2::FileExt::try_lock_exclusive(&file)
            .context("Another storage operation is still finishing")?;
        Ok(file)
    }

    fn paths(&self, location: &Location) -> Result<DiskPaths> {
        disk_paths(&self.home, &self.device, location)
    }

    fn receipt(&self, path: &Path, location: &Location) -> Result<()> {
        let receipt: DiskReceipt = serde_json::from_reader(private_open(path, false)?)?;
        receipt.verify(&self.device, location)
    }

    fn backing(&self, location: &Location) -> Result<DiskPaths> {
        let paths = self.paths(location)?;
        for directory in [
            Path::new(&location.directory).join(".nodeharbor"),
            paths
                .directory
                .parent()
                .context("No private disk parent")?
                .to_owned(),
            paths.directory.clone(),
        ] {
            private_directory(&directory, false)?;
        }
        self.receipt(&paths.receipt, location)?;
        check_volume(
            &private_open(&paths.image, false)?,
            &location.volume_id,
            self.volume_identity.as_ref(),
        )?;
        Ok(paths)
    }

    fn linked(&self, location: &Location) -> Result<DiskPaths> {
        let paths = self.backing(location)?;
        verify_link(&paths.link, &paths.directory)?;
        Ok(paths)
    }

    /// Validate saved ownership, privacy, volume identity and registration before
    /// starting a worker. This only reads host files and never invokes Lima.
    pub fn validate(&self, location: &Location) -> Result<()> {
        self.linked(location).map(|_| ())
    }

    async fn runtime_disk(&self, location: &Location) -> Result<serde_json::Value> {
        let output = self
            .command(vec![
                "disk".into(),
                "list".into(),
                location.id.clone(),
                "--json".into(),
            ])
            .await?;
        let disk: serde_json::Value = serde_json::from_str(output.trim())
            .context("Lima did not report the configured disk")?;
        anyhow::ensure!(
            disk["name"] == location.id && matches!(disk["format"].as_str(), Some("raw" | "qcow2")),
            "Lima returned a different or unsupported disk"
        );
        anyhow::ensure!(
            disk["dir"].as_str().map(Path::new) == Some(self.paths(location)?.link.as_path()),
            "Lima reported an unexpected disk directory"
        );
        Ok(disk)
    }

    pub async fn inspect(&self, location: &Location) -> Result<DiskInspection> {
        let paths = self.linked(location)?;
        let disk = self.runtime_disk(location).await?;
        let size = disk["size"]
            .as_u64()
            .context("Lima did not report disk capacity")?;
        anyhow::ensure!(
            size == location.allocation_gib << 30,
            "The worker disk capacity differs from its saved allocation"
        );
        let metadata = private_open(&paths.image, false)?.metadata()?;
        let format = disk["format"]
            .as_str()
            .context("Missing disk format")?
            .to_owned();
        if format == "raw" {
            anyhow::ensure!(metadata.len() == size, "The raw worker disk is incomplete");
        }
        Ok(DiskInspection {
            size_bytes: size,
            allocated_bytes: allocated_bytes(&metadata),
            format,
            image: paths.image,
        })
    }

    pub async fn create(&self, location: &Location) -> Result<DiskInspection> {
        let lock = self.lock()?;
        self.stopped().await?;
        let paths = self.paths(location)?;
        let pending = self.home.join(format!(".create-{}.json", location.id));
        if std::fs::symlink_metadata(&paths.link).is_ok() && !pending.exists() {
            return self.inspect(location).await;
        }
        if pending.exists() {
            self.receipt(&pending, location)?;
            // The verified copy may have completed before a crash interrupted
            // replacement of the original, now-empty runtime directory.
            if self.backing(location).is_ok() {
                let missing = std::fs::symlink_metadata(&paths.link)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
                let empty = std::fs::symlink_metadata(&paths.link)
                    .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
                    && std::fs::read_dir(&paths.link)?.next().is_none();
                if missing || empty {
                    if empty {
                        private_directory(&paths.link, false)?;
                        std::fs::remove_dir(&paths.link)?;
                    }
                    install_link(&paths.link, &paths.directory)?;
                    sync_directory(paths.link.parent().context("No disk registry")?)?;
                    let inspection = self.inspect(location).await?;
                    std::fs::remove_file(pending)?;
                    return Ok(inspection);
                }
            }
        } else {
            anyhow::ensure!(
                std::fs::symlink_metadata(&paths.directory).is_err(),
                "The selected disk directory already exists; it has been left untouched"
            );
            anyhow::ensure!(
                std::fs::symlink_metadata(&paths.link).is_err(),
                "The runtime disk already exists; it has been left untouched"
            );
            write_receipt(&pending, &DiskReceipt::new(&self.device, location)?, None)?;
        }
        let target_parent = prepare_parent(location, &self.device, self.volume_identity.as_ref())?;
        private_directory(&self.home.join("_disks"), true)?;
        if std::fs::symlink_metadata(&paths.link).is_err() {
            self.command(vec![
                "disk".into(),
                "create".into(),
                location.id.clone(),
                format!("--size={}GiB", location.allocation_gib),
                "--format=raw".into(),
            ])
            .await?;
        }
        if std::fs::symlink_metadata(&paths.link)?
            .file_type()
            .is_symlink()
        {
            let result = self.inspect(location).await?;
            std::fs::remove_file(pending)?;
            return Ok(result);
        }
        private_directory(&paths.link, false)?;
        let disk = self.runtime_disk(location).await?;
        anyhow::ensure!(
            disk["size"].as_u64() == Some(location.allocation_gib << 30)
                && disk["instance"].as_str().unwrap_or("").is_empty(),
            "New disk has unexpected capacity or is attached to another instance"
        );
        let source = paths.link.join("datadisk");
        restrict_file(&source)?;
        let target = paths.directory.clone();
        let receipt = DiskReceipt::new(&self.device, location)?;
        let source_volume = (self.volume_identity)(&private_open(&source, false)?)?;
        let identity = self.volume_identity.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            let _target_parent = target_parent;
            stage_copy(
                &source,
                &target,
                &receipt,
                &source_volume,
                identity.as_ref(),
            )
        })
        .await??;
        let _lock = self.lock()?;
        self.stopped().await?;
        self.backing(location)?;
        anyhow::ensure!(
            std::fs::read_dir(&paths.link)?
                .all(|entry| entry.is_ok_and(|entry| entry.file_name() == "datadisk")),
            "Unexpected runtime disk files; original directory retained"
        );
        // This is the adapter's authenticated, detached, pending runtime disk.
        // The destination has already been synchronized and byte-verified.
        std::fs::remove_file(paths.link.join("datadisk"))?;
        std::fs::remove_dir(&paths.link)?;
        install_link(&paths.link, &paths.directory)?;
        sync_directory(paths.link.parent().context("No disk registry")?)?;
        std::fs::remove_file(pending)?;
        self.inspect(location).await
    }

    pub async fn grow(&self, old: &Location, new: &Location) -> Result<DiskInspection> {
        let _lock = self.lock()?;
        self.stopped().await?;
        same_disk(old, new)?;
        anyhow::ensure!(
            old.directory == new.directory
                && old.volume_id == new.volume_id
                && new.allocation_gib >= old.allocation_gib,
            "Disk growth cannot move or shrink existing storage"
        );
        if self.linked(new).is_ok() {
            return self.inspect(new).await;
        }
        let paths = self.linked(old)?;
        let image = private_open(&paths.image, false)?;
        check_volume(&image, &old.volume_id, self.volume_identity.as_ref())?;
        let disk = self.runtime_disk(old).await?;
        let old_size = old.allocation_gib << 30;
        let new_size = new.allocation_gib << 30;
        let size = disk["size"].as_u64().context("Missing disk size")?;
        anyhow::ensure!(
            size == old_size || size == new_size,
            "Unexpected disk size during growth recovery"
        );
        if size < new_size {
            self.command(vec![
                "disk".into(),
                "resize".into(),
                old.id.clone(),
                format!("--size={}GiB", new.allocation_gib),
            ])
            .await?;
        }
        private_open(&paths.image, false)?.sync_all()?;
        anyhow::ensure!(
            self.runtime_disk(new).await?["size"].as_u64() == Some(new_size),
            "Lima did not apply disk growth"
        );
        write_receipt(
            &paths.receipt,
            &DiskReceipt::new(&self.device, new)?,
            Some(self.volume_identity.as_ref()),
        )?;
        self.inspect(new).await
    }

    pub async fn stage_move(&self, old: &Location, new: &Location) -> Result<()> {
        let lock = self.lock()?;
        self.stopped().await?;
        same_disk(old, new)?;
        anyhow::ensure!(
            old.directory != new.directory && old.allocation_gib == new.allocation_gib,
            "Moving storage must preserve its disk allocation"
        );
        let source = self.linked(old)?.image;
        let target_parent = prepare_parent(new, &self.device, self.volume_identity.as_ref())?;
        let target = self.paths(new)?.directory;
        let receipt = DiskReceipt::new(&self.device, new)?;
        let source_volume = old.volume_id.clone();
        let identity = self.volume_identity.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            let _target_parent = target_parent;
            stage_copy(
                &source,
                &target,
                &receipt,
                &source_volume,
                identity.as_ref(),
            )
        })
        .await?
    }

    pub async fn commit_move(&self, old: &Location, new: &Location) -> Result<()> {
        let _lock = self.lock()?;
        self.stopped().await?;
        same_disk(old, new)?;
        anyhow::ensure!(
            old.allocation_gib == new.allocation_gib,
            "A storage move cannot change allocation"
        );
        let old_paths = self.backing(old)?;
        let new_paths = self.backing(new)?;
        if verify_link(&old_paths.link, &new_paths.directory).is_ok() {
            return Ok(());
        }
        verify_link(&old_paths.link, &old_paths.directory)?;
        let pending = old_paths.link.with_extension("pending-link");
        if std::fs::symlink_metadata(&pending).is_ok() {
            verify_link(&pending, &new_paths.directory)?;
        } else {
            install_link(&pending, &new_paths.directory)?;
        }
        std::fs::rename(&pending, &old_paths.link)?;
        sync_directory(old_paths.link.parent().context("No disk registry")?)?;
        Ok(())
    }

    /// Explicit post-qualification cleanup. Never removes a selected parent or
    /// a directory still registered with Lima, and rejects unexpected files.
    /// Remove a retired, receipt-owned image only with its original volume
    /// present and no worker VM. Retries tolerate completed deletion, never a
    /// missing mount masquerading as an empty directory.
    pub async fn remove_owned(&self, location: &Location) -> Result<()> {
        let _lock = self.lock()?;
        self.stopped().await?;
        let paths = self.paths(location)?;
        let ancestor = paths
            .directory
            .ancestors()
            .find(|p| p.exists())
            .context("Original disk volume is unavailable")?;
        check_volume(
            &File::open(ancestor)?,
            &location.volume_id,
            self.volume_identity.as_ref(),
        )?;
        if paths.directory.exists() {
            private_directory(&paths.directory, false)?;
            anyhow::ensure!(
                std::fs::read_dir(&paths.directory)?.all(|entry| entry.is_ok_and(|e| matches!(
                    e.file_name().to_str(),
                    Some("datadisk" | "receipt.json")
                ))),
                "Unexpected files in managed disk; preserved"
            );
            if paths.image.exists() {
                self.receipt(&paths.receipt, location)?;
                check_volume(
                    &private_open(&paths.image, false)?,
                    &location.volume_id,
                    self.volume_identity.as_ref(),
                )?;
                if std::fs::symlink_metadata(&paths.link).is_ok() {
                    verify_link(&paths.link, &paths.directory)?;
                    self.command(vec!["disk".into(), "delete".into(), location.id.clone()])
                        .await?;
                    anyhow::ensure!(std::fs::symlink_metadata(&paths.link).is_err(), "The runtime retained this disk because another instance still references it; image preserved");
                }
                std::fs::remove_file(&paths.image)?;
                sync_directory(&paths.directory)?;
            }
            if paths.receipt.exists() {
                self.receipt(&paths.receipt, location)?;
                std::fs::remove_file(&paths.receipt)?;
                sync_directory(&paths.directory)?;
            }
            std::fs::remove_dir(&paths.directory)?;
        }
        if let Ok(link) = std::fs::read_link(&paths.link) {
            anyhow::ensure!(
                link == paths.directory,
                "Retired disk registration changed; preserved"
            );
            std::fs::remove_file(&paths.link)?;
            sync_directory(paths.link.parent().context("No disk registration parent")?)?;
        }
        Ok(())
    }

    pub async fn remove_retained(&self, location: &Location) -> Result<()> {
        let _lock = self.lock()?;
        self.stopped().await?;
        let paths = self.paths(location)?;
        let registered = std::fs::read_link(&paths.link)
            .context("Cannot verify the active worker disk before cleanup")?;
        anyhow::ensure!(
            registered != paths.directory && registered.is_dir(),
            "Cannot remove a registered worker disk"
        );
        match std::fs::symlink_metadata(&paths.directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A crash can follow removing the last directory but precede
                // clearing the retained-copy record. Never mistake an absent
                // mounted volume for a successfully removed directory.
                let ancestor = paths
                    .directory
                    .ancestors()
                    .find(|path| path.exists())
                    .context("Original storage volume is unavailable")?;
                check_volume(
                    &File::open(ancestor)?,
                    &location.volume_id,
                    self.volume_identity.as_ref(),
                )?;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
            Ok(_) => private_directory(&paths.directory, false)?,
        }
        anyhow::ensure!(
            registered.canonicalize()? != paths.directory.canonicalize()?,
            "Cannot remove a registered worker disk"
        );
        check_volume(
            &File::open(&paths.directory)?,
            &location.volume_id,
            self.volume_identity.as_ref(),
        )?;
        anyhow::ensure!(
            std::fs::read_dir(&paths.directory)?.all(|entry| entry.is_ok_and(|entry| matches!(
                entry.file_name().to_str(),
                Some("datadisk" | "receipt.json")
            ))),
            "Unexpected files in retained disk directory; files preserved"
        );
        let has_receipt = std::fs::symlink_metadata(&paths.receipt).is_ok();
        let has_image = std::fs::symlink_metadata(&paths.image).is_ok();
        anyhow::ensure!(
            !has_image || has_receipt,
            "Retained image has no ownership receipt; files preserved"
        );
        if has_receipt {
            self.receipt(&paths.receipt, location)?;
        }
        if has_image {
            check_volume(
                &private_open(&paths.image, false)?,
                &location.volume_id,
                self.volume_identity.as_ref(),
            )?;
            std::fs::remove_file(&paths.image)?;
            sync_directory(&paths.directory)?;
        }
        if has_receipt {
            std::fs::remove_file(&paths.receipt)?;
            sync_directory(&paths.directory)?;
        }
        std::fs::remove_dir(&paths.directory)?;
        sync_directory(
            paths
                .directory
                .parent()
                .context("No retained disk parent")?,
        )
    }
}

fn same_disk(old: &Location, new: &Location) -> Result<()> {
    anyhow::ensure!(
        old.id == new.id,
        "A storage change must preserve its disk identity"
    );
    Ok(())
}

fn prepare_parent(location: &Location, device: &str, identity: &VolumeIdentity) -> Result<File> {
    let parent = Path::new(&location.directory);
    let existing = if parent.exists() {
        parent
    } else {
        parent.parent().context("Storage parent is unavailable")?
    };
    let anchor = File::open(existing).context("Storage volume is unavailable")?;
    check_volume(&anchor, &location.volume_id, identity)?;
    if !parent.exists() {
        // Only create the explicitly selected final directory, never mount ancestors.
        anyhow::ensure!(
            parent.parent().is_some_and(Path::is_dir),
            "Storage volume is unavailable"
        );
        private_directory(parent, true)?;
    }
    anyhow::ensure!(parent.is_dir(), "Storage parent is unavailable");
    private_directory(&parent.join(".nodeharbor"), true)?;
    private_directory(&parent.join(".nodeharbor").join(device), true)?;
    let anchor = File::open(parent)?;
    check_volume(&anchor, &location.volume_id, identity)?;
    Ok(anchor)
}

fn private_directory(path: &Path, create: bool) -> Result<()> {
    if create && std::fs::symlink_metadata(path).is_err() {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .context("Cannot create private worker storage directory")?;
    }
    let metadata =
        std::fs::symlink_metadata(path).context("Worker storage directory is unavailable")?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Worker storage directory was replaced"
    );
    private_metadata(&metadata)
}

fn private_metadata(metadata: &std::fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments and no memory preconditions.
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "Worker storage must be private to its owner"
        );
    }
    Ok(())
}

fn private_open(path: &Path, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(create).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .context("Worker storage file is unavailable")?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "Worker storage must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.nlink() == 1,
            "Worker storage cannot be a hard link"
        );
    }
    private_metadata(&metadata)?;
    Ok(file)
}

fn restrict_file(path: &Path) -> Result<()> {
    anyhow::ensure!(
        std::fs::symlink_metadata(path)?.is_file(),
        "Runtime created a non-regular disk image"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    private_open(path, false)?;
    Ok(())
}

fn write_receipt(
    path: &Path,
    receipt: &DiskReceipt,
    identity: Option<&VolumeIdentity>,
) -> Result<()> {
    let parent = path.parent().context("No receipt directory")?;
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    if let Some(identity) = identity {
        check_volume(pending.as_file(), &receipt.volume_id, identity)?;
    }
    serde_json::to_writer(pending.as_file_mut(), receipt)?;
    pending.as_file().sync_all()?;
    pending.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)
}

fn verify_link(link: &Path, expected: &Path) -> Result<()> {
    anyhow::ensure!(
        std::fs::read_link(link).context("Worker disk registration is unavailable")? == expected,
        "Worker disk points to an unexpected storage location"
    );
    anyhow::ensure!(
        expected.is_dir(),
        "The original storage volume is unavailable; storage will not be redirected"
    );
    Ok(())
}

fn install_link(link: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (link, target);
        anyhow::bail!("Additional worker disk registration is unsupported on this host")
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn allocated_bytes(metadata: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.blocks().saturating_mul(512)
    }
    #[cfg(not(unix))]
    metadata.len()
}

fn check_volume(file: &File, expected: &str, identity: &VolumeIdentity) -> Result<()> {
    anyhow::ensure!(
        identity(file)? == expected,
        "The original storage volume is unavailable or changed; storage will not be redirected"
    );
    Ok(())
}

fn stage_copy(
    source: &Path,
    target: &Path,
    receipt: &DiskReceipt,
    source_volume: &str,
    identity: &VolumeIdentity,
) -> Result<()> {
    let parent = target.parent().context("No destination parent")?;
    if target.exists() {
        private_directory(target, false)?;
        let existing: DiskReceipt =
            serde_json::from_reader(private_open(&target.join("receipt.json"), false)?)?;
        anyhow::ensure!(
            serde_json::to_value(existing)? == serde_json::to_value(receipt)?,
            "Destination disk already belongs to different storage"
        );
        return verify_sparse_copy(
            source,
            &target.join("datadisk"),
            source_volume,
            &receipt.volume_id,
            identity,
        );
    }
    let pending = parent.join(format!(".{}.pending", receipt.disk_id));
    if pending.exists() {
        private_directory(&pending, false)?;
        check_volume(&File::open(&pending)?, &receipt.volume_id, identity)?;
        let existing: DiskReceipt =
            serde_json::from_reader(private_open(&pending.join("receipt.json"), false)?)?;
        anyhow::ensure!(
            serde_json::to_value(existing)? == serde_json::to_value(receipt)?,
            "Pending disk belongs to different storage"
        );
        anyhow::ensure!(
            std::fs::read_dir(&pending)?.all(|entry| entry.is_ok_and(|entry| matches!(
                entry.file_name().to_str(),
                Some("datadisk" | "receipt.json")
            ))),
            "Unexpected files in pending disk directory"
        );
        if pending.join("datadisk").exists() {
            check_volume(
                &private_open(&pending.join("datadisk"), false)?,
                &receipt.volume_id,
                identity,
            )?;
            std::fs::remove_file(pending.join("datadisk"))?;
        }
    } else {
        private_directory(&pending, true)?;
        write_receipt(&pending.join("receipt.json"), receipt, Some(identity))?;
    }
    sparse_copy(
        source,
        &pending.join("datadisk"),
        source_volume,
        &receipt.volume_id,
        identity,
    )?;
    verify_sparse_copy(
        source,
        &pending.join("datadisk"),
        source_volume,
        &receipt.volume_id,
        identity,
    )?;
    sync_directory(&pending)?;
    std::fs::rename(&pending, target)?;
    sync_directory(parent)
}

#[cfg(unix)]
fn extents(file: &File) -> Result<Vec<(u64, u64)>> {
    use std::os::fd::AsRawFd;
    let length = file.metadata()?.len();
    let mut offset = 0;
    let mut result = Vec::new();
    while offset < length {
        // SAFETY: a live file descriptor and checked, nonnegative offsets.
        let data = unsafe { libc::lseek(file.as_raw_fd(), offset as libc::off_t, libc::SEEK_DATA) };
        if data < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENXIO) {
                break;
            }
            return Err(error)
                .context("Storage filesystem does not support verified sparse copying");
        }
        // SAFETY: SEEK_DATA returned a valid offset on this live descriptor.
        let hole = unsafe { libc::lseek(file.as_raw_fd(), data, libc::SEEK_HOLE) };
        anyhow::ensure!(hole > data, "Cannot determine sparse disk extent");
        let end = (hole as u64).min(length);
        result.push((data as u64, end));
        offset = end;
    }
    Ok(result)
}

#[cfg(not(unix))]
fn extents(_file: &File) -> Result<Vec<(u64, u64)>> {
    anyhow::bail!("Verified sparse copying requires macOS or Linux")
}

fn sparse_copy(
    source: &Path,
    destination: &Path,
    source_volume: &str,
    target_volume: &str,
    identity: &VolumeIdentity,
) -> Result<()> {
    let mut input = private_open(source, false)?;
    check_volume(&input, source_volume, identity)?;
    let before = input.metadata()?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut output = options.open(destination)?;
    check_volume(&output, target_volume, identity)?;
    output.set_len(before.len())?;
    for (start, end) in extents(&input)? {
        input.seek(SeekFrom::Start(start))?;
        output.seek(SeekFrom::Start(start))?;
        let copied = std::io::copy(&mut (&mut input).take(end - start), &mut output)?;
        anyhow::ensure!(
            copied == end - start,
            "The source disk changed during migration"
        );
    }
    output.flush()?;
    output.sync_all()?;
    let after = input.metadata()?;
    anyhow::ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "The source disk changed during migration"
    );
    Ok(())
}

fn verify_sparse_copy(
    source: &Path,
    destination: &Path,
    source_volume: &str,
    target_volume: &str,
    identity: &VolumeIdentity,
) -> Result<()> {
    let mut input = private_open(source, false)?;
    let mut output = private_open(destination, false)?;
    check_volume(&input, source_volume, identity)?;
    check_volume(&output, target_volume, identity)?;
    anyhow::ensure!(
        input.metadata()?.len() == output.metadata()?.len(),
        "Copied disk has a different length"
    );
    // Compare the union of both sets of allocated extents, including unexpected
    // destination data in a source hole. Unallocated regions in both are zero.
    let mut ranges = extents(&input)?;
    ranges.extend(extents(&output)?);
    ranges.sort_unstable();
    let mut end_verified = 0;
    let mut a = vec![0; 1024 * 1024];
    let mut b = vec![0; a.len()];
    for (start, end) in ranges {
        let mut offset = start.max(end_verified);
        input.seek(SeekFrom::Start(offset))?;
        output.seek(SeekFrom::Start(offset))?;
        while offset < end {
            let count = (end - offset).min(a.len() as u64) as usize;
            input.read_exact(&mut a[..count])?;
            output.read_exact(&mut b[..count])?;
            anyhow::ensure!(
                a[..count] == b[..count],
                "Copied disk bytes do not match the original"
            );
            offset += count as u64;
        }
        end_verified = end_verified.max(end);
    }
    Ok(())
}
