//! Private runtime directories and verified, interruptible copies. Callers must
//! verify the VM creation receipt and stop the source VM before copying it.
use crate::{storage::Location, storage_layout::Layout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relocation {
    pub source: String,
    pub source_volume_id: String,
    pub target: Layout,
    pub switched: bool,
}

fn check_volume(path: &Path, expected: &str) -> Result<()> {
    anyhow::ensure!(
        crate::storage::volume_identity_file(&File::open(path)?)? == expected,
        "The selected VM volume disappeared or was replaced; storage will not be redirected"
    );
    Ok(())
}

fn owned_directory(path: &Path) -> Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        m.is_dir() && !m.file_type().is_symlink(),
        "The VM directory was replaced"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "The VM directory must be private and owned by the current user"
        );
    }
    Ok(())
}

fn mkdir(path: &Path) -> Result<()> {
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => owned_directory(path),
        Err(e) => Err(e.into()),
    }
}

fn receipt(layout: &Layout, owner: &str) -> Result<serde_json::Value> {
    uuid::Uuid::parse_str(owner)?;
    Ok(serde_json::json!({"version":1,"deviceId":owner,"volumeId":layout.volume_id}))
}

pub(crate) fn validate_home(layout: &Layout, owner: &str) -> Result<()> {
    let home = layout.home();
    owned_directory(&home)?;
    check_volume(&home, &layout.volume_id)?;
    let file = open_read(&home.join(".nodeharbor-runtime.json"))?;
    let saved: serde_json::Value = serde_json::from_reader(file)?;
    anyhow::ensure!(
        saved == receipt(layout, owner)?,
        "The VM directory belongs to a different owner or volume"
    );
    Ok(())
}

pub(crate) fn prepare_home(layout: &Layout, owner: &str) -> Result<()> {
    let home = layout.home();
    let folder = home.parent().context("Missing VM storage folder")?;
    // Only the final selected folder can be new. Never recreate a missing mount.
    let anchor = if folder.exists() {
        folder
    } else {
        folder.parent().context("Missing selected volume")?
    };
    check_volume(anchor, &layout.volume_id)?;
    if !folder.exists() {
        mkdir(folder)?;
    }
    check_volume(folder, &layout.volume_id)?;
    if home.join(".nodeharbor-runtime.json").exists() {
        return validate_home(layout, owner);
    }
    mkdir(&home)?;
    anyhow::ensure!(
        std::fs::read_dir(&home)?.next().is_none(),
        "The VM destination already contains files without a matching ownership receipt"
    );
    check_volume(&home, &layout.volume_id)?;
    let mut pending = tempfile::NamedTempFile::new_in(&home)?;
    serde_json::to_writer(&mut pending, &receipt(layout, owner)?)?;
    pending.as_file().sync_all()?;
    pending.persist_noclobber(home.join(".nodeharbor-runtime.json"))?;
    File::open(&home)?.sync_all()?;
    validate_home(layout, owner)
}

fn open_read(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let m = file.metadata()?;
    anyhow::ensure!(m.is_file(), "Unexpected VM runtime file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            m.uid() == unsafe { libc::geteuid() },
            "The VM runtime file belongs to another user"
        );
    }
    Ok(file)
}

/// Check the real image, not just Lima's requested disk size. QEMU's shared
/// read-only inspection is also usable during initial guest qualification.
pub(crate) async fn verify_boot_image(layout: &Layout, owner: &str) -> Result<()> {
    validate_home(layout, owner)?;
    let path = layout.home().join("worker/disk");
    let mut file = open_read(&path).context("The selected drive has no owned VM system image")?;
    anyhow::ensure!(
        crate::storage::volume_identity_file(&file)? == layout.volume_id,
        "The VM system image is not on the selected volume"
    );
    let mut magic = [0; 4];
    file.read_exact(&mut magic)?;
    let size = if magic == *b"QFI\xfb" {
        let mut command = tokio::process::Command::new("qemu-img");
        command
            .args(["info", "--force-share", "--output=json"])
            .arg(&path);
        let output = crate::process::run_command(
            command,
            None,
            30,
            None,
            crate::process::OutputFormat::Lines,
        )
        .await?;
        anyhow::ensure!(
            output.success,
            "Cannot verify the VM system image: {}",
            output.stderr
        );
        let info: serde_json::Value = serde_json::from_str(&output.stdout)?;
        anyhow::ensure!(info["format"] == "qcow2" && info.get("backing-filename").is_none(),
            "The system image depends on another image; preserve the original VM until its backing storage can be verified");
        info["virtual-size"]
            .as_u64()
            .context("Missing VM system image capacity")?
    } else {
        file.metadata()?.len()
    };
    anyhow::ensure!(
        Some(size) == layout.system_gib.checked_mul(1 << 30),
        "The actual VM system image capacity differs from the reviewed allocation"
    );
    validate_home(layout, owner)
}

