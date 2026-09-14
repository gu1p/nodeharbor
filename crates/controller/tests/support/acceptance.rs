//! Test-only infrastructure. Never linked into a production constructor.
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use nodeharbor_controller::{router, Cluster, DeviceIdentity, HealthBackend, Reconciler, State};
use nodeharbor_core::Resources;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::IntoFuture,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub struct Owner(Value);
impl Owner {
    pub fn id(&self) -> &str {
        self.0["deviceId"].as_str().unwrap()
    }
    pub fn token(&self) -> &str {
        self.0["token"].as_str().unwrap()
    }
}

#[derive(Default)]
pub struct Infrastructure {
    bootstrap_failure: AtomicBool,
    probe_failure: AtomicBool,
    revoke_failure: AtomicBool,
    placements: Mutex<HashMap<String, (bool, bool)>>,
}
impl Infrastructure {
    pub fn fail_bootstrap(&self, value: bool) {
        self.bootstrap_failure.store(value, Ordering::SeqCst);
    }
    pub fn fail_probe(&self, value: bool) {
        self.probe_failure.store(value, Ordering::SeqCst);
    }
    pub fn fail_revoke(&self, value: bool) {
        self.revoke_failure.store(value, Ordering::SeqCst);
    }
    pub fn can_schedule(&self, owner: &Owner) -> bool {
        self.placements.lock().unwrap().get(owner.id()) == Some(&(true, true))
    }
}
#[async_trait]
impl Cluster for Infrastructure {
    async fn bootstrap(&self, owner: &DeviceIdentity) -> anyhow::Result<Value> {
        anyhow::ensure!(
            !self.bootstrap_failure.load(Ordering::SeqCst),
            "Simulated preparation failure"
        );
        Ok(json!({"deviceId":owner.id,"nodeName":owner.node_name(),"testInfrastructure":true}))
    }
    async fn drain(&self, owner: &DeviceIdentity) -> anyhow::Result<()> {
        self.placements
            .lock()
            .unwrap()
            .insert(owner.id.clone(), (false, false));
        Ok(())
    }
    async fn resume(&self, _: &DeviceIdentity) -> anyhow::Result<()> {
        Ok(())
    }
    async fn revoke(&self, owner: &DeviceIdentity) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.revoke_failure.load(Ordering::SeqCst),
            "Simulated cleanup outage"
        );
        self.placements.lock().unwrap().remove(&owner.id);
        Ok(())
    }
}
#[async_trait]
impl HealthBackend for Infrastructure {
    async fn observe(&self, _: &DeviceIdentity, _: &Resources) -> anyhow::Result<f64> {
        anyhow::ensure!(
            !self.probe_failure.load(Ordering::SeqCst),
            "Simulated independent probe failure"
        );
        Ok(40.0)
    }
    async fn place(
        &self,
        owner: &DeviceIdentity,
        ci: bool,
        _: bool,
        accepting: bool,
    ) -> anyhow::Result<()> {
        self.placements
            .lock()
            .unwrap()
            .insert(owner.id.clone(), (ci, accepting));
        Ok(())
    }
}

pub struct Fixture {
    root: tempfile::TempDir,
    pub state: State,
    pub admin: String,
    pub infrastructure: Arc<Infrastructure>,
    base: String,
    epoch: i64,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    client: reqwest::Client,
}
impl Fixture {
    pub async fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let admin = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let infrastructure = Arc::new(Infrastructure::default());
        let state = State::open(
            &format!(
                "sqlite://{}",
                root.path().join("controller.sqlite").display()
            ),
            &admin,
        )
        .await
        .unwrap()
        .with_cluster(infrastructure.clone());
        let (base, task) = Self::serve(&state).await;
        Self {
            root,
            state,
            admin,
            infrastructure,
            base,
            task,
            epoch: Utc::now().timestamp().div_euclid(30) * 30,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
        }
    }
    async fn serve(state: &State) -> (String, tokio::task::JoinHandle<std::io::Result<()>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(axum::serve(listener, router(state.clone())).into_future());
        (base, task)
    }
    pub async fn restart(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
        self.state.db.close().await;
        self.state = State::open(
            &format!(
                "sqlite://{}",
                self.root.path().join("controller.sqlite").display()
            ),
            &self.admin,
        )
        .await
        .unwrap()
        .with_cluster(self.infrastructure.clone());
        (self.base, self.task) = Self::serve(&self.state).await;
    }
    pub async fn post(&self, path: &str, token: Option<&str>, body: Value) -> (u16, Value) {
        let mut request = self.client.post(format!("{}{path}", self.base)).json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
    pub async fn code(&self) -> Value {
        let (status, body) = self
            .post("/enrollment-codes", Some(&self.admin), json!({}))
            .await;
        assert_eq!(status, 201);
        body["code"].clone()
    }
    pub async fn enroll(&self) -> Owner {
        let (status, value) = self.post("/enroll", None,
            json!({"code":self.code().await,"name":"Disposable worker","platform":"linux","architecture":"amd64"})).await;
        assert_eq!(status, 201);
        Owner(value)
    }
    pub async fn control(&self, owner: &Owner, action: &str) -> (u16, Value) {
        self.post(&format!("/device/{action}"), Some(owner.token()), json!({}))
            .await
    }
    pub async fn heartbeat(&self, owner: &Owner, state: &str, permitted: bool, generation: u64) {
        let (status, _) = self.post("/heartbeat", Some(owner.token()), json!({"state":state,
            "reason":if state=="error" {"Selected storage is unavailable"} else {"Acceptance scenario"},
            "permitted":permitted,"allowCi":true,"allowServices":false,
            "resources":Resources::default(),"storageGeneration":generation,
            "eligibleCi":true})).await;
        assert_eq!(status, 200);
    }
    pub async fn observe(&self, owner: &Owner, seconds: i64) {
        let time = Utc.timestamp_opt(self.epoch + seconds, 0).single().unwrap();
        // Control only the heartbeat's clock. All health samples and admission
        // decisions are produced by the unmodified production reconciler.
        sqlx::query("UPDATE devices SET last_seen=? WHERE id=?")
            .bind(time.to_rfc3339())
            .bind(owner.id())
            .execute(&self.state.db)
            .await
            .unwrap();
        Reconciler::new(self.state.clone(), self.infrastructure.clone())
            .tick_at(time)
            .await
            .unwrap();
    }
    pub async fn eligible(&self, owner: &Owner) -> bool {
        sqlx::query_scalar("SELECT eligible_ci FROM devices WHERE id=?")
            .bind(owner.id())
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }
    pub async fn qualify(&self, owner: &Owner) {
        for seconds in (0..=600).step_by(30) {
            self.heartbeat(owner, "sharing", true, 0).await;
            self.observe(owner, seconds).await;
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
