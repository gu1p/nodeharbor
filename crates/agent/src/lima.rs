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
        if cfg!(target_os = "macos") {
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
}
impl LimaRunner {
    pub fn new(program: PathBuf, home: PathBuf) -> Self {
        Self { program, home }
    }
    pub fn bundled(directory: &Path) -> Result<Self> {
        anyhow::ensure!(
            cfg!(target_os = "macos"),
            "This worker runtime requires macOS"
        );
        let executable = std::env::current_exe()?;
        let contents = executable
            .parent()
            .and_then(Path::parent)
            .context("Cannot locate application resources")?;
        Ok(Self::new(
            contents.join("Resources/lima/bin/limactl"),
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
        let mut command = tokio::process::Command::new(&self.program);
        command
            .arg("--tty=false")
            .args(args)
            .env("LIMA_HOME", &self.home);
        #[cfg(target_os = "macos")]
        command.env("SSH", "/usr/bin/ssh");
        crate::process::run_command(
            command,
            input,
            timeout,
            progress,
            crate::process::OutputFormat::Lines,
        )
        .await
    }
}
#[async_trait]
impl Runner for LimaRunner {
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
        "minimumLimaVersion":runtime["version"],"vmType":"vz","arch":std::env::consts::ARCH,
        "cpus":resources.cpus,"memory":format!("{}MiB",resources.memory_mib),"disk":format!("{}GiB",resources.disk_gib),
        "images":[image],"networks":[{"lima":"user-v2"}],"mounts":[],
        "containerd":{"user":false,"system":false},
        "ssh":{"loadDotSSHPubKeys":false,"forwardAgent":false,"forwardX11":false},
        "portForwards":[{"guestIP":"0.0.0.0","guestIPMustBeZero":false,"guestPortRange":[1,65535],"proto":"any","ignore":true}],
        "hostResolver":{"enabled":true},"propagateProxyEnv":false,"provision":data
    }))
}