fn verify_copied_image(
    input: &mut File,
    existing: &mut File,
    canceled: &dyn Fn() -> Result<()>,
) -> Result<()> {
    anyhow::ensure!(
        input.metadata()?.len() == existing.metadata()?.len(),
        "The previously copied VM image changed; the original has been preserved"
    );
    let mut ranges = crate::lima_storage::extents(input)?;
    ranges.extend(crate::lima_storage::extents(existing)?);
    ranges.sort_unstable();
    ranges.dedup();
    let mut source = vec![0; 1 << 20];
    let mut destination = vec![0; source.len()];
    for (start, end) in ranges {
        input.seek(SeekFrom::Start(start))?;
        existing.seek(SeekFrom::Start(start))?;
        let mut at = start;
        while at < end {
            canceled()?;
            let count = (end - at).min(source.len() as u64) as usize;
            input.read_exact(&mut source[..count])?;
            existing.read_exact(&mut destination[..count])?;
            anyhow::ensure!(
                source[..count] == destination[..count],
                "The previously copied VM image changed; the original has been preserved"
            );
            at += count as u64;
        }
    }
    Ok(())
}

fn copy_file(
    source: &Path,
    destination: &Path,
    source_volume: &str,
    target_volume: &str,
    canceled: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let mut input = open_read(source)?;
    anyhow::ensure!(
        crate::storage::volume_identity_file(&input)? == source_volume,
        "The source VM volume changed"
    );
    let before = input.metadata()?;
    if destination.exists()
        && destination
            .file_name()
            .is_some_and(|name| name == "disk" || name == "diffdisk")
    {
        let mut existing = open_read(destination)?;
        anyhow::ensure!(
            crate::storage::volume_identity_file(&existing)? == target_volume,
            "The copied VM volume changed"
        );
        verify_copied_image(&mut input, &mut existing, canceled)?;
        anyhow::ensure!(
            before.modified()? == input.metadata()?.modified()?,
            "The original VM changed while verifying its copy"
        );
        return Ok(());
    }
    let parent = destination.parent().context("Missing VM file directory")?;
    check_volume(parent, target_volume)?;
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    anyhow::ensure!(
        crate::storage::volume_identity_file(output.as_file())? == target_volume,
        "The VM destination volume changed"
    );
    output.as_file().set_len(before.len())?;
    let ranges = crate::lima_storage::extents(&input)?;
    let mut buffer = vec![0; 1024 * 1024];
    for &(start, end) in &ranges {
        input.seek(SeekFrom::Start(start))?;
        output.seek(SeekFrom::Start(start))?;
        let mut offset = start;
        while offset < end {
            canceled()?;
            let count = (end - offset).min(buffer.len() as u64) as usize;
            input.read_exact(&mut buffer[..count])?;
            output.write_all(&buffer[..count])?;
            offset += count as u64;
        }
    }
    output.as_file().sync_all()?;
    let mut actual = vec![0; buffer.len()];
    for (start, end) in ranges {
        input.seek(SeekFrom::Start(start))?;
        output.seek(SeekFrom::Start(start))?;
        let mut offset = start;
        while offset < end {
            canceled()?;
            let count = (end - offset).min(buffer.len() as u64) as usize;
            input.read_exact(&mut buffer[..count])?;
            output.read_exact(&mut actual[..count])?;
            anyhow::ensure!(
                buffer[..count] == actual[..count],
                "Copied VM bytes differ from the source"
            );
            offset += count as u64;
        }
    }
    let after = input.metadata()?;
    anyhow::ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "The source VM changed while copying"
    );
    canceled()?;
    check_volume(parent, target_volume)?;
    output.persist(destination)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn source_files(source: &Path) -> Result<serde_json::Value> {
    fn visit(path: &Path) -> Result<serde_json::Value> {
        let mut files = std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".pid") || name.ends_with(".sock") || name.ends_with(".lock") {
                continue;
            }
            let m = std::fs::symlink_metadata(entry.path())?;
            let value = if m.is_dir() {
                visit(&entry.path())?
            } else if m.file_type().is_symlink() {
                serde_json::json!({"link":std::fs::read_link(entry.path())?})
            } else {
                #[cfg(unix)]
                let inode = {
                    use std::os::unix::fs::MetadataExt;
                    m.ino()
                };
                #[cfg(not(unix))]
                let inode = 0;
                serde_json::json!({"size":m.len(),"inode":inode,"modified":m.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos().to_string()})
            };
            files.insert(name, value);
        }
        Ok(serde_json::to_value(files)?)
    }
    visit(&source.join("worker"))
}

