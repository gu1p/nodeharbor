use crate::{CommandOutput, ProgressSink, Runner, VmInfo};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmProvider {
    #[default]
    Multipass,
    Lima,
}
impl VmProvider {
    pub fn native() -> Self {
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            Self::Lima
        } else {
            Self::Multipass
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Multipass => "multipass",
            Self::Lima => "lima",
        }
    }
}

/// Lima keeps this application's single worker in a private, standard data
/// directory. It does not use or change the owner's other Lima installations.
pub struct LimaRunner {
    program: PathBuf,
    home: PathBuf,
    #[cfg(test)]
    simulated_process: bool,
}
impl LimaRunner {
    pub fn new(program: PathBuf, home: PathBuf) -> Self {
        Self {
            program,
            home,
            #[cfg(test)]
            simulated_process: false,
        }
    }
    #[cfg(all(test, target_os = "macos"))]
    fn for_process_test(program: PathBuf, home: PathBuf) -> Self {
        Self {
            program,
            home,
            simulated_process: true,
        }
    }
    pub fn bundled(directory: &Path) -> Result<Self> {
        let executable = std::env::current_exe()?;
        Ok(Self::new(
            crate::runtime_platform::bundled_program(std::env::consts::OS, &executable)?,
            directory.join("lima"),
        ))
    }
    async fn command(
        &self,
        args: &[String],
        input: Option<Vec<u8>>,
        timeout: u64,
        progress: Option<ProgressSink>,
    ) -> Result<CommandOutput> {
        anyhow::ensure!(
            self.program.is_file(),
            "The bundled VM runtime is missing. Reinstall the complete NodeHarbor application"
        );
        if args
            .first()
            .is_some_and(|argument| matches!(argument.as_str(), "start" | "create"))
        {
            #[cfg(test)]
            let real_preflight = !self.simulated_process;
            #[cfg(not(test))]
            let real_preflight = true;
            if real_preflight {
                crate::runtime_platform::preflight().await?;
            }
        }
        let temporary = self.home.join("_tmp");
        if self.home.is_dir() && !temporary.exists() {
            std::fs::create_dir(&temporary)?;
        }
        let mut command = tokio::process::Command::new(&self.program);
        if temporary.is_dir() {
            command.env("TMPDIR", &temporary);
        }
        command.env("XDG_CACHE_HOME", self.home.join("_cache"));
        command
            .arg("--tty=false")
            .args(args)
            .env("LIMA_HOME", &self.home);
        #[cfg(target_os = "macos")]
        {
            command.env("SSH", "/usr/bin/ssh");
            let bin = self
                .program
                .parent()
                .context("Cannot locate bundled Lima tools")?;
            let paths = [
                bin,
                Path::new("/usr/bin"),
                Path::new("/bin"),
                Path::new("/usr/sbin"),
                Path::new("/sbin"),
            ];
            command.env("PATH", std::env::join_paths(paths)?);
        }
        let output = crate::process::run_command(
            command,
            input,
            timeout,
            progress,
            crate::process::OutputFormat::Lines,
        )
        .await?;
        Ok(startup_diagnostic(
            std::env::consts::OS,
            args.first().map(String::as_str),
            output,
        ))
    }
}
#[async_trait]
impl Runner for LimaRunner {
    async fn stream(
        &self,
        args: &[String],
        input: Option<std::fs::File>,
        output: Option<std::fs::File>,
        limit: u64,
    ) -> Result<()> {
        let mut command = tokio::process::Command::new(&self.program);
        let temporary = self.home.join("_tmp");
        if self.home.is_dir() && !temporary.exists() {
            std::fs::create_dir(&temporary)?;
        }
        if temporary.is_dir() {
            command.env("TMPDIR", temporary);
        }
        command.env("XDG_CACHE_HOME", self.home.join("_cache"));
        command
            .arg("--tty=false")
            .args(args)
            .env("LIMA_HOME", &self.home);
        #[cfg(target_os = "macos")]
        command.env("SSH", "/usr/bin/ssh");
        crate::process::stream_command(command, input, output, limit).await
    }
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<CommandOutput> {
        self.command(args, stdin, timeout, None).await
    }
    async fn run_with_progress(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
        progress: ProgressSink,
    ) -> Result<CommandOutput> {
        self.command(args, stdin, timeout, Some(progress)).await
    }
}

