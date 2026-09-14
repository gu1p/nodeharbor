use anyhow::{Context, Result};
use nodeharbor_core::{Qualification, Resources};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct HealthSample {
    pub at: i64,
    pub ready: bool,
    pub rtt_ms: Option<f64>,
}
impl HealthSample {
    fn healthy(&self) -> bool {
        self.ready && self.rtt_ms.is_some_and(|rtt| rtt.is_finite() && rtt >= 0.0)
    }
}

/// Thirty-second observation slots make missing measurements count as failures.
/// Repeated measurements cannot manufacture time or improve an unhealthy slot.
pub fn assess_health(samples: &[HealthSample], now: i64) -> Qualification {
    let end = now.div_euclid(30) * 30;
    let mut slots = BTreeMap::<i64, HealthSample>::new();
    for sample in samples
        .iter()
        .filter(|s| s.at <= now && s.at >= end - 86400)
    {
        let slot = sample.at.div_euclid(30) * 30;
        slots
            .entry(slot)
            .and_modify(|existing| {
                existing.ready &= sample.healthy();
                existing.rtt_ms = existing.rtt_ms.zip(sample.rtt_ms).map(|(a, b)| a.max(b));
            })
            .or_insert_with(|| sample.clone());
    }
    let current = slots
        .last_key_value()
        .is_some_and(|(_, sample)| sample.healthy() && now - sample.at <= 90);
    let latest = slots.last_key_value().map(|(at, _)| *at).unwrap_or(end);
    let ci_samples: Option<Vec<_>> = (latest - 600..=latest)
        .step_by(30)
        .map(|at| slots.get(&at))
        .collect();
    let ci = current
        && ci_samples.is_some_and(|samples| {
            if !samples.iter().all(|sample| sample.healthy()) {
                return false;
            }
            let mut rtts: Vec<_> = samples.iter().filter_map(|sample| sample.rtt_ms).collect();
            rtts.sort_by(f64::total_cmp);
            let p95 = rtts[(rtts.len() * 95).div_ceil(100) - 1];
            // A complete window still requires every independent check to pass.
            // Apply the shared latency percentile, rather than a maximum that
            // turns an isolated slow response into ten minutes of lost admission.
            nodeharbor_core::qualify(&nodeharbor_core::HealthWindow {
                healthy_seconds: 600,
                availability: 1.0,
                p95_rtt_ms: p95,
                loss: 0.0,
                ready: true,
                age_seconds: now.saturating_sub(samples.last().unwrap().at) as u64,
            })
            .ci
        });
    let expected = 2881.0; // Both endpoints of one complete 24-hour observation window.
    let good: Vec<_> = slots.values().filter(|sample| sample.healthy()).collect();
    let availability = good.len() as f64 / expected;
    let mut rtts: Vec<_> = good.iter().filter_map(|s| s.rtt_ms).collect();
    rtts.sort_by(f64::total_cmp);
    let p95 = rtts
        .get((rtts.len() * 95).div_ceil(100).saturating_sub(1))
        .copied()
        .unwrap_or(f64::INFINITY);
    let complete_window = slots.contains_key(&(end - 86400));
    let services =
        ci && complete_window && availability >= 0.995 && 1.0 - availability < 0.005 && p95 < 300.0;
    Qualification {
        ci,
        services,
        reason: if services {
            "Qualified for eligible services and CI"
        } else if ci {
            "CI ready; observing service reliability"
        } else {
            "Waiting for ten minutes of independently verified connectivity"
        }
        .into(),
    }
}

fn quantity(value: &Value) -> Result<f64> {
    let text = value.as_str().context("Node capacity is missing")?;
    for (suffix, multiplier) in [
        ("Ki", 1024.0),
        ("Mi", 1048576.0),
        ("Gi", 1073741824.0),
        ("Ti", 1099511627776.0),
        ("n", 1e-9),
        ("u", 1e-6),
        ("m", 1e-3),
        ("k", 1e3),
        ("K", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
        ("", 1.0),
    ] {
        if let Some(number) = text.strip_suffix(suffix) {
            if let Ok(number) = number.parse::<f64>() {
                let result = number * multiplier;
                anyhow::ensure!(result.is_finite() && result > 0.0, "Invalid node capacity");
                return Ok(result);
            }
        }
    }
    anyhow::bail!("Unrecognized node capacity")
}

/// The peer must come from this device's controller-managed NetBird group.
/// Kubernetes labels alone are insufficient proof of a contributed worker.
pub fn verify_worker_evidence(
    id: &str,
    architecture: &str,
    budget: &Resources,
    node: &Value,
    peer: &Value,
) -> Result<()> {
    let device = uuid::Uuid::parse_str(id)?;
    anyhow::ensure!(
        node["metadata"]["name"] == format!("nodeharbor-{}", device.simple())
            && node["metadata"]["labels"]["nodeharbor.sikalio.dev/device"] == id
            && node["metadata"]["labels"]["kubernetes.io/arch"] == architecture,
        "Worker identity or architecture does not match enrollment"
    );
    let ip: std::net::Ipv4Addr = peer["ip"]
        .as_str()
        .context("Network peer has no address")?
        .parse()?;
    anyhow::ensure!(
        peer["connected"] == true && ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]),
        "The device is not connected to the private worker network"
    );
    let addresses = node["status"]["addresses"]
        .as_array()
        .context("Node has no addresses")?;
    anyhow::ensure!(
        addresses
            .iter()
            .any(|a| a["type"] == "InternalIP" && a["address"] == ip.to_string()),
        "Node address differs from its enrolled network peer"
    );
    let conditions = node["status"]["conditions"]
        .as_array()
        .context("Node readiness is unavailable")?;
    for (name, status) in [
        ("Ready", "True"),
        ("MemoryPressure", "False"),
        ("DiskPressure", "False"),
        ("PIDPressure", "False"),
    ] {
        anyhow::ensure!(
            conditions
                .iter()
                .any(|c| c["type"] == name && c["status"] == status),
            "Worker is not ready or is under resource pressure"
        );
    }
    anyhow::ensure!(
        !conditions
            .iter()
            .any(|c| c["type"] == "NetworkUnavailable" && c["status"] != "False"),
        "Kubernetes worker networking is unavailable"
    );
    let capacity = &node["status"]["capacity"];
    let cpu = quantity(&capacity["cpu"])?;
    let memory = quantity(&capacity["memory"])? / 1048576.0;
    let disk = quantity(&capacity["ephemeral-storage"])? / 1073741824.0;
    let allocatable_disk = quantity(&node["status"]["allocatable"]["ephemeral-storage"])
        .context("Worker allocatable storage is missing or invalid")?
        / 1073741824.0;
    anyhow::ensure!(
        cpu == f64::from(budget.cpus)
            && memory >= budget.memory_mib as f64 * 0.85
            && memory <= budget.memory_mib as f64
            && disk >= budget.disk_gib as f64 * 0.85
            && disk <= budget.disk_gib as f64
            && disk >= 10.0,
        "Actual worker capacity differs from its owner’s resource budget"
    );
    anyhow::ensure!(
        allocatable_disk <= disk,
        "Worker allocatable storage exceeds its reported capacity"
    );
    Ok(())
}
