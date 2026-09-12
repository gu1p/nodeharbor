use nodeharbor_agent::{Agent, Store};
#[tokio::test]
async fn an_unenrolled_machine_cannot_enable_a_worker() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    assert!(agent.action("resume").await.is_err());
    assert!(agent.action("prepare").await.is_err());
    assert!(!agent.snapshot().await.unwrap().policy.enabled);
}
#[tokio::test]
async fn the_ui_snapshot_never_contains_the_saved_device_credential() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .update(|c| {
            c.device_token = Some("private-test-token-never-render".into());
            c.controller_url = Some("https://example.com".into());
            Ok(())
        })
        .unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    let snapshot = serde_json::to_string(&agent.snapshot().await.unwrap()).unwrap();
    assert!(!snapshot.contains("private-test-token-never-render"));
}
#[tokio::test]
async fn an_invalid_resource_change_preserves_the_previous_settings() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    let before = agent.snapshot().await.unwrap();
    let mut policy = before.policy.clone();
    policy.resources.memory_mib = u64::MAX;
    assert!(agent.save_policy(policy).await.is_err());
    assert_eq!(
        agent.snapshot().await.unwrap().policy.resources,
        before.policy.resources
    );
}

#[tokio::test]
async fn an_incomplete_preparation_cannot_enable_sharing() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent
        .store
        .update(|c| {
            c.device_token = Some("test-device-token".into());
            c.vm_created = true;
            Ok(())
        })
        .unwrap();
    assert!(agent.action("resume").await.is_err());
    assert!(!agent.snapshot().await.unwrap().policy.enabled);
}
