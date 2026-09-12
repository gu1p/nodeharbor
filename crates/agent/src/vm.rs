use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};

pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}
#[async_trait]
pub trait Runner: Send + Sync {
    async fn run(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<CommandOutput>;
}
pub struct MultipassRunner;
fn multipass_program() -> PathBuf {
    #[cfg(target_os = "macos")]
    let candidates = vec![
        PathBuf::from("/usr/local/bin/multipass"),
        PathBuf::from("/opt/homebrew/bin/multipass"),
    ];
    #[cfg(target_os = "linux")]
    let candidates = vec![
        PathBuf::from("/snap/bin/multipass"),
        PathBuf::from("/usr/bin/multipass"),
    ];
    #[cfg(target_os = "windows")]
    let candidates = vec![PathBuf::from(
        std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into()),
    )
    .join("Multipass/bin/multipass.exe")];
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("multipass"))
}
#[async_trait]
impl Runner for MultipassRunner {
    async fn run(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<CommandOutput> {
        let mut command = Command::new(multipass_program());
        command
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child=command.spawn().context("Multipass is unavailable. Install it from canonical.com/multipass/install, then try again")?;
        if let Some(bytes) = stdin {
            child
                .stdin
                .take()
                .context("Worker input is unavailable")?
                .write_all(&bytes)
                .await?;
        }
        let output = tokio::time::timeout(Duration::from_secs(timeout), child.wait_with_output())
            .await
            .context("The worker operation timed out")??;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into(),
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        })
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmInfo {
    pub installed: bool,
    pub running: bool,
    /// Unknown and transitional states may consume resources but cannot run guest commands.
    #[serde(default)]
    pub reachable: bool,
    #[serde(default)]
    pub addresses: Vec<String>,
}
pub struct Vm {
    pub name: String,
    device_id: String,
    runner: Arc<dyn Runner>,
    receipt: Option<PathBuf>,
}
impl Vm {
    pub fn new(device_id: &str, runner: Arc<dyn Runner>) -> Result<Self> {
        Ok(Self {
            name: crate::managed_vm_name(device_id)?,
            device_id: device_id.into(),
            runner,
            receipt: None,
        })
    }
    pub fn local(device_id: &str) -> Result<Self> {
        Self::new(device_id, Arc::new(MultipassRunner))
    }
    pub fn managed(device_id: &str, directory: &Path, runner: Arc<dyn Runner>) -> Result<Self> {
        let mut vm = Self::new(device_id, runner)?;
        vm.receipt = Some(directory.join(format!("{}.receipt.json", vm.name)));
        Ok(vm)
    }
    pub fn local_in(device_id: &str, directory: &Path) -> Result<Self> {
        Self::managed(device_id, directory, Arc::new(MultipassRunner))
    }
    pub fn has_receipt(&self) -> Result<bool> {
        let Some(path) = &self.receipt else {
            return Ok(false);
        };
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let receipt: Value =
            serde_json::from_reader(file).context("The VM creation receipt is damaged")?;
        anyhow::ensure!(
            receipt["version"] == 1
                && receipt["deviceId"] == self.device_id
                && receipt["name"] == self.name,
            "The VM creation receipt does not match this device"
        );
        Ok(true)
    }
    fn record_creation(&self) -> Result<()> {
        let path = self
            .receipt
            .as_ref()
            .context("A local creation receipt is required to prepare a worker")?;
        let mut file =
            tempfile::NamedTempFile::new_in(path.parent().context("Invalid receipt directory")?)?;
        serde_json::to_writer(
            &mut file,
            &json!({"version":1,"deviceId":self.device_id,"name":self.name}),
        )?;
        file.as_file().sync_all()?;
        file.persist(path)
            .context("Cannot save the VM creation receipt")?;
        Ok(())
    }
    async fn command(
        &self,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<String> {
        let output = self.runner.run(&args, stdin, timeout).await?;
        anyhow::ensure!(
            output.success,
            "Multipass: {}",
            output.stderr.chars().take(1200).collect::<String>().trim()
        );
        Ok(output.stdout)
    }
    pub async fn info(&self) -> Result<VmInfo> {
        let output = self
            .command(
                vec!["list".into(), "--format".into(), "json".into()],
                None,
                15,
            )
            .await?;
        let value: Value = serde_json::from_str(&output)
            .context("Multipass returned invalid machine information")?;
        let items = value["list"]
            .as_array()
            .context("Multipass returned no machine inventory")?;
        match items.iter().find(|item| item["name"] == self.name) {
            None => Ok(VmInfo::default()),
            Some(item) => Ok(VmInfo {
                installed: true,
                running: !matches!(
                    item["state"].as_str(),
                    Some("Stopped" | "Suspended" | "Deleted")
                ),
                reachable: item["state"] == "Running",
                addresses: item["ipv4"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
        }
    }
    async fn guest(
        &self,
        command: &[&str],
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<String> {
        let mut args = vec!["exec".into(), self.name.clone(), "--".into()];
        args.extend(command.iter().map(|s| (*s).to_owned()));
        self.command(args, stdin, timeout).await
    }
    pub async fn verify_owner(&self) -> Result<()> {
        let owner = self
            .guest(&["sudo", "cat", "/etc/nodeharbor/device-id"], None, 15)
            .await?;
        anyhow::ensure!(
            owner.trim() == self.device_id,
            "This VM does not belong to the enrolled device; it has been left untouched"
        );
        Ok(())
    }
    pub async fn stop(&self) -> Result<()> {
        if !self.has_receipt()? {
            self.verify_owner().await?;
        }
        self.command(vec!["stop".into(), self.name.clone()], None, 60)
            .await?;
        Ok(())
    }
    pub async fn start(&self) -> Result<()> {
        anyhow::ensure!(
            self.has_receipt()?,
            "This application has no creation receipt for the VM; it has been left untouched"
        );
        self.command(vec!["start".into(), self.name.clone()], None, 180)
            .await?;
        self.verify_owner().await?;
        Ok(())
    }
    pub async fn create(
        &self,
        resources: &nodeharbor_core::Resources,
        directory: &Path,
        files: Value,
    ) -> Result<()> {
        anyhow::ensure!(
            !self.info().await?.installed,
            "The worker VM already exists; refusing to replace it"
        );
        let mut writes = vec![
            json!({"path":"/etc/nodeharbor/device-id","owner":"root:root","permissions":"0600","content":self.device_id}),
        ];
        writes.extend(
            files
                .as_array()
                .context("Invalid worker image files")?
                .iter()
                .cloned(),
        );
        let cloud_config = json!({"write_files":writes});
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        use std::io::Write;
        writeln!(file, "#cloud-config\n{cloud_config}")?;
        file.flush()?;
        // Persist before launch: the hypervisor may create the VM and then fail
        // while waiting for guest networking. Recovery must not depend on SSH.
        self.record_creation()?;
        self.command(
            vec![
                "launch".into(),
                "24.04".into(),
                "--name".into(),
                self.name.clone(),
                "--cpus".into(),
                resources.cpus.to_string(),
                "--memory".into(),
                format!("{}M", resources.memory_mib),
                "--disk".into(),
                format!("{}G", resources.disk_gib),
                "--cloud-init".into(),
                file.path().to_string_lossy().into(),
            ],
            None,
            1200,
        )
        .await?;
        self.guest(&["sudo", "cloud-init", "status", "--wait"], None, 180)
            .await?;
        self.verify_owner().await
    }
    pub async fn configure(&self, bootstrap: Value) -> Result<()> {
        self.verify_owner().await?;
        self.guest(
            &[
                "sudo",
                "python3",
                "/usr/local/lib/nodeharbor/configure_worker.py",
            ],
            Some(serde_json::to_vec(&bootstrap)?),
            600,
        )
        .await?;
        Ok(())
    }
    pub async fn renew_lease(&self) -> Result<()> {
        self.verify_owner().await?;
        self.guest(
            &[
                "sudo",
                "python3",
                "/usr/local/lib/nodeharbor/watchdog.py",
                "renew",
            ],
            None,
            15,
        )
        .await?;
        Ok(())
    }
    pub async fn workloads(&self) -> Result<Vec<Value>> {
        self.verify_owner().await?;
        let output = self
            .guest(
                &["sudo", "/usr/local/bin/k3s", "crictl", "pods", "-o", "json"],
                None,
                20,
            )
            .await?;
        let value: Value = serde_json::from_str(&output)?;
        Ok(value["items"].as_array().into_iter().flatten().filter(|pod|pod["state"]=="SANDBOX_READY"&&pod["metadata"]["namespace"]!="kube-system").map(|pod|json!({"name":pod["metadata"]["name"],"namespace":pod["metadata"]["namespace"],"state":"running"})).collect())
    }
}
