//! Local contribution supervision. VM names are owned by enrolled device IDs.
use serde::{Deserialize, Serialize};

pub fn managed_vm_name(device_id: &str) -> anyhow::Result<String> {
    let id = uuid::Uuid::parse_str(device_id)?;
    Ok(format!("nodeharbor-{}", id.simple()))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerInput {
    pub permitted: bool,
    pub running: bool,
    pub draining_since: Option<u64>,
    pub now: u64,
    pub drain_seconds: u32,
    pub workloads: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub enum WorkerAction {
    Start,
    Drain,
    Wait,
    Stop,
    Keep,
}
pub fn worker_transition(input: &WorkerInput) -> WorkerAction {
    if input.permitted {
        return if input.running {
            WorkerAction::Keep
        } else {
            WorkerAction::Start
        };
    }
    if !input.running {
        return WorkerAction::Keep;
    }
    match input.draining_since {
        None => WorkerAction::Drain,
        Some(since)
            if input.workloads == 0
                || input.now.saturating_sub(since) >= u64::from(input.drain_seconds) =>
        {
            WorkerAction::Stop
        }
        Some(_) => WorkerAction::Wait,
    }
}

mod store;
pub use store::{Configuration, Store};

mod vm;
pub use vm::{CommandOutput, Runner, Vm, VmInfo};
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
