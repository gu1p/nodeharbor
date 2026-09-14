use anyhow::{anyhow, ensure, Context, Result};
use flate2::read::GzDecoder;
use objc2_foundation::{NSFileManager, NSFileManagerItemReplacementOptions, NSString, NSURL};
use std::{fs, io::Cursor, os::unix::fs::PermissionsExt, path::Path};

/// Install bytes already authenticated by the Tauri updater's download API.
/// Apple's file replacement API requires staging on the destination volume.
/// Never redirect the owner's TMPDIR or move their app into another filesystem.
pub fn install_verified(app: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        fs::symlink_metadata(app)?.is_dir(),
        "The installed app must be a directory, not a symbolic link"
    );
    let parent = app
        .parent()
        .context("The application has no parent directory")?;
    let staging = tempfile::Builder::new()
        .prefix(".nodeharbor-update-")
        .tempdir_in(parent)
        .context("Cannot stage the update beside the application")?;
    let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)));
    archive
        .unpack(staging.path())
        .context("Cannot unpack the verified update")?;
    let replacement = staging.path().join("NodeHarbor.app");
    ensure!(
        fs::read_dir(staging.path())?.count() == 1 && fs::symlink_metadata(&replacement)?.is_dir(),
        "The update must contain one NodeHarbor application"
    );
    ensure!(
        fs::symlink_metadata(replacement.join("Contents"))?.is_dir(),
        "The update has no application contents"
    );
    ensure!(
        fs::symlink_metadata(replacement.join("Contents/Info.plist"))?.is_file(),
        "The update has no application metadata"
    );
    ensure!(
        fs::symlink_metadata(replacement.join("Contents/MacOS"))?.is_dir(),
        "The update has no executables directory"
    );
    for name in ["nodeharbor", "nodeharbor-agent"] {
        let metadata = fs::symlink_metadata(replacement.join("Contents/MacOS").join(name))?;
        ensure!(
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
            "The update has no runnable {name}"
        );
    }
    let original_url = NSURL::from_directory_path(app).context("Invalid application path")?;
    let replacement_url =
        NSURL::from_directory_path(&replacement).context("Invalid update path")?;
    let backup = format!(
        "{}.previous.app",
        staging
            .path()
            .file_name()
            .context("Missing staging name")?
            .to_string_lossy()
    );
    NSFileManager::defaultManager()
        .replaceItemAtURL_withItemAtURL_backupItemName_options_resultingItemURL_error(
            &original_url,
            &replacement_url,
            Some(&NSString::from_str(&backup)),
            NSFileManagerItemReplacementOptions::UsingNewMetadataOnly,
            None,
        )
        .map_err(|error| anyhow!("macOS could not replace the application: {error}"))?;
    Ok(())
}
