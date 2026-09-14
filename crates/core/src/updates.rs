use minisign_verify::{PublicKey, Signature};
use std::io::Read;

pub const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;

/// Authenticate a bounded stream without holding an APK in the owner's memory.
pub fn verify_signed_update(
    mut reader: impl Read,
    key: &str,
    signature: &str,
) -> Result<(), String> {
    let invalid = || "The application update signature could not be verified".to_string();
    if key.len() > 4096 || signature.len() > 8192 {
        return Err(invalid());
    }
    let key = PublicKey::decode(key).map_err(|_| invalid())?;
    let signature = Signature::decode(signature).map_err(|_| invalid())?;
    let mut verifier = key.verify_stream(&signature).map_err(|_| invalid())?;
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let count = reader.read(&mut buffer).map_err(|_| invalid())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_UPDATE_BYTES {
            return Err("The application update exceeds its supported size".into());
        }
        verifier.update(&buffer[..count]);
    }
    verifier.finalize().map_err(|_| invalid())
}

pub(crate) fn verify_release_file(path: &str, signature: &str) -> bool {
    let path = std::path::Path::new(path);
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return false;
    }
    let Ok(metadata) = path.symlink_metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > MAX_UPDATE_BYTES {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    verify_signed_update(
        file,
        include_str!("../../../nodeharbor.minisign.pub"),
        signature,
    )
    .is_ok()
}
