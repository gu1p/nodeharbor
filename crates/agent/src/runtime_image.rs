//! Download the pinned image into the selected runtime filesystem. This avoids
//! Lima's platform-global download cache on the settings/system volume.
use crate::storage_layout::Layout;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

pub(crate) async fn download(
    layout: &Layout,
    owner: &str,
    url: &str,
    digest: &str,
    canceled: &(dyn Fn() -> Result<()> + Sync),
) -> Result<PathBuf> {
    canceled()?;
    crate::runtime_storage::validate_home(layout, owner)?;
    let expected = digest
        .strip_prefix("sha256:")
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("The pinned VM image has an invalid digest")?;
    let home = layout.home();
    let destination = home.join(format!(".boot-{expected}"));
    if let Ok(metadata) = std::fs::symlink_metadata(&destination) {
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "The cached VM image was replaced"
        );
        let mut file = std::fs::File::open(&destination)?;
        let mut hash = Sha256::new();
        let mut bytes = [0; 65536];
        loop {
            canceled()?;
            let n = file.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            hash.update(&bytes[..n]);
        }
        anyhow::ensure!(
            format!("{:x}", hash.finalize()) == expected,
            "The cached VM image failed SHA-256 verification"
        );
        return Ok(destination);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(900))
        .build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .context("Cannot download the pinned VM image")?
        .error_for_status()?;
    const LIMIT: u64 = 4 << 30;
    anyhow::ensure!(
        response.content_length().is_none_or(|n| n <= LIMIT),
        "The VM image download exceeds its preparation allowance"
    );
    let mut file = tempfile::Builder::new()
        .prefix(".boot-download-")
        .tempfile_in(&home)?;
    anyhow::ensure!(
        crate::storage::volume_identity_file(file.as_file())? == layout.volume_id,
        "The VM download volume changed"
    );
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    loop {
        canceled()?;
        let chunk = tokio::select! { chunk=response.chunk()=>chunk?, _=interval.tick()=>continue };
        let Some(chunk) = chunk else {
            break;
        };
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .context("VM image size overflow")?;
        anyhow::ensure!(
            bytes <= LIMIT,
            "The VM image download exceeds its preparation allowance"
        );
        hash.update(&chunk);
        file.write_all(&chunk)?;
    }
    anyhow::ensure!(
        format!("{:x}", hash.finalize()) == expected,
        "The VM image download failed SHA-256 verification"
    );
    canceled()?;
    crate::runtime_storage::validate_home(layout, owner)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(&destination)?;
    Ok(destination)
}

pub(crate) fn cleanup(layout: &Layout, owner: &str) -> Result<()> {
    crate::runtime_storage::validate_home(layout, owner)?;
    for entry in std::fs::read_dir(layout.home())? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.strip_prefix(".boot-").is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            let m = std::fs::symlink_metadata(entry.path())?;
            anyhow::ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "The cached VM image was replaced"
            );
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[tokio::test]
    async fn initial_image_download_is_verified_on_the_selected_volume() {
        let folder = tempfile::tempdir().unwrap();
        let layout = crate::storage_layout::Layout {
            version: 1,
            system_location_id: "one".into(),
            volume_id: crate::storage::volume_identity(folder.path()).unwrap(),
            runtime_directory: folder.path().join(".nhtest").to_string_lossy().into(),
            system_gib: 16,
        };
        let owner = "9511182e-9c48-4d20-a15b-1da8bb441386";
        crate::runtime_storage::prepare_home(&layout, owner).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/image", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new()
                    .route("/image", axum::routing::get(|| async { "fixture image" })),
            )
            .await
            .unwrap();
        });
        let digest = format!("sha256:{:x}", Sha256::digest(b"fixture image"));
        let path = download(&layout, owner, &url, &digest, &|| Ok(()))
            .await
            .unwrap();
        assert!(path.starts_with(layout.home()));
        assert_eq!(std::fs::read(path).unwrap(), b"fixture image");
        let wrong = format!("sha256:{}", "0".repeat(64));
        assert!(download(&layout, owner, &url, &wrong, &|| Ok(()))
            .await
            .is_err());
        assert!(download(&layout, owner, &url, &digest, &|| anyhow::bail!(
            "Owner stopped preparation"
        ))
        .await
        .is_err());
        server.abort();
    }
}
