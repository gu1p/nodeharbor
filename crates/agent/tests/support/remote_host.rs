//! An unprepared host with explicit capacity for protocol integration tests.
use nodeharbor_agent::{storage::Volume, Agent, CommandOutput, Runner, VmProvider};
use std::{path::Path, sync::Arc};

struct NoVm;
#[async_trait::async_trait]
impl Runner for NoVm {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(&self, _: &[String], _: Option<Vec<u8>>, _: u64) -> anyhow::Result<CommandOutput> {
        anyhow::bail!("An unprepared protocol fixture must not execute VM commands")
    }
}

pub fn unprepared_agent(directory: &Path) -> Agent {
    let root = directory.canonicalize().unwrap();
    let volume = Volume {
        available_bytes: None,
        drive_type: None,
        suggested_directory: None,
        id: "protocol-fixture-volume".into(),
        capacity_pool: "protocol-fixture-pool".into(),
        label: "Test storage".into(),
        mount_point: root.to_string_lossy().into(),
        filesystem: "apfs".into(),
        available_gib: 200,
        configured_gib: 0,
        eligible: true,
        reason: String::new(),
    };
    let agent = Agent::open_with_runner_and_volumes(&root, Arc::new(NoVm), vec![volume]).unwrap();
    agent
        .store
        .update(|config| {
            config.vm_provider = VmProvider::Lima;
            config.format_version = config.format_version.max(2);
            Ok(())
        })
        .unwrap();
    agent
}
