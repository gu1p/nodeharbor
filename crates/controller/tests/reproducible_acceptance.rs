//! End-to-end controller scenarios on private loopback listeners and disposable
//! SQLite state. Infrastructure and time are explicit test doubles, never a
//! production configuration option. No existing deployment is contacted.
#[path = "support/acceptance.rs"]
mod support;
use serde_json::json;
use support::Fixture;

#[tokio::test]
async fn enrollment_authentication() {
    let fixture = Fixture::start().await;
    assert_eq!(
        fixture.post("/enrollment-codes", None, json!({})).await.0,
        401
    );
    for (platform, architecture) in [
        ("linux", "amd64"),
        ("macos", "arm64"),
        ("windows", "amd64"),
        ("android", "arm64"),
    ] {
        let code = fixture.code().await;
        let payload = json!({"code":code,"name":"Disposable acceptance worker","platform":platform,"architecture":architecture});
        let (status, owner) = fixture.post("/enroll", None, payload.clone()).await;
        assert_eq!(status, 201);
        assert_eq!(fixture.post("/enroll", None, payload).await.0, 401);
        assert_eq!(
            fixture
                .post("/device/bootstrap", Some("invalid"), json!({}))
                .await
                .0,
            401
        );
        let token = owner["token"].as_str().unwrap();
        assert_eq!(
            fixture
                .post("/enrollment-codes", Some(token), json!({}))
                .await
                .0,
            401
        );
        let (status, grant) = fixture
            .post(
                "/device/bootstrap",
                Some(token),
                json!({"deviceId":"another-owner"}),
            )
            .await;
        assert_eq!(status, 200);
        assert_eq!(grant["deviceId"], owner["deviceId"]);
        assert!(!grant.to_string().contains(&fixture.admin));
    }
}

#[tokio::test]
async fn preparation_failure_recovery() {
    let mut fixture = Fixture::start().await;
    let owner = fixture.enroll().await;
    fixture.infrastructure.fail_bootstrap(true);
    assert_eq!(fixture.control(&owner, "bootstrap").await.0, 503);
    fixture.heartbeat(&owner, "error", false, 0).await;
    fixture.observe(&owner, 0).await;
    assert!(!fixture.eligible(&owner).await);
    fixture.restart().await;
    assert!(!fixture.eligible(&owner).await);
    fixture.infrastructure.fail_bootstrap(false);
    assert_eq!(fixture.control(&owner, "bootstrap").await.0, 200);
    fixture.heartbeat(&owner, "paused", false, 0).await;
    fixture.observe(&owner, 30).await;
    assert!(!fixture.eligible(&owner).await);
}

#[tokio::test]
async fn qualification_and_owner_controls() {
    let fixture = Fixture::start().await;
    let owner = fixture.enroll().await;
    for seconds in (0..600).step_by(30) {
        fixture.heartbeat(&owner, "sharing", true, 0).await;
        fixture.observe(&owner, seconds).await;
        assert!(
            !fixture.eligible(&owner).await,
            "Incomplete observation windows cannot admit work"
        );
    }
    fixture.heartbeat(&owner, "sharing", true, 0).await;
    fixture.observe(&owner, 600).await;
    assert!(fixture.eligible(&owner).await);
    assert!(fixture.infrastructure.can_schedule(&owner));
    fixture.heartbeat(&owner, "paused", false, 0).await;
    fixture.observe(&owner, 630).await;
    assert!(!fixture.eligible(&owner).await);
    assert!(!fixture.infrastructure.can_schedule(&owner));
    fixture.heartbeat(&owner, "sharing", true, 0).await;
    fixture.infrastructure.fail_probe(true);
    fixture.observe(&owner, 660).await;
    assert!(!fixture.eligible(&owner).await);
}

#[tokio::test]
async fn missing_storage_and_restart() {
    let mut fixture = Fixture::start().await;
    let owner = fixture.enroll().await;
    fixture.qualify(&owner).await;
    assert!(fixture.eligible(&owner).await);
    fixture.heartbeat(&owner, "error", false, 1).await;
    fixture.observe(&owner, 630).await;
    assert!(!fixture.eligible(&owner).await);
    assert!(!fixture.infrastructure.can_schedule(&owner));
    fixture.restart().await;
    assert!(!fixture.eligible(&owner).await);
    fixture.heartbeat(&owner, "sharing", true, 1).await;
    fixture.observe(&owner, 660).await;
    assert!(
        !fixture.eligible(&owner).await,
        "Returned storage requires fresh qualification"
    );
    let stale = fixture
        .post(
            "/heartbeat",
            Some(owner.token()),
            json!({"state":"sharing","permitted":true,"storageGeneration":0}),
        )
        .await;
    assert_eq!(stale.0, 400);
}

#[tokio::test]
async fn revocation_and_reenrollment() {
    let mut fixture = Fixture::start().await;
    let owner = fixture.enroll().await;
    fixture.qualify(&owner).await;
    fixture.infrastructure.fail_revoke(true);
    assert_eq!(
        fixture
            .post(
                &format!("/devices/{}/revoke", owner.id()),
                Some(&fixture.admin),
                json!({})
            )
            .await
            .0,
        503
    );
    assert_eq!(fixture.control(&owner, "bootstrap").await.0, 401);
    fixture.restart().await;
    assert_eq!(fixture.control(&owner, "bootstrap").await.0, 401);
    fixture.infrastructure.fail_revoke(false);
    fixture.state.retry_revocations().await.unwrap();
    assert!(!fixture.infrastructure.can_schedule(&owner));
    let replacement = fixture.enroll().await;
    assert_ne!(owner.id(), replacement.id());
    assert!(!fixture.eligible(&replacement).await);
    assert_eq!(fixture.control(&replacement, "bootstrap").await.0, 200);
}
