//! Device enrollment and fleet API. Credentials are hashed before persistence.
use axum::{
    extract::{Path, State as Extract},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::{str::FromStr, sync::Arc};
use tower_http::trace::TraceLayer;
use uuid::Uuid;
mod health;
pub use health::{assess_health, verify_worker_evidence, HealthSample};
mod reconcile;
pub use reconcile::{HealthBackend, Reconciler};
mod probe;
pub use probe::probe_router;
mod runtime;
pub use runtime::{configure_runtime, ConfiguredController};
mod network;
pub use network::ProbeConfig;
mod provision;
pub use provision::{ApiClient, ClusterConfig, Provisioner};

#[derive(Clone)]
pub struct DeviceIdentity {
    pub id: String,
    pub architecture: String,
}
impl DeviceIdentity {
    pub fn node_name(&self) -> String {
        format!("nodeharbor-{}", self.id.replace('-', ""))
    }
}

#[async_trait::async_trait]
pub trait Cluster: Send + Sync {
    async fn bootstrap(&self, device: &DeviceIdentity) -> anyhow::Result<Value>;
    async fn drain(&self, device: &DeviceIdentity) -> anyhow::Result<()>;
    async fn resume(&self, device: &DeviceIdentity) -> anyhow::Result<()>;
    async fn revoke(&self, device: &DeviceIdentity) -> anyhow::Result<()>;
}

struct ProxyAuth {
    secret_hash: String,
    emails: Vec<String>,
    origin: String,
}

#[derive(Clone)]
pub struct State {
    pub db: SqlitePool,
    admin_hash: Arc<String>,
    proxy: Option<Arc<ProxyAuth>>,
    cluster: Option<Arc<dyn Cluster>>,
    operations: Arc<tokio::sync::Mutex<()>>,
}
impl State {
    pub async fn open(database: &str, admin_token: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !admin_token.is_empty(),
            "An administrator token is required"
        );
        let options = SqliteConnectOptions::from_str(database)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(10));
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS enrollment_codes (hash TEXT PRIMARY KEY, expires_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS devices (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, platform TEXT NOT NULL, architecture TEXT NOT NULL,
                token_hash TEXT UNIQUE NOT NULL, revoked INTEGER NOT NULL DEFAULT 0,
                state TEXT NOT NULL DEFAULT 'paused', reason TEXT NOT NULL DEFAULT 'Waiting for this machine',
                last_seen TEXT, created_at TEXT NOT NULL, remote_paused INTEGER NOT NULL DEFAULT 0,
                resources TEXT, node_name TEXT, peer_id TEXT,
                eligible_ci INTEGER NOT NULL DEFAULT 0, eligible_services INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS audit (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, device_id TEXT, action TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS health_samples (device_id TEXT NOT NULL, at TEXT NOT NULL, ready INTEGER NOT NULL, rtt_ms REAL, PRIMARY KEY(device_id, at));
            CREATE TABLE IF NOT EXISTS device_policy (device_id TEXT PRIMARY KEY, allow_ci INTEGER NOT NULL DEFAULT 0, allow_services INTEGER NOT NULL DEFAULT 0, permitted INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS device_health (device_id TEXT PRIMARY KEY, reason TEXT NOT NULL, observed_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS pending_revocations (device_id TEXT PRIMARY KEY);")
            .execute(&db).await?;
        Ok(Self {
            db,
            admin_hash: Arc::new(hash(admin_token)),
            proxy: None,
            cluster: None,
            operations: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
    pub fn with_cluster(mut self, cluster: Arc<dyn Cluster>) -> Self {
        self.cluster = Some(cluster);
        self
    }
    pub async fn retry_revocations(&self) -> anyhow::Result<()> {
        let _operation = self.operations.lock().await;
        let Some(cluster) = &self.cluster else {
            return Ok(());
        };
        let rows = sqlx::query("SELECT d.id,d.architecture FROM devices d JOIN pending_revocations p ON p.device_id=d.id WHERE d.revoked=1").fetch_all(&self.db).await?;
        let mut errors = Vec::new();
        for row in rows {
            let device = DeviceIdentity {
                id: row.get("id"),
                architecture: row.get("architecture"),
            };
            match cluster.revoke(&device).await {
                Ok(()) => {
                    sqlx::query("DELETE FROM pending_revocations WHERE device_id=?")
                        .bind(&device.id)
                        .execute(&self.db)
                        .await?;
                }
                Err(error) => errors.push(format!("{}: {error}", device.id)),
            }
        }
        anyhow::ensure!(
            errors.is_empty(),
            "Device cleanup is pending: {}",
            errors.join("; ")
        );
        Ok(())
    }
    pub fn with_proxy_auth(
        mut self,
        secret: &str,
        emails: Vec<String>,
        origin: &str,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !secret.is_empty() && !emails.is_empty(),
            "A gateway secret and allowed identities are required"
        );
        let url = reqwest::Url::parse(origin)?;
        anyhow::ensure!(
            url.scheme() == "https" && url.origin().ascii_serialization() == origin,
            "The dashboard origin must be an HTTPS origin without a path"
        );
        self.proxy = Some(Arc::new(ProxyAuth {
            secret_hash: hash(secret),
            emails,
            origin: origin.into(),
        }));
        Ok(self)
    }
}
fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
#[derive(Debug)]
pub struct ApiError(StatusCode, String);
impl ApiError {
    fn unauthorized() -> Self {
        Self(
            StatusCode::UNAUTHORIZED,
            "Authentication required or enrollment expired".into(),
        )
    }
    fn bad(message: &str) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    fn internal(error: sqlx::Error) -> Self {
        tracing::error!(error=%error,"database operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The controller could not complete this request".into(),
        )
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
type ApiResult<T> = Result<T, ApiError>;
fn bearer(headers: &HeaderMap) -> ApiResult<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty())
        .ok_or_else(ApiError::unauthorized)
}
fn admin(state: &State, headers: &HeaderMap) -> ApiResult<()> {
    if bearer(headers).is_ok_and(|token| hash(token) == *state.admin_hash) {
        return Ok(());
    }
    if let Some(proxy) = &state.proxy {
        let header = |key: &str| headers.get(key).and_then(|h| h.to_str().ok()).unwrap_or("");
        if hash(header("x-nodeharbor-proxy-token")) == proxy.secret_hash
            && proxy
                .emails
                .iter()
                .any(|email| email == header("x-auth-request-email"))
        {
            if header("x-nodeharbor-request") != "1"
                || (headers.contains_key("origin") && header("origin") != proxy.origin)
            {
                return Err(ApiError(
                    StatusCode::FORBIDDEN,
                    "The dashboard request origin could not be verified".into(),
                ));
            }
            return Ok(());
        }
    }
    Err(ApiError::unauthorized())
}
async fn device_id(state: &State, headers: &HeaderMap) -> ApiResult<String> {
    sqlx::query_scalar("SELECT id FROM devices WHERE token_hash=? AND revoked=0")
        .bind(hash(bearer(headers)?))
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::unauthorized)
}
async fn audit(state: &State, device: Option<&str>, action: &str) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit (at,device_id,action) VALUES (?,?,?)")
        .bind(Utc::now().to_rfc3339())
        .bind(device)
        .bind(action)
        .execute(&state.db)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub architecture: String,
    pub state: String,
    pub reason: String,
    pub last_seen: Option<String>,
    pub eligible_ci: bool,
    pub eligible_services: bool,
    pub resources: Option<Value>,
    pub remote_paused: bool,
    pub allow_ci: bool,
    pub allow_services: bool,
    pub health_reason: String,
    pub observed_at: Option<String>,
}
async fn fleet(Extract(state): Extract<State>, headers: HeaderMap) -> ApiResult<Json<Vec<Device>>> {
    admin(&state, &headers)?;
    fleet_rows(&state).await
}
async fn device_fleet(
    Extract(state): Extract<State>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Device>>> {
    device_id(&state, &headers).await?;
    fleet_rows(&state).await
}
async fn fleet_rows(state: &State) -> ApiResult<Json<Vec<Device>>> {
    let rows=sqlx::query("SELECT d.id,d.name,d.platform,d.architecture,d.state,d.reason,d.last_seen,d.eligible_ci,d.eligible_services,d.resources,d.remote_paused,COALESCE(p.allow_ci,0) AS allow_ci,COALESCE(p.allow_services,0) AS allow_services,h.reason AS health_reason,h.observed_at FROM devices d LEFT JOIN device_policy p ON p.device_id=d.id LEFT JOIN device_health h ON h.device_id=d.id WHERE d.revoked=0 ORDER BY d.created_at")
        .fetch_all(&state.db).await.map_err(ApiError::internal)?;
    let mut devices = Vec::new();
    for row in rows {
        let last_seen: Option<String> = row.get("last_seen");
        let stale = last_seen
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_none_or(|seen| Utc::now().signed_duration_since(seen).num_seconds() > 90);
        let resources: Option<String> = row.get("resources");
        let observed_at: Option<String> = row.get("observed_at");
        let observed = observed_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|at| {
                (0..=90).contains(&Utc::now().signed_duration_since(at).num_seconds())
            });
        devices.push(Device {
            device_id: row.get("id"),
            name: row.get("name"),
            platform: row.get("platform"),
            architecture: row.get("architecture"),
            state: if stale {
                "offline".into()
            } else {
                row.get("state")
            },
            reason: if stale {
                "Waiting for this machine to reconnect".into()
            } else {
                row.get("reason")
            },
            last_seen,
            eligible_ci: !stale && observed && row.get::<bool, _>("eligible_ci"),
            eligible_services: !stale && observed && row.get::<bool, _>("eligible_services"),
            resources: resources.and_then(|s| serde_json::from_str(&s).ok()),
            remote_paused: row.get("remote_paused"),
            allow_ci: row.get("allow_ci"),
            allow_services: row.get("allow_services"),
            health_reason: if observed && !stale {
                row.get::<Option<String>, _>("health_reason")
                    .unwrap_or_default()
            } else {
                "Waiting for a fresh network observation".into()
            },
            observed_at,
        });
    }
    Ok(Json(devices))
}
async fn create_code(
    Extract(state): Extract<State>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    admin(&state, &headers)?;
    let code = random_token();
    let expires_at = (Utc::now() + Duration::minutes(10)).to_rfc3339();
    sqlx::query("INSERT INTO enrollment_codes (hash,expires_at) VALUES (?,?)")
        .bind(hash(&code))
        .bind(&expires_at)
        .execute(&state.db)
        .await
        .map_err(ApiError::internal)?;
    audit(&state, None, "enrollment_code_created").await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"code":code,"expiresAt":expires_at})),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Enrollment {
    code: String,
    name: String,
    platform: String,
    architecture: String,
}
async fn enroll(
    Extract(state): Extract<State>,
    Json(input): Json<Enrollment>,
) -> ApiResult<impl IntoResponse> {
    if input.name.trim().is_empty() || input.name.len() > 120 {
        return Err(ApiError::bad(
            "Choose a device name between 1 and 120 characters",
        ));
    }
    if !["macos", "linux", "windows"].contains(&input.platform.as_str())
        || !["amd64", "arm64"].contains(&input.architecture.as_str())
        || (input.platform == "windows" && input.architecture != "amd64")
    {
        return Err(ApiError::bad(
            "This operating system and worker architecture are not supported",
        ));
    }
    let mut tx = state.db.begin().await.map_err(ApiError::internal)?;
    let consumed =
        sqlx::query("DELETE FROM enrollment_codes WHERE hash=? AND expires_at>? RETURNING hash")
            .bind(hash(input.code.trim()))
            .bind(Utc::now().to_rfc3339())
            .fetch_optional(&mut *tx)
            .await
            .map_err(ApiError::internal)?;
    if consumed.is_none() {
        return Err(ApiError::unauthorized());
    }
    let id = Uuid::new_v4().to_string();
    let token = random_token();
    sqlx::query("INSERT INTO devices (id,name,platform,architecture,token_hash,created_at) VALUES (?,?,?,?,?,?)")
        .bind(&id).bind(input.name.trim()).bind(&input.platform).bind(&input.architecture).bind(hash(&token)).bind(Utc::now().to_rfc3339())
        .execute(&mut *tx).await.map_err(ApiError::internal)?;
    tx.commit().await.map_err(ApiError::internal)?;
    audit(&state, Some(&id), "enrolled").await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"deviceId":id,"token":token})),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Heartbeat {
    state: String,
    #[serde(default)]
    reason: String,
    resources: Option<nodeharbor_core::Resources>,
    #[serde(default)]
    allow_ci: bool,
    #[serde(default)]
    allow_services: bool,
    #[serde(default)]
    permitted: bool,
}
async fn heartbeat(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Json(input): Json<Heartbeat>,
) -> ApiResult<Json<Value>> {
    let id = device_id(&state, &headers).await?;
    if ![
        "paused",
        "preparing",
        "connecting",
        "sharing",
        "draining",
        "error",
    ]
    .contains(&input.state.as_str())
        || input.reason.len() > 2048
    {
        return Err(ApiError::bad("Invalid worker status"));
    }
    let mut transaction = state.db.begin().await.map_err(ApiError::internal)?;
    sqlx::query("UPDATE devices SET state=?,reason=?,last_seen=?,resources=? WHERE id=?")
        .bind(input.state)
        .bind(input.reason)
        .bind(Utc::now().to_rfc3339())
        .bind(
            input
                .resources
                .map(|r| serde_json::to_string(&r).unwrap_or_default()),
        )
        .bind(&id)
        .execute(&mut *transaction)
        .await
        .map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO device_policy(device_id,allow_ci,allow_services,permitted) VALUES(?,?,?,?) ON CONFLICT(device_id) DO UPDATE SET allow_ci=excluded.allow_ci,allow_services=excluded.allow_services,permitted=excluded.permitted")
        .bind(&id).bind(input.allow_ci).bind(input.allow_services).bind(input.permitted)
        .execute(&mut *transaction).await.map_err(ApiError::internal)?;
    transaction.commit().await.map_err(ApiError::internal)?;
    let row =
        sqlx::query("SELECT remote_paused,eligible_ci,eligible_services FROM devices WHERE id=?")
            .bind(&id)
            .fetch_one(&state.db)
            .await
            .map_err(ApiError::internal)?;
    Ok(Json(
        json!({"remotePaused":row.get::<bool,_>("remote_paused"),"eligibleCi":row.get::<bool,_>("eligible_ci"),"eligibleServices":row.get::<bool,_>("eligible_services")}),
    ))
}
async fn revoke(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let _operation = state.operations.lock().await;
    let identity = identity(&state, &id).await?;
    let mut transaction = state.db.begin().await.map_err(ApiError::internal)?;
    let updated = sqlx::query(
        "UPDATE devices SET revoked=1,remote_paused=1,eligible_ci=0,eligible_services=0 WHERE id=?",
    )
    .bind(&id)
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::internal)?;
    if updated.rows_affected() == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Device not found".into()));
    }
    sqlx::query("INSERT OR IGNORE INTO pending_revocations(device_id) VALUES(?)")
        .bind(&id)
        .execute(&mut *transaction)
        .await
        .map_err(ApiError::internal)?;
    transaction.commit().await.map_err(ApiError::internal)?;
    audit(&state, Some(&id), "revoked").await?;
    if let Some(cluster) = &state.cluster {
        cluster.revoke(&identity).await.map_err(cluster_error)?;
        sqlx::query("DELETE FROM pending_revocations WHERE device_id=?")
            .bind(&id)
            .execute(&state.db)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(Json(json!({"revoked":true})))
}