/// Caller has verified the creation receipt and stopped this owned VM. A
/// retirement receipt allows deferred cleanup when its original volume returns.
pub(crate) fn mark_retired_home(layout: &Layout, owner: &str) -> Result<()> {
    let home = layout.home();
    owned_directory(&home)?;
    check_volume(&home, &layout.volume_id)?;
    if home.join(".nodeharbor-runtime.json").exists() {
        return validate_home(layout, owner);
    }
    let mut file = tempfile::NamedTempFile::new_in(&home)?;
    serde_json::to_writer(&mut file, &receipt(layout, owner)?)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(home.join(".nodeharbor-runtime.json"))?;
    File::open(home)?.sync_all()?;
    Ok(())
}

pub(crate) fn remove_retired_home(layout: &Layout, owner: &str) -> Result<()> {
    validate_home(layout, owner)?;
    clear_stopped_home(&layout.home())
}

pub(crate) fn verify_source_receipt(source: &Path, target: &Layout, owner: &str) -> Result<()> {
    owned_directory(source)?;
    let saved: serde_json::Value = serde_json::from_reader(open_read(
        &source.join(".nodeharbor-relocation-source.json"),
    )?)?;
    anyhow::ensure!(
        saved["deviceId"] == owner
            && saved["target"] == serde_json::to_value(target)?
            && saved["volumeId"] == crate::storage::volume_identity(source)?,
        "The original VM volume or owner changed; its files have been preserved"
    );
    Ok(())
}

pub(crate) fn verify_source(source: &Path, target: &Layout, owner: &str) -> Result<()> {
    verify_source_receipt(source, target, owner)?;
    let saved: serde_json::Value = serde_json::from_reader(open_read(
        &source.join(".nodeharbor-relocation-source.json"),
    )?)?;
    anyhow::ensure!(
        saved["files"] == source_files(source)?,
        "The original VM changed after relocation; its files have been preserved"
    );
    Ok(())
}

pub(crate) fn remove_source_home(source: &Path, target: &Layout, owner: &str) -> Result<()> {
    verify_source_receipt(source, target, owner)?;
    clear_stopped_home(source)
}

fn clear_stopped_home(source: &Path) -> Result<()> {
    anyhow::ensure!(
        !source.join("worker").exists(),
        "Remove the original VM through Lima before cleaning up its directory"
    );
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() {
            anyhow::ensure!(
                ["_disks", "_networks", "_config", "_tmp", "_cache"]
                    .iter()
                    .any(|name| entry.file_name() == *name),
                "An unexpected VM directory remains; original runtime cleanup was stopped"
            );
        }
    }
    std::fs::remove_dir_all(source)?;
    Ok(())
}

