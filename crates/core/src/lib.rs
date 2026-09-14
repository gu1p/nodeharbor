//! Shared contribution rules. Local owner preferences always bound admission.
use serde::{Deserialize, Serialize};
mod android;
pub use android::android_request;
mod updates;
pub use updates::verify_signed_update;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerInput {
    pub permitted: bool,
    pub running: bool,
    pub draining_since: Option<u64>,
    pub now: u64,
    pub drain_seconds: u32,
    pub workloads: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resources {
    pub cpus: u16,
    pub memory_mib: u64,
    pub disk_gib: u64,
}
impl Default for Resources {
    fn default() -> Self {
        Self {
            cpus: 2,
            memory_mib: 4096,
            disk_gib: 30,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleWindow {
    pub days: Vec<u8>,
    pub start_minute: u16,
    pub end_minute: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Policy {
    pub enabled: bool,
    pub resources: Resources,
    pub idle_only: bool,
    pub idle_after_minutes: u32,
    pub allow_battery: bool,
    pub min_battery_percent: u8,
    pub schedule_enabled: bool,
    pub schedule: Vec<ScheduleWindow>,
    pub start_at_login: bool,
    pub background: bool,
    pub allow_ci: bool,
    pub allow_services: bool,
    pub drain_seconds: u32,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            resources: Resources::default(),
            idle_only: false,
            idle_after_minutes: 15,
            allow_battery: false,
            min_battery_percent: 30,
            schedule_enabled: false,
            schedule: Vec::new(),
            start_at_login: false,
            background: false,
            allow_ci: true,
            allow_services: false,
            drain_seconds: 300,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub idle_seconds: Option<u64>,
    pub on_battery: Option<bool>,
    pub battery_percent: Option<u8>,
    pub weekday: u8,
    pub minute: u16,
    pub resources: Resources,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Decision {
    pub allowed: bool,
    pub reason: String,
}
fn pause(reason: impl Into<String>) -> Decision {
    Decision {
        allowed: false,
        reason: reason.into(),
    }
}
pub fn validate_policy(policy: &Policy, host: &Resources) -> Result<(), String> {
    let r = &policy.resources;
    if r.cpus == 0 || r.cpus > host.cpus.saturating_sub(1).max(1) {
        return Err("Choose a CPU allowance that leaves capacity for this computer".into());
    }
    if r.memory_mib < 2048 || r.memory_mib > host.memory_mib.saturating_sub(2048) {
        return Err("Choose at least 2 GiB RAM and leave at least 2 GiB for this computer".into());
    }
    if r.disk_gib < 15 || r.disk_gib > host.disk_gib.saturating_sub(10) {
        return Err("Choose at least 15 GiB disk and leave 10 GiB free".into());
    }
    if policy.min_battery_percent > 100
        || policy.idle_after_minutes == 0
        || policy.drain_seconds > 1800
    {
        return Err("Battery, idle, or drain settings are outside their supported range".into());
    }
    for window in &policy.schedule {
        if window.days.is_empty()
            || window.days.iter().any(|day| *day > 6)
            || window.start_minute >= 1440
            || window.end_minute >= 1440
            || window.start_minute == window.end_minute
        {
            return Err("Each schedule needs days and different start and end times".into());
        }
    }
    Ok(())
}
fn in_window(window: &ScheduleWindow, weekday: u8, minute: u16) -> bool {
    if window.start_minute < window.end_minute {
        window.days.contains(&weekday)
            && minute >= window.start_minute
            && minute < window.end_minute
    } else {
        (window.days.contains(&weekday) && minute >= window.start_minute)
            || (window.days.contains(&((weekday + 6) % 7)) && minute < window.end_minute)
    }
}
pub fn evaluate(policy: &Policy, observation: &Observation) -> Decision {
    if !policy.enabled {
        return pause("Sharing is switched off");
    }
    if let Err(reason) = validate_policy(policy, &observation.resources) {
        return pause(reason);
    }
    if !policy.allow_ci && !policy.allow_services {
        return pause("No workload types are enabled");
    }
    match observation.on_battery {
        Some(true) if !policy.allow_battery => return pause("Paused while running on battery"),
        Some(true) => match observation.battery_percent {
            Some(level) if level >= policy.min_battery_percent => {}
            Some(_) => return pause("Battery is below your chosen minimum"),
            None => return pause("Battery level is unavailable"),
        },
        None => return pause("Power information is unavailable"),
        _ => {}
    }
    if policy.idle_only {
        match observation.idle_seconds {
            Some(seconds) if seconds >= u64::from(policy.idle_after_minutes) * 60 => {}
            Some(_) => return pause("Waiting for this computer to become idle"),
            None => return pause("Idle detection is unavailable on this desktop"),
        }
    }
    if policy.schedule_enabled
        && !policy
            .schedule
            .iter()
            .any(|window| in_window(window, observation.weekday, observation.minute))
    {
        return pause("Outside your sharing schedule");
    }
    Decision {
        allowed: true,
        reason: "Your sharing rules allow work".into(),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthWindow {
    pub healthy_seconds: u64,
    pub availability: f64,
    pub p95_rtt_ms: f64,
    pub loss: f64,
    pub ready: bool,
    pub age_seconds: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Qualification {
    pub ci: bool,
    pub services: bool,
    pub reason: String,
}
pub fn qualify(health: &HealthWindow) -> Qualification {
    let healthy = health.ready
        && health.age_seconds <= 90
        && health.availability.is_finite()
        && health.p95_rtt_ms.is_finite()
        && health.loss.is_finite()
        && (0.0..=1.0).contains(&health.availability)
        && (0.0..=1.0).contains(&health.loss)
        && health.p95_rtt_ms >= 0.0;
    let ci = healthy
        && health.healthy_seconds >= 600
        && health.p95_rtt_ms <= 500.0
        && health.loss <= 0.02;
    let services = ci
        && health.healthy_seconds >= 86400
        && health.availability >= 0.995
        && health.p95_rtt_ms < 300.0
        && health.loss < 0.005;
    Qualification {
        ci,
        services,
        reason: if services {
            "Qualified for eligible services and CI"
        } else if ci {
            "CI ready; observing service reliability"
        } else {
            "Waiting for sustained healthy connectivity"
        }
        .into(),
    }
}

/// Human-readable provenance shared by every shipped executable.
pub fn build_version() -> &'static str {
    static VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("NODEHARBOR_COMMIT").unwrap_or("development")
        )
    });
    VERSION.as_str()
}
