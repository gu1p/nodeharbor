use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use nodeharbor_controller::{DeviceIdentity, HealthBackend, Reconciler, State};
use nodeharbor_core::Resources;
use std::sync::{Arc, Mutex};

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
#[derive(Default)]
struct Backend {
    placements: Mutex<Vec<(bool, bool, bool)>>,
    fail: Mutex<bool>,
    rtt: Mutex<Option<f64>>,
}
#[async_trait]
impl HealthBackend for Backend {
    async fn observe(&self, _: &DeviceIdentity, _: &Resources) -> anyhow::Result<f64> {
        anyhow::ensure!(!*self.fail.lock().unwrap(), "Network probe unavailable");
        Ok(self.rtt.lock().unwrap().unwrap_or(40.0))
    }
    async fn place(
        &self,
        _: &DeviceIdentity,
        ci: bool,
        services: bool,
        accepting: bool,
    ) -> anyhow::Result<()> {
        self.placements
            .lock()
            .unwrap()
            .push((ci, services, accepting));
        Ok(())
    }
}
async fn enrolled() -> State {
    let state = State::open("sqlite::memory:", "test-admin").await.unwrap();
    sqlx::query("INSERT INTO devices(id,name,platform,architecture,token_hash,created_at,state,resources) VALUES(?, 'Laptop','macos','arm64','test-hash',?,'sharing',?)")
        .bind(ID).bind(Utc.timestamp_opt(0,0).unwrap().to_rfc3339())
        .bind(serde_json::to_string(&Resources::default()).unwrap()).execute(&state.db).await.unwrap();
    sqlx::query(
        "INSERT INTO device_policy(device_id,allow_ci,allow_services,permitted) VALUES(?,1,0,1)",
    )
    .bind(ID)
    .execute(&state.db)
    .await
    .unwrap();
    state
}
async fn seen(state: &State, now: i64) {
    sqlx::query("UPDATE devices SET last_seen=? WHERE id=?")
        .bind(Utc.timestamp_opt(now, 0).unwrap().to_rfc3339())
        .bind(ID)
        .execute(&state.db)
        .await
        .unwrap();
}
#[tokio::test]
async fn only_independent_observations_and_owner_opt_ins_enable_placement() {
    let state = enrolled().await;
    let backend = Arc::new(Backend::default());
    let reconciler = Reconciler::new(state.clone(), backend.clone());
    for now in (0..=600).step_by(30) {
        seen(&state, now).await;
        reconciler
            .tick_at(Utc.timestamp_opt(now, 0).unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        backend.placements.lock().unwrap().last(),
        Some(&(true, false, true))
    );
    sqlx::query("UPDATE device_policy SET allow_ci=0 WHERE device_id=?")
        .bind(ID)
        .execute(&state.db)
        .await
        .unwrap();
    seen(&state, 630).await;
    reconciler
        .tick_at(Utc.timestamp_opt(630, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        backend.placements.lock().unwrap().last(),
        Some(&(false, false, true))
    );
}
#[tokio::test]
async fn saved_observations_apply_the_shared_latency_percentile_to_actual_admission() {
    let state = enrolled().await;
    let backend = Arc::new(Backend::default());
    let reconciler = Reconciler::new(state.clone(), backend.clone());
    for now in (0..=600).step_by(30) {
        *backend.rtt.lock().unwrap() = Some(if now == 570 { 507.0 } else { 275.0 });
        seen(&state, now).await;
        reconciler
            .tick_at(Utc.timestamp_opt(now, 0).unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        backend.placements.lock().unwrap().last(),
        Some(&(true, false, true))
    );
    *backend.rtt.lock().unwrap() = Some(501.0);
    seen(&state, 630).await;
    reconciler
        .tick_at(Utc.timestamp_opt(630, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        backend.placements.lock().unwrap().last(),
        Some(&(false, false, true))
    );
}
#[tokio::test]
async fn stale_or_remotely_paused_workers_lose_existing_eligibility() {
    let state = enrolled().await;
    let backend = Arc::new(Backend::default());
    let reconciler = Reconciler::new(state.clone(), backend.clone());
    seen(&state, 0).await;
    sqlx::query("UPDATE devices SET eligible_ci=1,eligible_services=1 WHERE id=?")
        .bind(ID)
        .execute(&state.db)
        .await
        .unwrap();
    reconciler
        .tick_at(Utc.timestamp_opt(120, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        backend.placements.lock().unwrap().last(),
        Some(&(false, false, false))
    );
    let ci: bool = sqlx::query_scalar("SELECT eligible_ci FROM devices WHERE id=?")
        .bind(ID)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert!(!ci);
    seen(&state, 150).await;
    sqlx::query("UPDATE devices SET remote_paused=1 WHERE id=?")
        .bind(ID)
        .execute(&state.db)
        .await
        .unwrap();
    reconciler
        .tick_at(Utc.timestamp_opt(150, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        backend.placements.lock().unwrap().last(),
        Some(&(false, false, false))
    );
}
