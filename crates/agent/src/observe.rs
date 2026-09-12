use chrono::{Datelike, Timelike};
use nodeharbor_core::{Observation, Resources};
use std::path::Path;

pub fn observation(directory: &Path, allocated_disk: u64) -> Observation {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    system.refresh_cpu_all();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let free = disks
        .iter()
        .filter(|disk| directory.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space() / 1024 / 1024 / 1024)
        .unwrap_or(0);
    let (on_battery, battery_percent) = power();
    let now = chrono::Local::now();
    Observation {
        idle_seconds: idle(),
        on_battery,
        battery_percent,
        weekday: now.weekday().num_days_from_monday() as u8,
        minute: (now.hour() * 60 + now.minute()) as u16,
        resources: Resources {
            cpus: system
                .physical_core_count()
                .unwrap_or(system.cpus().len())
                .max(1)
                .min(u16::MAX as usize) as u16,
            memory_mib: system.total_memory() / 1024 / 1024,
            disk_gib: free.saturating_add(allocated_disk),
        },
    }
}
#[cfg(target_os = "linux")]
fn idle() -> Option<u64> {
    use x11rb::{connection::Connection, protocol::screensaver::ConnectionExt};
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let reply = connection.screensaver_query_info(root).ok()?.reply().ok()?;
    Some(u64::from(reply.ms_since_user_input) / 1000)
}
#[cfg(not(target_os = "linux"))]
fn idle() -> Option<u64> {
    user_idle::UserIdle::get_time()
        .ok()
        .map(|idle| idle.as_seconds())
}
fn power() -> (Option<bool>, Option<u8>) {
    let Ok(manager) = battery::Manager::new() else {
        return (None, None);
    };
    let Ok(batteries) = manager.batteries() else {
        return (None, None);
    };
    let mut found = false;
    let mut discharging = false;
    let mut level = 100;
    for result in batteries {
        let Ok(battery) = result else {
            return (None, None);
        };
        found = true;
        match battery.state() {
            battery::State::Discharging | battery::State::Empty => discharging = true,
            battery::State::Charging | battery::State::Full => {}
            _ => return (None, None),
        }
        level = level.min(
            (battery.state_of_charge().value * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8,
        );
    }
    if found {
        (Some(discharging), Some(level))
    } else {
        (Some(false), None)
    }
}
pub fn architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}
