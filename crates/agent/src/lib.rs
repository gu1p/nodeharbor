//! Local contribution supervision. VM names are owned by enrolled device IDs.
pub use nodeharbor_core::{worker_transition, WorkerAction, WorkerInput};
pub mod storage_lifecycle;

pub fn managed_vm_name(device_id: &str) -> anyhow::Result<String> {
    let id = uuid::Uuid::parse_str(device_id)?;
    Ok(format!("nodeharbor-{}", id.simple()))
}
mod store;
pub use store::{Configuration, Store};
pub mod lima_storage;
mod runtime_image;
pub mod runtime_platform;
mod runtime_storage;
pub mod storage;
pub mod storage_layout;

mod vm;
pub use vm::{CommandOutput, MultipassRunner, Runner, Vm, VmInfo, WorkloadInventory};
mod lima;
pub use lima::{LimaRunner, VmProvider};
pub mod activity;
pub mod process;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}
pub type ProgressSink = std::sync::Arc<dyn Fn(OutputStream, &str) + Send + Sync>;

mod agent;
mod observe;
pub use agent::{Agent, Snapshot};

mod guest;
pub use guest::guest_files;

#[derive(Debug, PartialEq, Eq)]
pub enum CloseBehavior {
    Hide,
    DrainThenExit,
    Exit,
}

pub fn close_behavior(background: bool, active: bool, explicit_quit: bool) -> CloseBehavior {
    if background && !explicit_quit {
        CloseBehavior::Hide
    } else if active {
        CloseBehavior::DrainThenExit
    } else {
        CloseBehavior::Exit
    }
}

#[cfg(test)]
mod runtime_storage_native_tests;