pub(crate) fn copy_runtime(
    source: &Path,
    target: &Layout,
    owner: &str,
    disks: &[Location],
    canceled: &dyn Fn() -> Result<()>,
) -> Result<()> {
    canceled()?;
    prepare_home(target, owner)?;
    let source_volume = crate::storage::volume_identity(source)?;
    let marker = source.join(".nodeharbor-relocation-source.json");
    if marker.exists() {
        verify_source(source, target, owner)?;
    } else {
        let mut file = tempfile::NamedTempFile::new_in(source)?;
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({"deviceId":owner,"target":target,"volumeId":source_volume,"files":source_files(source)?}),
        )?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&marker)?;
        File::open(source)?.sync_all()?;
    }
    let home = target.home();
    anyhow::ensure!(
        source != home && !home.starts_with(source) && !source.starts_with(&home),
        "VM runtime directories cannot overlap"
    );
    let allowed: Vec<_> = disks
        .iter()
        .map(|l| crate::lima_storage::disk_paths(source, owner, l).map(|p| p.directory))
        .collect::<Result<_>>()?;
    fn visit(
        source: &Path,
        home: &Path,
        relative: &Path,
        source_volume: &str,
        target: &Layout,
        allowed: &[std::path::PathBuf],
        canceled: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(source.join(relative))? {
            canceled()?;
            let entry = entry?;
            let name = entry.file_name();
            let text = name.to_string_lossy();
            if text == "_networks"
                || text == "_tmp"
                || text == "_cache"
                || text.starts_with(".boot-")
                || text == ".nodeharbor-copy-worker"
                || text == ".nodeharbor-relocation-source.json"
                || text == ".nodeharbor-runtime.json"
                || text.ends_with(".pid")
                || text.ends_with(".sock")
                || text.ends_with(".lock")
            {
                continue;
            }
            let rel = relative.join(&name);
            let from = source.join(&rel);
            let to = if !home.join("worker").exists() && rel.starts_with("worker") {
                home.join(".nodeharbor-copy-worker")
                    .join(rel.strip_prefix("worker")?)
            } else {
                home.join(&rel)
            };
            let metadata = std::fs::symlink_metadata(&from)?;
            if metadata.file_type().is_symlink() {
                let link = std::fs::read_link(&from)?;
                let resolved = from
                    .canonicalize()
                    .context("The original VM has an unavailable link")?;
                anyhow::ensure!(
                    resolved.starts_with(source) || allowed.contains(&resolved),
                    "The VM contains an unowned external link"
                );
                let link = if link.is_absolute() && link.starts_with(source) {
                    home.join(link.strip_prefix(source)?)
                } else {
                    link
                };
                #[cfg(not(unix))]
                {
                    let _ = link;
                    anyhow::bail!("Lima VM relocation requires macOS or Linux");
                }
                #[cfg(unix)]
                {
                    if std::fs::symlink_metadata(&to).is_ok() {
                        anyhow::ensure!(
                            std::fs::read_link(&to)? == link,
                            "The destination VM link changed"
                        );
                    } else {
                        std::os::unix::fs::symlink(link, &to)?;
                    }
                }
            } else if metadata.is_dir() {
                mkdir(&to)?;
                check_volume(&to, &target.volume_id)?;
                visit(source, home, &rel, source_volume, target, allowed, canceled)?;
            } else if metadata.is_file() {
                copy_file(&from, &to, source_volume, &target.volume_id, canceled)?;
            } else {
                anyhow::bail!("Unexpected special file in the stopped VM runtime");
            }
        }
        Ok(())
    }
    visit(
        source,
        &home,
        Path::new(""),
        &source_volume,
        target,
        &allowed,
        canceled,
    )?;
    canceled()?;
    File::open(&home)?.sync_all()?;
    verify_source(source, target, owner)?;
    let staged = home.join(".nodeharbor-copy-worker");
    if staged.exists() {
        anyhow::ensure!(
            !home.join("worker").exists(),
            "The VM destination changed during copying"
        );
        std::fs::rename(staged, home.join("worker"))?;
        File::open(&home)?.sync_all()?;
    }
    validate_home(target, owner)
}

