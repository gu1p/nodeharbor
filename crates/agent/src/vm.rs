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

#[derive(Default)]
pub struct WorkloadInventory {
    pub workloads: Vec<Value>,
    pub system_components: Vec<Value>,
}
impl WorkloadInventory {
    pub fn into_visible(mut self) -> Vec<Value> {
        self.workloads.extend(self.system_components);
        self.workloads
    }
}
#[async_trait]
pub trait Runner: Send + Sync {
    async fn replacement_space(
        &self,
        _name: &str,
    ) -> Result<crate::storage_lifecycle::ReplacementSpace> {
        anyhow::bail!("Cannot verify the runtime's storage volume; source storage is preserved")
    }
    async fn stream(
        &self,
        _args: &[String],
        _input: Option<std::fs::File>,
        _output: Option<std::fs::File>,
        _limit: u64,
    ) -> Result<()> {
        anyhow::bail!("This runtime does not implement verified worker backup streaming")
    }
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
    async fn replacement_space(
        &self,
        name: &str,
    ) -> Result<crate::storage_lifecycle::ReplacementSpace> {
        #[cfg(not(windows))]
        {
            let _ = name;
            anyhow::bail!("Single-disk replacement space inspection requires Windows with Multipass Hyper-V; this legacy host configuration is unsupported")
        }
        #[cfg(windows)]
        {
            use fs2::FileExt;
            let driver = self
                .run(&["get".into(), "local.driver".into()], None, 30)
                .await?;
            anyhow::ensure!(driver.success && driver.stdout.trim() == "hyperv", "Shrink requires verifiable Multipass Hyper-V storage; this driver is unsupported and its settings were preserved");
            let system = PathBuf::from(
                std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()),
            );
            let mut command =
                Command::new(system.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
            command.env("NODEHARBOR_REVIEW_VM", name).args(["-NoProfile", "-NonInteractive", "-Command", r#"
$ErrorActionPreference='Stop'
$storage=[Environment]::GetEnvironmentVariable('MULTIPASS_STORAGE','Machine')
if ([string]::IsNullOrWhiteSpace($storage)) { $storage=Join-Path $env:ProgramData 'Multipass' }
$instance=(Resolve-Path -LiteralPath (Join-Path $storage ('data\vault\instances\'+$env:NODEHARBOR_REVIEW_VM))).ProviderPath
$drives=@(Get-VMHardDiskDrive -VMName $env:NODEHARBOR_REVIEW_VM -ErrorAction Stop)
if ($drives.Count -ne 1) { throw 'Expected one original worker disk' }
$disk=(Resolve-Path -LiteralPath $drives[0].Path).ProviderPath
if (!$disk.StartsWith($instance.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Daemon storage configuration differs from the actual worker disk' }
@{directory=$instance;image=$disk}|ConvertTo-Json -Compress
"#]);
            let output = crate::process::run_command(
                command,
                None,
                30,
                None,
                crate::process::OutputFormat::Lines,
            )
            .await?;
            anyhow::ensure!(output.success, "Cannot inspect the original Multipass disk with the current Hyper-V permissions and storage configuration; source storage is preserved");
            let value: Value = serde_json::from_str(&output.stdout)?;
            let directory = PathBuf::from(
                value["directory"]
                    .as_str()
                    .context("Missing original VM directory")?,
            )
            .canonicalize()?;
            let image = PathBuf::from(
                value["image"]
                    .as_str()
                    .context("Missing original VM disk")?,
            )
            .canonicalize()?;
            anyhow::ensure!(
                image.starts_with(&directory),
                "Original worker disk location changed"
            );
            let file = std::fs::File::open(image)?;
            anyhow::ensure!(
                file.metadata()?.is_file(),
                "Original worker disk is not a file"
            );
            Ok(crate::storage_lifecycle::ReplacementSpace {
                directory,
                reclaimed_bytes: file.allocated_size()?,
            })
        }
    }
    async fn stream(
        &self,
        args: &[String],
        input: Option<std::fs::File>,
        output: Option<std::fs::File>,
        limit: u64,
    ) -> Result<()> {
        let mut command = Command::new(multipass_program());
        command.args(args);
        crate::process::stream_command(command, input, output, limit).await
    }
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
    pub async fn replacement_space(&self) -> Result<crate::storage_lifecycle::ReplacementSpace> {
        anyhow::ensure!(
            self.has_receipt()?,
            "Original worker ownership receipt is required for storage inspection"
        );
        self.runner.replacement_space(&self.name).await
    }
    pub async fn backup_inspect(&self) -> Result<Value> {
        self.verify_owner().await?;
        let result = self
            .guest(
                &[
                    "sudo",
                    "python3",
                    "-c",
                    include_str!("../../../guest/storage_backup.py"),
                    "inspect",
                    &self.device_id,
                ],
                None,
                1800,
            )
            .await?;
        Ok(serde_json::from_str(&result)?)
    }

    pub async fn backup_stream(
        &self,
        action: &str,
        input: Option<std::fs::File>,
        output: Option<std::fs::File>,
        limit: u64,
    ) -> Result<()> {
        anyhow::ensure!(
            matches!(action, "backup" | "verify" | "restore"),
            "Unsupported backup operation"
        );
        self.verify_owner().await?;
        let mut args: Vec<String> = if self.is_lima() {
            vec!["shell".into(), "--workdir=/".into(), self.name.clone()]
        } else {
            vec!["exec".into(), self.name.clone(), "--".into()]
        };
        args.extend(
            [
                "sudo",
                "python3",
                "-c",
                include_str!("../../../guest/storage_backup.py"),
                action,
                &self.device_id,
            ]
            .into_iter()
            .map(str::to_owned),
        );
        self.with_owner_lease(self.runner.stream(&args, input, output, limit))
            .await
    }

    /// Scope renewal to the supervising future, including host-side verification.
    /// Dropping this future on Pause/Stop also drops renewal; a rejected lease
    /// prevents the caller from advancing the durable maintenance phase.
    pub async fn with_owner_lease<T>(
        &self,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        self.renew_lease().await?;
        tokio::pin!(work);
        let period = std::time::Duration::from_secs(30);
        let mut renewals = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = renewals.tick() => self.renew_lease().await?,
                result = &mut work => return result,
            }
        }
    }

    pub async fn replacement_capacity(&self, total_gib: u64) -> Result<()> {
        self.verify_owner().await?;
        let result = self.guest(&["sudo", "python3", "-c", "import pathlib,subprocess,sys; root='/var/lib/nodeharbor/storage/k3s' if pathlib.Path('/etc/nodeharbor/storage-state.json').exists() else '/var/lib/rancher/k3s'; subprocess.run(['/usr/local/bin/k3s','kubectl','--kubeconfig',root+'/agent/kubelet.kubeconfig','get','node',sys.argv[1],'-o','json'],check=True)", &crate::managed_vm_name(&self.device_id)?], None, 60).await?;
        let node: Value = serde_json::from_str(&result)?;
        let capacity = &node["status"]["capacity"]["ephemeral-storage"];
        let allocatable = &node["status"]["allocatable"]["ephemeral-storage"];
        fn bytes(value: &Value) -> Result<f64> {
            let value = value
                .as_str()
                .context("Kubernetes did not report worker storage capacity")?;
            for (suffix, scale) in [
                ("Ki", 1024.),
                ("Mi", 1048576.),
                ("Gi", 1073741824.),
                ("Ti", 1099511627776.),
                ("m", 0.001),
                ("k", 1000.),
                ("K", 1000.),
                ("M", 1e6),
                ("G", 1e9),
                ("T", 1e12),
            ] {
                if let Some(number) = value.strip_suffix(suffix) {
                    return Ok(number.parse::<f64>()? * scale);
                }
            }
            Ok(value.parse()?)
        }
        let size = bytes(capacity)?;
        let usable = bytes(allocatable)?;
        anyhow::ensure!(
            size.is_finite()
                && usable.is_finite()
                && size >= total_gib as f64 * (1u64 << 30) as f64 * 0.75
                && size <= total_gib as f64 * (1u64 << 30) as f64
                && usable > 0.
                && usable <= size,
            "Kubernetes has not confirmed the replacement storage capacity"
        );
        Ok(())
    }

    pub async fn reset_restored_credentials(&self) -> Result<()> {
        self.verify_owner().await?;
        self.guest(&["sudo", "python3", "-c", "from pathlib import Path\np=Path('/var/lib/rancher/k3s/agent')\nfor name in ('client-kubelet.crt','client-kubelet.key','serving-kubelet.crt','serving-kubelet.key','client-kube-proxy.crt','client-kube-proxy.key','client-k3s-controller.crt','client-k3s-controller.key','kubelet.kubeconfig','kubeproxy.kubeconfig','k3scontroller.kubeconfig'):\n (p/name).unlink(missing_ok=True)\nPath('/etc/rancher/node/password').unlink(missing_ok=True)"], None, 60).await?;
        Ok(())
    }
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
        self.resize_resources(resources, true).await
    }
    pub async fn resize_compute(&self, resources: &nodeharbor_core::Resources) -> Result<()> {
        self.resize_resources(resources, false).await
    }
    async fn resize_resources(
        &self,
        resources: &nodeharbor_core::Resources,
        disk: bool,
    ) -> Result<()> {
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
            let mut args = vec![
                "edit".into(),
                self.name.clone(),
                format!("--cpus={}", resources.cpus),
                format!("--memory={}", resources.memory_mib as f64 / 1024.0),
            ];
            if disk {
                args.push(format!("--disk={}", resources.disk_gib));
            }
            self.command(args, None, 60).await?;
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
        self.create_inner(resources, directory, files, None).await
    }
    pub async fn create_with_storage(
        &self,
        resources: &nodeharbor_core::Resources,
        directory: &Path,
        files: Value,
        locations: &[crate::storage::Location],
        pool_id: &str,
        generation: u64,
    ) -> Result<()> {
        anyhow::ensure!(
            self.is_lima(),
            "This runtime does not support pooled worker disks"
        );
        let configuration = crate::lima::configuration_with_storage(
            &self.device_id,
            resources,
            &files,
            locations,
            pool_id,
            generation,
        )?;
        self.create_inner(resources, directory, files, Some(configuration))
            .await
    }
    async fn create_inner(
        &self,
        resources: &nodeharbor_core::Resources,
        directory: &Path,
        files: Value,
        configuration: Option<Value>,
    ) -> Result<()> {
        anyhow::ensure!(
            !self.info().await?.installed,
            "The worker VM already exists; refusing to replace it"
        );
        if self.is_lima() {
            let config = configuration.map(Ok).unwrap_or_else(|| {
                crate::lima::configuration(&self.device_id, resources, &files)
            })?;
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
    pub fn storage_adapter(&self, directory: &Path) -> Result<crate::lima_storage::LimaStorage> {
        crate::lima_storage::LimaStorage::new(
            self.runner.clone(),
            directory.join("lima"),
            &self.device_id,
        )
    }
    pub async fn set_storage_attachments(
        &self,
        locations: &[crate::storage::Location],
    ) -> Result<()> {
        anyhow::ensure!(
            self.is_lima() && self.has_receipt()?,
            "The worker has no matching Lima ownership receipt"
        );
        anyhow::ensure!(
            self.info().await?.stopped,
            "Stop the worker before changing storage attachments"
        );
        let disks = json!(locations
            .iter()
            .map(|location| json!({"name":location.id,"format":false}))
            .collect::<Vec<_>>());
        let expression =
            format!(".plain = true | .ssh.overVsock = false | .additionalDisks = {disks}");
        self.command(
            vec!["edit".into(), self.name.clone(), "--set".into(), expression],
            None,
            60,
        )
        .await?;
        Ok(())
    }

    pub async fn set_storage_configuration(
        &self,
        locations: &[crate::storage::Location],
        previous: &[crate::storage::Location],
        pool_id: &str,
        generation: u64,
        migrate: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            self.is_lima() && self.has_receipt()?,
            "The worker has no matching Lima ownership receipt"
        );
        let output = self
            .command(
                vec![
                    "list".into(),
                    "--json".into(),
                    "--filter=.name == \"worker\"".into(),
                ],
                None,
                30,
            )
            .await?;
        anyhow::ensure!(
            crate::lima::info(&output)?.stopped,
            "Stop the worker before changing storage provisioning"
        );
        let instance: Value = serde_json::from_str(&output)?;
        let mut configuration = instance["config"].clone();
        anyhow::ensure!(
            configuration.is_object(),
            "Lima did not report the saved worker configuration"
        );
        crate::lima::update_storage_configuration(
            &mut configuration,
            &self.device_id,
            pool_id,
            locations,
            previous,
            generation,
            migrate,
        )?;
        let mut args = vec!["edit".into(), self.name.clone()];
        for (key, value) in [
            ("plain", json!(true)),
            ("ssh.overVsock", json!(false)),
            ("additionalDisks", configuration["additionalDisks"].clone()),
            ("provision", configuration["provision"].clone()),
            ("probes", configuration["probes"].clone()),
        ] {
            args.extend(["--set".into(), format!(".{key} = {value}")]);
        }
        self.command(args, None, 60).await?;
        Ok(())
    }
    pub async fn install_guest_files(&self) -> Result<()> {
        self.verify_owner().await?;
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
            60,
        )
        .await?;
        Ok(())
    }
    pub async fn quiesce_storage(&self) -> Result<()> {
        self.verify_owner().await?;
        let loaded = self
            .guest(
                &[
                    "systemctl",
                    "show",
                    "k3s-agent.service",
                    "--property=LoadState",
                    "--value",
                ],
                None,
                15,
            )
            .await?;
        if loaded.trim() == "loaded" {
            self.guest(
                &["sudo", "systemctl", "disable", "--now", "k3s-agent.service"],
                None,
                120,
            )
            .await?;
        }
        Ok(())
    }
    pub async fn prepare_storage_guest(&self) -> Result<()> {
        self.renew_lease().await?;
        let preparation = async {
            self.install_guest_files().await?;
            self.quiesce_storage().await?;
            self.guest(&["sudo", "sh", "-c", "set -eu; if ! command -v pvs >/dev/null || ! command -v rsync >/dev/null || ! command -v growpart >/dev/null; then export DEBIAN_FRONTEND=noninteractive; apt-get update; apt-get install -y lvm2 e2fsprogs util-linux cloud-guest-utils rsync; fi; if [ \"$(systemctl show lima-guestagent.service --property=LoadState --value)\" = loaded ]; then systemctl disable --now lima-guestagent.service; fi"], None, 600).await
        };
        tokio::pin!(preparation);
        let period = std::time::Duration::from_secs(30);
        let mut renewals = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Cancelling maintenance drops preparation and renewal together, leaving
        // the existing watchdog responsible for an abandoned worker.
        loop {
            tokio::select! {
                result = &mut preparation => return result.map(|_| ()),
                _ = renewals.tick() => self.renew_lease().await?,
            }
        }
    }
    pub async fn storage_command(&self, command: &str, request: Option<Value>) -> Result<Value> {
        anyhow::ensure!(
            self.is_lima()
                && matches!(
                    command,
                    "apply" | "activate" | "check" | "migrate" | "retire" | "restored"
                ),
            "Unsupported storage operation"
        );
        self.verify_owner().await?;
        if matches!(command, "apply" | "migrate") {
            let loaded = self
                .guest(
                    &[
                        "systemctl",
                        "show",
                        "k3s-agent.service",
                        "--property=LoadState",
                        "--value",
                    ],
                    None,
                    15,
                )
                .await?;
            if loaded.trim() == "loaded" {
                self.guest(
                    &["sudo", "systemctl", "stop", "k3s-agent.service"],
                    None,
                    120,
                )
                .await?;
            }
        }
        let input = request.as_ref().map(serde_json::to_vec).transpose()?;
        let args = [
            "sudo",
            "python3",
            "/usr/local/lib/nodeharbor/storage_pool.py",
            command,
        ];
        let operation = self.guest(&args, input, 1800);
        tokio::pin!(operation);
        let period = std::time::Duration::from_secs(30);
        let mut renewal = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        loop {
            tokio::select! {
                result = &mut operation => return Ok(serde_json::from_str(&result?)?),
                _ = renewal.tick() => self.renew_lease().await?,
            }
        }
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
        Ok(self.workload_inventory(system_pod_uids).await?.workloads)
    }
    pub async fn workload_inventory(
        &self,
        system_pod_uids: &[String],
    ) -> Result<WorkloadInventory> {
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
        let mut inventory = WorkloadInventory::default();
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
            let target = if namespace == "kube-system" || system {
                &mut inventory.system_components
            } else {
                &mut inventory.workloads
            };
            target.push(json!({"name":name,"namespace":namespace,"state":"running"}));
        }
        Ok(inventory)
    }
}
