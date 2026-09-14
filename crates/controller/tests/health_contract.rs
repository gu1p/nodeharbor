use nodeharbor_controller::{assess_health, verify_worker_evidence, HealthSample};
use nodeharbor_core::Resources;
use serde_json::json;

fn samples(start: i64, end: i64) -> Vec<HealthSample> {
    (start..=end)
        .step_by(30)
        .map(|at| HealthSample {
            at,
            ready: true,
            rtt_ms: Some(35.0),
        })
        .collect()
}
#[test]
fn ci_requires_ten_observed_minutes_and_services_a_full_day() {
    assert!(!assess_health(&samples(0, 570), 570).ci);
    let ten_minutes = assess_health(&samples(0, 600), 600);
    assert!(ten_minutes.ci);
    assert!(!ten_minutes.services);
    assert!(assess_health(&samples(0, 86400), 86400).services);
}
#[test]
fn ci_uses_the_shared_p95_policy_without_discarding_failed_or_missing_observations() {
    let mut history = samples(0, 600);
    for sample in &mut history {
        sample.rtt_ms = Some(275.0);
    }
    history[19].rtt_ms = Some(507.0);
    let policy = nodeharbor_core::HealthWindow {
        healthy_seconds: 600,
        availability: 1.0,
        p95_rtt_ms: 275.0,
        loss: 0.0,
        ready: true,
        age_seconds: 0,
    };
    assert!(nodeharbor_core::qualify(&policy).ci);
    assert!(
        assess_health(&history, 600).ci,
        "One slow response must follow the shared p95 policy"
    );
    history[18].rtt_ms = Some(501.0);
    assert!(
        !assess_health(&history, 600).ci,
        "A p95 above 500 ms must prevent admission"
    );
    history[18].rtt_ms = Some(500.0);
    assert!(assess_health(&history, 600).ci);
    history[19].ready = false;
    assert!(
        !assess_health(&history, 600).ci,
        "A failed connectivity check still prevents admission"
    );
    history.remove(19);
    assert!(
        !assess_health(&history, 600).ci,
        "Missing time cannot be supplied by a percentile"
    );
}
#[test]
fn controller_downtime_counts_as_missing_observations_and_does_not_invent_health() {
    let mut history = samples(0, 300);
    history.extend(samples(86100, 86400));
    let result = assess_health(&history, 86400);
    assert!(!result.ci);
    assert!(!result.services);
}
#[test]
fn the_service_window_allows_a_small_past_outage_but_requires_current_health() {
    let mut history = samples(0, 86400);
    history[10].ready = false;
    history[10].rtt_ms = None;
    assert!(assess_health(&history, 86400).services);
    history.last_mut().unwrap().ready = false;
    assert!(!assess_health(&history, 86400).ci);
    assert!(!assess_health(&history, 86400).services);
}
#[test]
fn lagging_duplicate_or_nonfinite_observations_cannot_qualify_a_worker() {
    assert!(!assess_health(&samples(0, 86400), 86500).ci);
    assert!(
        !assess_health(
            &vec![
                HealthSample {
                    at: 86400,
                    ready: true,
                    rtt_ms: Some(10.0)
                };
                3000
            ],
            86400
        )
        .services
    );
    let mut history = samples(0, 86400);
    for sample in &mut history {
        sample.rtt_ms = Some(f64::NAN);
    }
    assert!(!assess_health(&history, 86400).ci);
}
#[test]
fn node_identity_network_and_actual_capacity_must_agree_with_the_enrolled_vm() {
    let id = "9511182e-9c48-4d20-a15b-1da8bb441386";
    let name = "nodeharbor-9511182e9c484d20a15b1da8bb441386";
    let mut node = json!({"metadata":{"name":name,"labels":{"nodeharbor.sikalio.dev/device":id,"kubernetes.io/arch":"arm64"}},
        "spec":{"podCIDR":"10.42.3.0/24"},
        "status":{"addresses":[{"type":"InternalIP","address":"100.90.1.2"}],
          "conditions":[{"type":"Ready","status":"True"},{"type":"MemoryPressure","status":"False"},{"type":"DiskPressure","status":"False"},{"type":"PIDPressure","status":"False"}],
          "capacity":{"cpu":"2","memory":"2999999Ki","ephemeral-storage":"19000000Ki"},
          "allocatable":{"ephemeral-storage":"17000000Ki"}}});
    let peer = json!({"ip":"100.90.1.2","connected":true});
    let budget = Resources {
        cpus: 2,
        memory_mib: 3072,
        disk_gib: 20,
    };
    assert!(verify_worker_evidence(id, "arm64", &budget, &node, &peer).is_ok());
    node["status"]["addresses"][0]["address"] = json!("10.50.0.2");
    assert!(verify_worker_evidence(id, "arm64", &budget, &node, &peer).is_err());
    node["status"]["addresses"][0]["address"] = json!("100.90.1.2");
    node["status"]["capacity"]["cpu"] = json!("8");
    assert!(verify_worker_evidence(id, "arm64", &budget, &node, &peer).is_err());
}