fn cluster_error(error: anyhow::Error) -> ApiError {
    tracing::error!(error=%error, "cluster operation failed");
    ApiError(StatusCode::SERVICE_UNAVAILABLE, "The cluster operation could not be completed; the worker remains unavailable for new assignments".into())
}
async fn identity(state: &State, id: &str) -> ApiResult<DeviceIdentity> {
    let architecture: Option<String> =
        sqlx::query_scalar("SELECT architecture FROM devices WHERE id=?")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::internal)?;
    Ok(DeviceIdentity {
        id: id.into(),
        architecture: architecture
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Device not found".into()))?,
    })
}
async fn device_control(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path(action): Path<String>,
) -> ApiResult<Json<Value>> {
    let _operation = state.operations.lock().await;
    let id = device_id(&state, &headers).await?;
    let device = identity(&state, &id).await?;
    let cluster = state.cluster.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "This controller is waiting for its Kubernetes and private network configuration"
                .into(),
        )
    })?;
    let value = match action.as_str() {
        "bootstrap" => {
            let paused: bool = sqlx::query_scalar("SELECT remote_paused FROM devices WHERE id=?")
                .bind(&id)
                .fetch_one(&state.db)
                .await
                .map_err(ApiError::internal)?;
            if paused {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "This device is paused by its fleet administrator".into(),
                ));
            }
            cluster.bootstrap(&device).await.map_err(cluster_error)?
        }
        "drain" => {
            cluster.drain(&device).await.map_err(cluster_error)?;
            sqlx::query("UPDATE devices SET eligible_ci=0,eligible_services=0 WHERE id=?")
                .bind(&id)
                .execute(&state.db)
                .await
                .map_err(ApiError::internal)?;
            json!({"draining":true})
        }
        "resume" => {
            cluster.resume(&device).await.map_err(cluster_error)?;
            json!({"observing":true})
        }
        _ => {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "Worker operation not found".into(),
            ))
        }
    };
    audit(&state, Some(&id), &format!("worker_{action}")).await?;
    Ok(Json(value))
}
async fn ready(Extract(state): Extract<State>) -> ApiResult<Json<Value>> {
    sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(
        json!({"status":"ready","version":env!("CARGO_PKG_VERSION")}),
    ))
}
async fn admin_control(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path((id, action)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    if !["pause", "resume"].contains(&action.as_str()) {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Fleet operation not found".into(),
        ));
    }
    let _operation = state.operations.lock().await;
    let device = identity(&state, &id).await?;
    let paused = action == "pause";
    let updated=sqlx::query("UPDATE devices SET remote_paused=?,eligible_ci=0,eligible_services=0 WHERE id=? AND revoked=0")
        .bind(paused).bind(&id).execute(&state.db).await.map_err(ApiError::internal)?;
    if updated.rows_affected() == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Device not found".into()));
    }
    audit(&state, Some(&id), &format!("admin_{action}")).await?;
    if paused {
        if let Some(cluster) = &state.cluster {
            cluster.drain(&device).await.map_err(cluster_error)?;
        }
    }
    Ok(Json(json!({"remotePaused":paused})))
}
pub fn router(state: State) -> Router {
    Router::new()
        .route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/readyz", get(ready))
        .route("/api/v1/fleet", get(fleet))
        .route("/api/v1/device/fleet", get(device_fleet))
        .route("/api/v1/device/{action}", post(device_control))
        .route("/api/v1/enrollment-codes", post(create_code))
        .route("/api/v1/enroll", post(enroll))
        .route("/api/v1/heartbeat", post(heartbeat))
        .route("/api/v1/devices/{id}/revoke", post(revoke))
        .route("/api/v1/devices/{id}/{action}", post(admin_control))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response
                    .headers_mut()
                    .insert("cache-control", "no-store".parse().unwrap());
                response
                    .headers_mut()
                    .insert("x-content-type-options", "nosniff".parse().unwrap());
                response
            },
        ))
        .with_state(state)
}