/// Keep the actionable summary ahead of the VM's diagnostic truncation. Raw
/// command lines still reach the activity sink with its existing redactions.
fn startup_diagnostic(
    platform: &str,
    operation: Option<&str>,
    mut output: CommandOutput,
) -> CommandOutput {
    if platform != "macos" || !matches!(operation, Some("start" | "create")) || output.success {
        return output;
    }
    let denied = output.stderr.lines().any(|line| {
        let Some((_, message)) = line.split_once("[hostagent] mkdir ") else {
            return false;
        };
        let Some((path, error)) = message.split_once(": ") else {
            return false;
        };
        let temporary = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("diskfs_iso"))
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            });
        temporary
            && matches!(
                error.split('"').next(),
                Some("operation not permitted" | "permission denied")
            )
    });
    if denied {
        // Redact before shortening so truncation cannot expose part of a secret.
        // 800 Unicode characters plus the summary fit the 4096-byte log limit.
        let details: String = crate::activity::redact(&output.stderr, &[])
            .chars()
            .take(800)
            .collect();
        output.stderr = format!(
            "Cannot create worker temporary files. Check NodeHarbor’s folder access in System Settings → Privacy & Security → Files & Folders. For an external drive, enable Removable Volumes for NodeHarbor, then quit and reopen the app.\nLima: {details}"
        );
    }
    output
}

pub(crate) fn info(output: &str) -> Result<VmInfo> {
    if output.trim().is_empty() {
        return Ok(VmInfo::default());
    }
    let value: Value =
        serde_json::from_str(output).context("Lima returned invalid worker information")?;
    anyhow::ensure!(
        value["name"] == "worker",
        "Lima returned a different instance; it has been left untouched"
    );
    let status = value["status"]
        .as_str()
        .context("Lima returned no worker state")?;
    Ok(VmInfo {
        installed: true,
        running: status != "Stopped",
        reachable: status == "Running",
        stopped: status == "Stopped",
        addresses: vec![],
    })
}

pub(crate) fn configuration(
    device: &str,
    resources: &nodeharbor_core::Resources,
    files: &Value,
) -> Result<Value> {
    uuid::Uuid::parse_str(device)?;
    let runtime: Value = serde_json::from_str(include_str!("../../../runtime/lima.json"))?;
    let image = &runtime["images"][std::env::consts::ARCH];
    anyhow::ensure!(
        image.is_object(),
        "This CPU architecture has no packaged worker image"
    );
    let mut data = vec![
        json!({"mode":"data","path":"/etc/nodeharbor/device-id","content":device,"owner":"root:root","permissions":"0600","overwrite":false}),
    ];
    for file in files.as_array().context("Invalid worker image files")? {
        data.push(json!({"mode":"data","path":file["path"],"content":file["content"],"owner":file["owner"],"permissions":file["permissions"],"overwrite":false}));
    }
    Ok(json!({
        "minimumLimaVersion":runtime["version"],"vmType":crate::runtime_platform::vm_type(std::env::consts::OS,std::env::consts::ARCH)?,"arch":std::env::consts::ARCH,
        "cpus":resources.cpus,"memory":format!("{}MiB",resources.memory_mib),"disk":format!("{}GiB",resources.disk_gib),
        "images":[image],"networks":[{"lima":"user-v2"}],"mounts":[],
        "containerd":{"user":false,"system":false},
        "plain":true,"ssh":{"overVsock":false,"loadDotSSHPubKeys":false,"forwardAgent":false,"forwardX11":false},
        "portForwards":[{"guestIP":"0.0.0.0","guestIPMustBeZero":false,"guestPortRange":[1,65535],"proto":"any","ignore":true}],
        "hostResolver":{"enabled":true},"propagateProxyEnv":false,"provision":data
    }))
}

pub(crate) fn storage_request(
    device: &str,
    pool_id: &str,
    locations: &[crate::storage::Location],
    previous: &[crate::storage::Location],
    generation: u64,
) -> Value {
    json!({"format":1,"deviceId":device,"poolId":pool_id,"generation":generation,
        "disks":locations.iter().enumerate().map(|(index,location)|json!({
            "id":location.id,"device":format!("/dev/vd{}",char::from(b'b'+index as u8)),
            "allocationBytes":location.allocation_gib*1024*1024*1024,
            "initialize":!previous.iter().any(|old|old.id==location.id)
        })).collect::<Vec<_>>()})
}

pub(crate) fn configuration_with_storage(
    device: &str,
    resources: &nodeharbor_core::Resources,
    files: &Value,
    locations: &[crate::storage::Location],
    pool_id: &str,
    generation: u64,
) -> Result<Value> {
    let mut configuration = configuration(device, resources, files)?;
    configuration["disk"] = json!("16GiB");
    update_storage_configuration(
        &mut configuration,
        device,
        pool_id,
        locations,
        &[],
        generation,
        true,
    )?;
    Ok(configuration)
}

