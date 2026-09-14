//! Native Lima package paths and non-mutating host capability checks.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn vm_type(platform: &str, architecture: &str) -> Result<&'static str> {
    anyhow::ensure!(
        matches!(architecture, "aarch64" | "x86_64"),
        "This CPU architecture has no packaged Lima worker runtime"
    );
    match platform {
        "macos" => Ok("vz"),
        "linux" => Ok("qemu"),
        _ => anyhow::bail!("Selectable Lima worker storage requires macOS or Linux"),
    }
}

/// Both executable names use the main application's resource directory.
/// Deriving Linux's prefix from the executable also handles mounted AppImages.
pub fn bundled_program(platform: &str, executable: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        executable.is_absolute(),
        "The application path must be absolute"
    );
    let prefix = executable
        .parent()
        .and_then(Path::parent)
        .context("Cannot locate application resources")?;
    match platform {
        "macos" => Ok(prefix.join("Resources/lima/bin/limactl")),
        "linux" => Ok(prefix.join("lib/NodeHarbor/lima/bin/limactl")),
        _ => anyhow::bail!("This bundled worker runtime requires macOS or Linux"),
    }
}

fn qemu_version(output: &str, prefix: &str) -> Result<(u64, u64, u64)> {
    let version = output
        .strip_prefix(prefix)
        .and_then(|text| text.split_whitespace().next())
        .context("The installed QEMU tool did not report a supported version")?;
    let mut parts = version.split('.');
    let major = parts
        .next()
        .context("Missing QEMU major version")?
        .parse()?;
    let minor = parts
        .next()
        .context("Missing QEMU minor version")?
        .parse()?;
    let patch = parts
        .next()
        .context("Missing QEMU patch version")?
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .context("Missing QEMU patch version")?
        .parse()?;
    Ok((major, minor, patch))
}

pub fn validate_linux_host(
    architecture: &str,
    emulator_version: &str,
    image_version: &str,
    kvm_api_version: i32,
) -> Result<()> {
    vm_type("linux", architecture)?;
    anyhow::ensure!(
        qemu_version(emulator_version, "QEMU emulator version ")? >= (6, 2, 0)
            && qemu_version(image_version, "qemu-img version ")? >= (6, 2, 0),
        "The Linux worker requires QEMU and qemu-img version 6.2.0 or newer"
    );
    anyhow::ensure!(
        kvm_api_version == 12,
        "KVM hardware virtualization is unavailable to this user; the worker cannot start"
    );
    Ok(())
}

pub fn validate_macos_host(version: &str, hardware_virtualization: bool) -> Result<()> {
    let major: u64 = version
        .split('.')
        .next()
        .context("Cannot determine the macOS version")?
        .parse()
        .context("Cannot determine the macOS version")?;
    anyhow::ensure!(
        major >= 14,
        "The packaged worker requires macOS 14 or newer"
    );
    anyhow::ensure!(
        hardware_virtualization,
        "Hardware virtualization is unavailable on this Mac; the worker cannot start"
    );
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn output(program: &str, args: &[&str]) -> Result<String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    let result =
        crate::process::run_command(command, None, 10, None, crate::process::OutputFormat::Lines)
            .await
            .with_context(|| format!("Cannot inspect the required host runtime tool {program}"))?;
    anyhow::ensure!(
        result.success,
        "Cannot inspect the required host runtime tool {program}"
    );
    Ok(result.stdout.trim().into())
}

#[cfg(target_os = "linux")]
fn kvm_api_version() -> Result<i32> {
    use std::os::{fd::AsRawFd, unix::fs::FileTypeExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .context("KVM is unavailable to this user; Linux workers require access to /dev/kvm")?;
    anyhow::ensure!(
        file.metadata()?.file_type().is_char_device(),
        "The host KVM device is invalid"
    );
    // KVM_GET_API_VERSION is the read-only system ioctl documented by Linux.
    // It does not create a VM or change the device configuration.
    let version = unsafe { libc::ioctl(file.as_raw_fd(), 0xAE00) };
    Ok(version)
}

/// Run before create/start/apply. Inspection and stopping remain available even
/// if the host later loses a dependency or permission needed to start a worker.
pub async fn preflight() -> Result<()> {
    vm_type(std::env::consts::OS, std::env::consts::ARCH)?;
    #[cfg(target_os = "macos")]
    {
        let version = output("/usr/bin/sw_vers", &["-productVersion"]).await?;
        let virtualization = output("/usr/sbin/sysctl", &["-n", "kern.hv_support"]).await?;
        validate_macos_host(&version, virtualization == "1")?;
    }
    #[cfg(target_os = "linux")]
    {
        let program = match std::env::consts::ARCH {
            "aarch64" => "qemu-system-aarch64",
            "x86_64" => "qemu-system-x86_64",
            _ => unreachable!("vm_type rejects unsupported architectures"),
        };
        let emulator = output(program, &["--version"]).await.context(
            "Install the QEMU system emulator for this CPU architecture before preparing a worker",
        )?;
        let image = output("qemu-img", &["--version"])
            .await
            .context("Install qemu-utils before preparing a worker")?;
        validate_linux_host(
            std::env::consts::ARCH,
            &emulator,
            &image,
            kvm_api_version()?,
        )?;
    }
    Ok(())
}