fn storage_evidence(
    capacity: serde_json::Value,
    allocatable: serde_json::Value,
) -> serde_json::Value {
    json!({"metadata":{"name":"nodeharbor-9511182e9c484d20a15b1da8bb441386","labels":{"nodeharbor.sikalio.dev/device":"9511182e-9c48-4d20-a15b-1da8bb441386","kubernetes.io/arch":"arm64"}},
        "status":{"addresses":[{"type":"InternalIP","address":"100.90.1.2"}],
          "conditions":[{"type":"Ready","status":"True"},{"type":"MemoryPressure","status":"False"},{"type":"DiskPressure","status":"False"},{"type":"PIDPressure","status":"False"}],
          "capacity":{"cpu":"2","memory":"3900000Ki","ephemeral-storage":capacity},
          "allocatable":{"ephemeral-storage":allocatable}}})
}

#[test]
fn a_worker_cannot_qualify_when_kubernetes_sees_only_the_boot_disk_instead_of_its_combined_storage()
{
    let budget = Resources {
        disk_gib: 100,
        ..Resources::default()
    };
    let result = verify_worker_evidence(
        "9511182e-9c48-4d20-a15b-1da8bb441386",
        "arm64",
        &budget,
        &storage_evidence(json!("15Gi"), json!("12Gi")),
        &json!({"ip":"100.90.1.2","connected":true}),
    );
    assert!(
        result.is_err(),
        "Healthy networking cannot qualify a worker that exposes only its boot disk"
    );
}

#[test]
fn reported_storage_capacity_has_inclusive_budget_bounds_with_filesystem_overhead() {
    let budget = Resources {
        disk_gib: 100,
        ..Resources::default()
    };
    for (capacity, accepted) in [
        ("85Gi".to_owned(), true),
        ("100Gi".to_owned(), true),
        (((85_u64 << 30) - 1).to_string(), false),
        (((100_u64 << 30) + 1).to_string(), false),
    ] {
        let result = verify_worker_evidence(
            "9511182e-9c48-4d20-a15b-1da8bb441386",
            "arm64",
            &budget,
            &storage_evidence(json!(capacity), json!("80Gi")),
            &json!({"ip":"100.90.1.2","connected":true}),
        );
        assert_eq!(result.is_ok(), accepted, "Reported capacity {capacity}");
    }
}

#[test]
fn allocatable_storage_must_be_present_positive_finite_and_no_larger_than_capacity() {
    let budget = Resources {
        disk_gib: 100,
        ..Resources::default()
    };
    for (allocatable, accepted) in [
        (json!("90Gi"), true),
        (json!("1"), true),
        (serde_json::Value::Null, false),
        (json!(0), false),
        (json!("0"), false),
        (json!("-1"), false),
        (json!("91Gi"), false),
        (json!("NaN"), false),
        (json!("inf"), false),
    ] {
        let result = verify_worker_evidence(
            "9511182e-9c48-4d20-a15b-1da8bb441386",
            "arm64",
            &budget,
            &storage_evidence(json!("90Gi"), allocatable.clone()),
            &json!({"ip":"100.90.1.2","connected":true}),
        );
        assert_eq!(
            result.is_ok(),
            accepted,
            "Reported allocatable storage {allocatable}"
        );
    }
}