/// The same durable contract is used by first boot and retained-VM replacement.
/// Only NodeHarbor's storage entries are replaced; owner identity, guest files,
/// compute resources, and unrelated provisioning remain intact.
pub(crate) fn update_storage_configuration(
    configuration: &mut Value,
    device: &str,
    pool_id: &str,
    locations: &[crate::storage::Location],
    previous: &[crate::storage::Location],
    generation: u64,
    migrate: bool,
) -> Result<()> {
    uuid::Uuid::parse_str(device)?;
    uuid::Uuid::parse_str(pool_id)?;
    anyhow::ensure!(
        !locations.is_empty() && locations.len() <= 16,
        "Choose one to sixteen storage disks"
    );
    configuration["plain"] = json!(true);
    configuration["ssh"]["overVsock"] = json!(false);
    configuration["additionalDisks"] = json!(locations
        .iter()
        .map(|location| json!({"name":location.id,"format":false}))
        .collect::<Vec<_>>());
    let mut provision = configuration["provision"]
        .as_array()
        .context("Missing worker provisioning")?
        .clone();
    provision.retain(|entry| {
        entry["path"] != "/etc/nodeharbor/storage-request.json"
            && !entry["script"]
                .as_str()
                .is_some_and(|script| script.contains("/usr/local/lib/nodeharbor/storage_pool.py"))
    });
    provision.push(json!({"mode":"data","path":"/etc/nodeharbor/storage-request.json","owner":"root:root","permissions":"0600","overwrite":true,"content":serde_json::to_string(&storage_request(device,pool_id,locations,previous,generation))?}));
    let mut script = String::from(
        r#"#!/bin/sh
set -eu
if ! command -v pvs >/dev/null || ! command -v rsync >/dev/null || ! command -v growpart >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y lvm2 e2fsprogs util-linux cloud-guest-utils rsync
fi
if [ -f /etc/nodeharbor/storage-state.json ] || [ -f /etc/nodeharbor/storage-pending.json ]; then
  python3 /usr/local/lib/nodeharbor/storage_pool.py activate
else
  python3 /usr/local/lib/nodeharbor/storage_pool.py apply < /etc/nodeharbor/storage-request.json
fi
"#,
    );
    if migrate {
        script.push_str("python3 /usr/local/lib/nodeharbor/storage_pool.py migrate\n");
    }
    script.push_str("python3 /usr/local/lib/nodeharbor/storage_pool.py check\n");
    provision.push(json!({"mode":"system","script":script}));
    configuration["provision"] = json!(provision);
    let mut probes = if configuration["probes"].is_null() {
        vec![]
    } else {
        configuration["probes"]
            .as_array()
            .context("Invalid worker readiness probes")?
            .clone()
    };
    probes.retain(|probe| {
        !probe["script"]
            .as_str()
            .is_some_and(|script| script.contains("/usr/local/lib/nodeharbor/storage_pool.py"))
    });
    probes.push(json!({"mode":"readiness","description":"The complete configured worker storage must be mounted","script":"#!/bin/sh\nsudo python3 /usr/local/lib/nodeharbor/storage_pool.py check\n"}));
    configuration["probes"] = json!(probes);
    Ok(())
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    fn failure(stderr: &str) -> CommandOutput {
        CommandOutput {
            success: false,
            stdout: "unchanged output".into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn temporary_directory_denials_explain_macos_recovery_and_retain_diagnostics() {
        for operation in ["start", "create"] {
            for error in ["operation not permitted", "permission denied"] {
                let stderr = format!(
                    "time=\"fixture\" level=error msg=\"[hostagent] mkdir /Volumes/External Disk/tmp/diskfs_iso123: {error}\" fields.level=fatal"
                );
                let output = startup_diagnostic("macos", Some(operation), failure(&stderr));
                assert!(!output.success);
                assert_eq!(output.stdout, "unchanged output");
                assert!(output
                    .stderr
                    .starts_with("Cannot create worker temporary files."));
                assert!(output.stderr.contains("Removable Volumes for NodeHarbor"));
                assert!(output.stderr.contains("quit and reopen"));
                assert!(output.stderr.contains(&stderr));
            }
        }
    }

    #[test]
    fn guidance_survives_activity_limits_without_exposing_sensitive_output() {
        let denial = "[hostagent] mkdir /Volumes/Test/tmp/diskfs_iso9: operation not permitted";
        for stderr in [
            format!("{}\n{denial}", "download progress\n".repeat(500)),
            format!("password=private-fixture-value\n{denial}"),
            format!("{}\n{denial}", "🦀".repeat(950)),
        ] {
            let output = startup_diagnostic("macos", Some("start"), failure(&stderr));
            let logged = crate::activity::redact(&output.stderr, &[]);
            assert!(logged.starts_with("Cannot create worker temporary files."));
            assert!(logged
                .chars()
                .take(1200)
                .collect::<String>()
                .contains("quit and reopen"));
            assert!(!logged.contains("private-fixture-value"));
            assert!(output.stderr.len() <= 4096);
        }
    }

    #[test]
    fn unrelated_errors_platforms_operations_and_success_are_unchanged() {
        let denial = "[hostagent] mkdir /Volumes/Test/tmp/diskfs_iso9: operation not permitted";
        for (platform, operation, success, stderr) in [
            ("linux", Some("start"), false, denial),
            ("windows", Some("start"), false, denial),
            ("macos", Some("shell"), false, denial),
            ("macos", Some("stop"), false, denial),
            ("macos", None, false, denial),
            ("macos", Some("start"), true, denial),
            ("macos", Some("start"), false, "VM launch timed out"),
            (
                "macos",
                Some("start"),
                false,
                "[hostagent] mkdir /Volumes/Test/worker: operation not permitted",
            ),
            (
                "macos",
                Some("start"),
                false,
                "[hostagent] mkdir /Volumes/Test/diskfs_iso9: no space left on device",
            ),
            (
                "macos",
                Some("start"),
                false,
                "mkdir /tmp/diskfs_iso9: permission denied",
            ),
            (
                "macos",
                Some("start"),
                false,
                "[hostagent] mkdir /tmp/diskfs_iso9: read-only file system\npermission denied",
            ),
        ] {
            let mut output = failure(stderr);
            output.success = success;
            let result = startup_diagnostic(platform, operation, output);
            assert_eq!(result.stderr, stderr);
            assert_eq!(result.success, success);
            assert_eq!(result.stdout, "unchanged output");
        }
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    const OWNER: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
    const POOL: &str = "40423e6c-1de0-46dc-a634-144671b655cd";
    fn disk(id: &str, gib: u64) -> crate::storage::Location {
        crate::storage::Location {
            id: id.into(),
            volume_id: "fixture".into(),
            directory: format!("/fixture/{id}"),
            allocation_gib: gib,
        }
    }
    #[test]
    fn storage_request_uses_the_durable_pool_and_marks_only_new_members_for_initialization() {
        let old = disk("nhold", 15);
        let new = disk("nhnew", 15);
        let request = storage_request(OWNER, POOL, &[old.clone(), new], &[old], 7);
        assert_eq!(request["deviceId"], OWNER);
        assert_eq!(request["poolId"], POOL);
        assert_eq!(request["generation"], 7);
        assert_eq!(request["disks"][0]["initialize"], false);
        assert_eq!(request["disks"][1]["initialize"], true);
        assert_eq!(request["disks"][1]["device"], "/dev/vdc");
    }
    #[test]
    fn replacement_configuration_is_idempotent_and_preserves_unrelated_provisioning() {
        let result = configuration_with_storage(
            OWNER,
            &nodeharbor_core::Resources::default(),
            &json!([]),
            &[disk("nhold", 30)],
            OWNER,
            1,
        );
        if cfg!(target_os = "windows") {
            assert!(result.unwrap_err().to_string().contains("macOS or Linux"));
            return;
        }
        let mut config = result.unwrap();
        let unrelated = json!({"mode":"system","script":"echo unrelated"});
        config["provision"]
            .as_array_mut()
            .unwrap()
            .push(unrelated.clone());
        config["probes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"mode":"readiness","script":"true"}));
        let target = [disk("nhnew", 15)];
        update_storage_configuration(&mut config, OWNER, POOL, &target, &[], 2, false).unwrap();
        let once = config.clone();
        update_storage_configuration(&mut config, OWNER, POOL, &target, &[], 2, false).unwrap();
        assert_eq!(
            config, once,
            "Retry must not duplicate provisioning or probes"
        );
        assert_eq!(config["disk"], "16GiB");
        assert_eq!(config["mounts"], json!([]));
        let provision = config["provision"].as_array().unwrap();
        assert!(provision.contains(&unrelated));
        let entries: Vec<_> = provision
            .iter()
            .filter(|p| p["path"] == "/etc/nodeharbor/storage-request.json")
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["overwrite"], true);
        let request: Value = serde_json::from_str(entries[0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(request, storage_request(OWNER, POOL, &target, &[], 2));
        assert!(!provision.iter().any(|p| p["script"]
            .as_str()
            .is_some_and(|s| s.contains("storage_pool.py migrate"))));
        assert_eq!(config["probes"].as_array().unwrap().len(), 2);
        assert!(provision
            .iter()
            .any(|p| p["path"] == "/etc/nodeharbor/device-id" && p["overwrite"] == false));
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "lima_process_tests.rs"]
mod process_tests;