pub(crate) struct SelectedRuntimeRunner {
    pub inner: std::sync::Arc<dyn crate::Runner>,
    pub layout: Layout,
    pub owner: String,
}
#[async_trait::async_trait]
impl crate::Runner for SelectedRuntimeRunner {
    fn provider(&self) -> crate::VmProvider {
        crate::VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        input: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<crate::CommandOutput> {
        if let Err(error) = validate_home(&self.layout, &self.owner) {
            if args.first().is_some_and(|a| a == "list") {
                return Ok(crate::CommandOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            return Err(error)
                .context("The selected VM drive is unavailable; reconnect its original volume");
        }
        self.inner.run(args, input, timeout).await
    }
    async fn run_with_progress(
        &self,
        args: &[String],
        input: Option<Vec<u8>>,
        timeout: u64,
        progress: crate::ProgressSink,
    ) -> Result<crate::CommandOutput> {
        validate_home(&self.layout, &self.owner)
            .context("The selected VM drive is unavailable; reconnect its original volume")?;
        self.inner
            .run_with_progress(args, input, timeout, progress)
            .await
    }
    async fn stream(
        &self,
        args: &[String],
        input: Option<File>,
        output: Option<File>,
        limit: u64,
    ) -> Result<()> {
        validate_home(&self.layout, &self.owner)
            .context("The selected VM drive is unavailable; reconnect its original volume")?;
        self.inner.stream(args, input, output, limit).await
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    const OWNER: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";

    fn target(root: &std::path::Path) -> crate::storage_layout::Layout {
        crate::storage_layout::Layout {
            version: 1,
            system_location_id: "diskone".into(),
            volume_id: crate::storage::volume_identity(root).unwrap(),
            runtime_directory: root.join(".nh9511182e").to_string_lossy().into(),
            system_gib: 16,
        }
    }

    #[test]
    fn runtime_directory_requires_its_original_volume_and_owner() {
        let dir = tempfile::tempdir().unwrap();
        let mut layout = target(dir.path());
        layout.volume_id = "wrong-volume".into();
        assert!(prepare_home(&layout, OWNER).is_err());
        assert!(!layout.home().exists());
        layout.volume_id = crate::storage::volume_identity(dir.path()).unwrap();
        prepare_home(&layout, OWNER).unwrap();
        prepare_home(&layout, OWNER).unwrap();
        assert!(prepare_home(&layout, "af9f3259-8fdf-42b9-a3b4-1c53759bb2bb").is_err());
    }

    #[test]
    fn relocating_stopped_runtime_preserves_sparse_bytes_and_cancellation_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("old");
        mkdir(&source).unwrap();
        let worker = source.join("worker");
        std::fs::create_dir(&worker).unwrap();
        let mut file = std::fs::File::create(worker.join("disk")).unwrap();
        file.set_len(1 << 30).unwrap();
        file.seek(SeekFrom::Start((1 << 30) - 5)).unwrap();
        file.write_all(b"proof").unwrap();
        file.sync_all().unwrap();
        std::fs::write(worker.join("ha.pid"), "1234").unwrap();
        let layout = target(dir.path());
        assert!(
            copy_runtime(&source, &layout, OWNER, &[], &|| anyhow::bail!(
                "Paused by owner"
            ))
            .is_err()
        );
        assert!(worker.join("disk").exists());
        copy_runtime(&source, &layout, OWNER, &[], &|| Ok(())).unwrap();
        copy_runtime(&source, &layout, OWNER, &[], &|| Ok(())).unwrap();
        let mut copied = std::fs::File::open(layout.home().join("worker/disk")).unwrap();
        copied.seek(SeekFrom::Start((1 << 30) - 5)).unwrap();
        let mut bytes = [0; 5];
        copied.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"proof");
        assert!(!layout.home().join("worker/ha.pid").exists());
        use std::os::unix::fs::MetadataExt;
        assert!(copied.metadata().unwrap().blocks() * 512 < 1 << 20);
        assert!(worker.join("disk").exists());
        verify_source(&source, &layout, OWNER).unwrap();
        std::fs::write(worker.join("unexpected"), b"another instance").unwrap();
        assert!(verify_source(&source, &layout, OWNER).is_err());
    }
}

#[cfg(all(test, unix))]
mod verification_tests {
    use super::*;
    #[tokio::test]
    async fn system_image_proof_rejects_missing_replaced_and_wrong_size_disks() {
        let root = tempfile::tempdir().unwrap();
        let owner = "9511182e-9c48-4d20-a15b-1da8bb441386";
        let layout = Layout {
            version: 1,
            system_location_id: "one".into(),
            volume_id: crate::storage::volume_identity(root.path()).unwrap(),
            runtime_directory: root.path().join(".nhtest").to_string_lossy().into(),
            system_gib: 16,
        };
        prepare_home(&layout, owner).unwrap();
        assert!(verify_boot_image(&layout, owner).await.is_err());
        std::fs::create_dir(layout.home().join("worker")).unwrap();
        let image = layout.home().join("worker/disk");
        let file = File::create(&image).unwrap();
        file.set_len(15 << 30).unwrap();
        assert!(verify_boot_image(&layout, owner).await.is_err());
        file.set_len(16 << 30).unwrap();
        verify_boot_image(&layout, owner).await.unwrap();
        std::fs::rename(&image, root.path().join("other")).unwrap();
        std::os::unix::fs::symlink(root.path().join("other"), &image).unwrap();
        assert!(verify_boot_image(&layout, owner).await.is_err());
    }
}

#[cfg(all(test, unix))]
mod unavailable_runtime_tests {
    use super::*;
    use crate::{CommandOutput, Runner};
    use std::sync::Arc;
    struct NeverRun;
    #[async_trait::async_trait]
    impl Runner for NeverRun {
        async fn run(&self, _: &[String], _: Option<Vec<u8>>, _: u64) -> Result<CommandOutput> {
            anyhow::bail!("The runtime must not be invoked on a missing or replacement mount")
        }
    }
    #[tokio::test]
    async fn a_missing_selected_runtime_never_recreates_storage_on_an_ancestor_volume() {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout {
            version: 1,
            system_location_id: "one".into(),
            volume_id: "absent-volume".into(),
            runtime_directory: root
                .path()
                .join("missing/NodeHarbor/.nhtest")
                .to_string_lossy()
                .into(),
            system_gib: 16,
        };
        let runner = SelectedRuntimeRunner {
            inner: Arc::new(NeverRun),
            layout: layout.clone(),
            owner: "9511182e-9c48-4d20-a15b-1da8bb441386".into(),
        };
        let info = runner
            .run(&["list".into(), "--json".into()], None, 10)
            .await
            .unwrap();
        assert!(info.success && info.stdout.is_empty());
        assert!(runner
            .run(&["start".into(), "worker".into()], None, 10)
            .await
            .is_err());
        assert!(!layout.home().exists());
    }
}

#[cfg(all(test, unix))]
mod interrupted_copy_tests {
    use super::*;
    #[test]
    fn interrupted_copy_never_exposes_a_partial_vm_and_verified_retries_reuse_its_image() {
        use std::{cell::Cell, os::unix::fs::MetadataExt};
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("original");
        mkdir(&source).unwrap();
        mkdir(&source.join("worker")).unwrap();
        std::fs::write(source.join("worker/disk"), vec![42; 8 << 20]).unwrap();
        let owner = "9511182e-9c48-4d20-a15b-1da8bb441386";
        let layout = Layout {
            version: 1,
            system_location_id: "one".into(),
            volume_id: crate::storage::volume_identity(root.path()).unwrap(),
            runtime_directory: root.path().join(".nhtest").to_string_lossy().into(),
            system_gib: 16,
        };
        let calls = Cell::new(0);
        let result = copy_runtime(&source, &layout, owner, &[], &|| {
            calls.set(calls.get() + 1);
            anyhow::ensure!(calls.get() < 4, "Owner canceled while copying");
            Ok(())
        });
        assert!(result.is_err());
        assert!(
            !layout.home().join("worker").exists(),
            "Lima must never discover a partially copied VM"
        );
        copy_runtime(&source, &layout, owner, &[], &|| Ok(())).unwrap();
        let image = layout.home().join("worker/disk");
        assert_eq!(std::fs::read(&image).unwrap(), vec![42; 8 << 20]);
        let inode = image.metadata().unwrap().ino();
        copy_runtime(&source, &layout, owner, &[], &|| Ok(())).unwrap();
        assert_eq!(
            image.metadata().unwrap().ino(),
            inode,
            "Verified images must be reused without reserving a second full copy"
        );
    }
}
