use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::process::Command;

pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}
#[async_trait]
pub trait Runner: Send + Sync {
    fn provider(&self) -> crate::VmProvider {
        crate::VmProvider::Multipass
    }
    async fn run(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> Result<CommandOutput>;
    async fn run_with_progress(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
        progress: crate::ProgressSink,
    ) -> Result<CommandOutput> {
        let output = self.run(args, stdin, timeout).await?;
        for (stream, text) in [
            (crate::OutputStream::Stdout, &output.stdout),
            (crate::OutputStream::Stderr, &output.stderr),
        ] {
            for line in text.split(['\r', '\n']).filter(|line| !line.is_empty()) {
                progress(stream, line);
            }
        }
        Ok(output)
    }
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
        multipass_command(args, stdin, timeout, None).await
    }
    async fn run_with_progress(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
        progress: crate::ProgressSink,
    ) -> Result<CommandOutput> {
        multipass_command(args, stdin, timeout, Some(progress)).await
    }
}
async fn multipass_command(
    args: &[String],
    stdin: Option<Vec<u8>>,
    timeout: u64,
    progress: Option<crate::ProgressSink>,
) -> Result<CommandOutput> {
    let program = multipass_program();
    let mut command = Command::new(&program);
    command.args(args);
    let format = if args.first().is_some_and(|arg| arg == "exec") {
        crate::process::OutputFormat::Lines
    } else {
        crate::process::OutputFormat::Terminal
    };
    let result = crate::process::run_command(command, stdin, timeout, progress, format).await;
    match result {
        Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|error|error.kind()==std::io::ErrorKind::NotFound)=>
            Err(error.context("Multipass is unavailable. Install it from canonical.com/multipass/install, then try again")),
        other=>other,
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
    pub stopped: bool,
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
        let identity_name = crate::managed_vm_name(device_id)?;
        let name = if runner.provider() == crate::VmProvider::Lima {
            "worker".into()
        } else {
            identity_name
        };
        Ok(Self {
            name,
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
    pub(crate) fn native_runner(
        provider: crate::VmProvider,
        directory: &Path,
    ) -> Result<Arc<dyn Runner>> {
        Ok(match provider {
            crate::VmProvider::Multipass => Arc::new(MultipassRunner),
            crate::VmProvider::Lima => Arc::new(crate::LimaRunner::bundled(directory)?),
        })
    }
    fn is_lima(&self) -> bool {
        self.runner.provider() == crate::VmProvider::Lima
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
            (if self.is_lima() {
                receipt["version"] == 2 && receipt["provider"] == "lima"
            } else {
                receipt["version"] == 1
            }) && receipt["deviceId"] == self.device_id
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
            &if self.is_lima() {
                json!({"version":2,"provider":"lima","deviceId":self.device_id,"name":self.name})
            } else {
                json!({"version":1,"deviceId":self.device_id,"name":self.name})
            },
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
            "{}: {}",
            self.runner.provider().name(),
            output.stderr.chars().take(1200).collect::<String>().trim()
        );
        Ok(output.stdout)
    }
    pub async fn info(&self) -> Result<VmInfo> {
        if self.is_lima() {
            let output = self
                .command(
                    vec![
                        "list".into(),
                        "--format=json".into(),
                        "--filter=.name == \"worker\"".into(),
                    ],
                    None,
                    15,
                )
                .await?;
            return crate::lima::info(&output);
        }
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
                stopped: item["state"] == "Stopped",
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
        let mut args = if self.is_lima() {
            vec!["shell".into(), "--workdir=/".into(), self.name.clone()]
        } else {
            vec!["exec".into(), self.name.clone(), "--".into()]
        };
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
        self.shutdown(false).await
    }
    pub async fn stop_now(&self) -> Result<()> {
        self.shutdown(true).await
    }
    async fn shutdown(&self, immediate: bool) -> Result<()> {
        if !self.has_receipt()? {
            self.verify_owner().await?;
        }
        let mut args = vec!["stop".into()];
        if immediate {
            args.push("--force".into());
        }
        args.push(self.name.clone());
        self.command(args, None, 60).await?;
        anyhow::ensure!(
            !self.info().await?.running,
            "The VM runtime has not confirmed that the worker stopped"
        );
        Ok(())
    }
    pub async fn start(&self) -> Result<()> {
        anyhow::ensure!(
            self.has_receipt()?,
            "This application has no creation receipt for the VM; it has been left untouched"
        );
        let args = if self.is_lima() {
            vec!["start".into(), "--timeout=10m".into(), self.name.clone()]
        } else {
            vec!["start".into(), self.name.clone()]
        };
        // Older guests can wait five minutes for K3s's first fresh Ready report
        // before receiving the packaged kubelet reporting update.
        let startup = self.command(args, None, 600);
        tokio::pin!(startup);
        let period = std::time::Duration::from_secs(30);
        let mut renewals = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                result = &mut startup => { result?; break; }
                _ = renewals.tick() => {
                    // SSH can become available before the VM runtime finishes its
                    // boot checks. Keep this owned startup alive within its existing
                    // deadline; dropping startup also drops every lease renewal.
                    let Ok(owner) = self.guest(&["sudo", "cat", "/etc/nodeharbor/device-id"], None, 15).await else {
                        continue;
                    };
                    anyhow::ensure!(owner.trim() == self.device_id,
                        "This VM does not belong to the enrolled device; it has been left untouched");
                    self.renew_lease().await?;
                }
            }
        }
        self.verify_owner().await?;
        Ok(())
    }
    pub async fn resize(&self, resources: &nodeharbor_core::Resources) -> Result<()> {
        anyhow::ensure!(
            self.has_receipt()?,
            "This application has no creation receipt for the VM"
        );
        let info = self.info().await?;
        anyhow::ensure!(
            info.installed && !info.running,
            "Stop the worker before changing its resources"
        );
        if self.is_lima() {
            self.command(
                vec![
                    "edit".into(),
                    self.name.clone(),
                    format!("--cpus={}", resources.cpus),
                    format!("--memory={}", resources.memory_mib as f64 / 1024.0),
                    format!("--disk={}", resources.disk_gib),
                ],
                None,
                60,
            )
            .await?;
            return Ok(());
        }
        for (key, value) in [
            ("cpus", resources.cpus.to_string()),
            ("memory", format!("{}M", resources.memory_mib)),
            ("disk", format!("{}G", resources.disk_gib)),
        ] {
            self.command(
                vec!["set".into(), format!("local.{}.{key}={value}", self.name)],
                None,
                60,
            )
            .await?;
        }
        Ok(())
    }
    /// Remove only this device's receipt-owned, stopped instance. Inventory is
    /// authoritative on retries after a crash between deletion and bookkeeping.
    pub async fn remove(&self) -> Result<()> {
        let owned = self.has_receipt()?;
        let info = self.info().await?;
        if info.installed {
            anyhow::ensure!(
                owned,
                "The worker has no ownership receipt; it has been left untouched"
            );
            anyhow::ensure!(
                info.stopped,
                "The VM runtime must confirm the worker is stopped before deleting its disk"
            );
            self.command(
                if self.is_lima() {
                    vec!["delete".into(), self.name.clone()]
                } else {
                    vec!["delete".into(), "--purge".into(), self.name.clone()]
                },
                None,
                60,
            )
            .await?;
            anyhow::ensure!(
                !self.info().await?.installed,
                "The VM runtime has not confirmed that the worker disk was removed"
            );
        }
        if owned {
            std::fs::remove_file(self.receipt.as_ref().context("No VM receipt path")?)?;
        }
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
        if self.is_lima() {
            let config = crate::lima::configuration(&self.device_id, resources, &files)?;
            let mut file = tempfile::Builder::new()
                .suffix(".yaml")
                .tempfile_in(directory)?;
            serde_json::to_writer(file.as_file_mut(), &config)?;
            file.as_file().sync_all()?;
            self.record_creation()?;
            self.command(
                vec![
                    "start".into(),
                    format!("--name={}", self.name),
                    "--timeout=15m".into(),
                    file.path().to_string_lossy().into(),
                ],
                None,
                1200,
            )
            .await?;
            return self.verify_owner().await;
        }
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
        self.renew_lease().await?;
        let configuration = async {
            self.guest(
                &[
                    "sudo",
                    "python3",
                    "-c",
                    include_str!("../../../guest/install_guest.py"),
                ],
                Some(serde_json::to_vec(
                    &json!({"deviceId":self.device_id,"files":crate::guest_files()}),
                )?),
                30,
            )
            .await?;
            self.guest(
                &[
                    "sudo",
                    "python3",
                    "/usr/local/lib/nodeharbor/configure_worker.py",
                ],
                Some(serde_json::to_vec(&bootstrap)?),
                600,
            )
            .await
        };
        tokio::pin!(configuration);
        let period = std::time::Duration::from_secs(30);
        let mut renewals = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Keep renewal in this operation's lifetime. Cancelling supervision
        // drops both futures; no background task can preserve an abandoned lease.
        loop {
            tokio::select! {
                result = &mut configuration => return result.map(|_| ()),
                _ = renewals.tick() => self.renew_lease().await?,
            }
        }
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
    pub async fn workloads(&self, system_pod_uids: &[String]) -> Result<Vec<Value>> {
        self.verify_owner().await?;
        let output = self
            .guest(
                &["sudo", "/usr/local/bin/k3s", "crictl", "pods", "-o", "json"],
                None,
                20,
            )
            .await?;
        let value: Value = serde_json::from_str(&output)?;
        let items = value["items"]
            .as_array()
            .context("The worker returned no workload inventory")?;
        let mut workloads = Vec::new();
        for pod in items {
            let state = pod["state"].as_str().context("A workload has no state")?;
            anyhow::ensure!(
                ["SANDBOX_READY", "SANDBOX_NOTREADY"].contains(&state),
                "A workload has an unknown state"
            );
            if state != "SANDBOX_READY" {
                continue;
            }
            let name = pod["metadata"]["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("A workload has no name")?;
            let namespace = pod["metadata"]["namespace"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("A workload has no namespace")?;
            let system = pod["metadata"]["uid"]
                .as_str()
                .is_some_and(|uid| system_pod_uids.iter().any(|known| known == uid));
            if namespace != "kube-system" && !system {
                workloads.push(json!({"name":name,"namespace":namespace,"state":"running"}));
            }
        }
        Ok(workloads)
    }
}
