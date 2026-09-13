use nodeharbor_core::{android_request, Observation, Policy, Resources};
use serde_json::{json, Value};

#[test]
fn android_admission_preserves_shared_owner_rules() {
    let host = Resources {
        cpus: 8,
        memory_mib: 8192,
        disk_gib: 100,
    };
    let observation = Observation {
        resources: host.clone(),
        on_battery: Some(false),
        ..Default::default()
    };
    let mut policy = Policy::default();
    let request = |policy: &Policy| {
        json!({"operation":"evaluate", "policy":policy, "observation":observation}).to_string()
    };
    let result: Value = serde_json::from_str(&android_request(&request(&policy)).unwrap()).unwrap();
    assert_eq!(result["allowed"], false);
    policy.enabled = true;
    policy.resources.memory_mib = 2048;
    let result: Value = serde_json::from_str(&android_request(&request(&policy)).unwrap()).unwrap();
    assert_eq!(result["allowed"], true);
    policy.resources.memory_mib = 8192;
    let result: Value = serde_json::from_str(&android_request(&request(&policy)).unwrap()).unwrap();
    assert_eq!(result["allowed"], false);
}

#[test]
fn android_bridge_rejects_ambiguous_or_oversized_input() {
    assert!(android_request("{}").is_err());
    assert!(android_request(r#"{"operation":"allowEverything"}"#).is_err());
    assert!(android_request(&" ".repeat(1024 * 1024 + 1)).is_err());
}

#[test]
fn android_drain_uses_shared_deadline() {
    let request = |now| {
        json!({"operation":"transition", "input":{
        "permitted":false,"running":true,"draining_since":100,"now":now,"drain_seconds":30,"workloads":2
    }}).to_string()
    };
    assert_eq!(android_request(&request(129)).unwrap(), "\"wait\"");
    assert_eq!(android_request(&request(130)).unwrap(), "\"stop\"");
}
