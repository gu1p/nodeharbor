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
          "capacity":{"cpu":"2","memory":"2999999Ki","ephemeral-storage":"19000000Ki"}}});
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
