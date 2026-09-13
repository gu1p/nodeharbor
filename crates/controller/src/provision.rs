use crate::{Cluster, DeviceIdentity};
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::path::{Path, PathBuf};

pub const CI_LABEL: &str = "nodeharbor.node-restriction.kubernetes.io/ci";
pub const SERVICES_LABEL: &str = "nodeharbor.node-restriction.kubernetes.io/services";

#[derive(Clone)]
pub struct ApiClient {
    base: Url,
    token_file: PathBuf,
    scheme: String,
    client: reqwest::Client,
    telemetry: Option<(crate::Telemetry, crate::Peer)>,
}
impl ApiClient {
    pub fn new(base: &str, token_file: &Path, scheme: &str, ca_pem: Option<&[u8]>) -> Result<Self> {
        let base = Url::parse(base)?;
        anyhow::ensure!(
            base.scheme() == "https"
                || (base.scheme() == "http"
                    && ["localhost", "127.0.0.1", "[::1]"]
                        .contains(&base.host_str().unwrap_or_default())),
            "Infrastructure APIs require HTTPS"
        );
        anyhow::ensure!(
            base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none()
                && base.path() == "/",
            "API URLs must be origins without embedded credentials"
        );
        anyhow::ensure!(
            ["Bearer", "Token"].contains(&scheme),
            "Unsupported authentication scheme"
        );
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(ca) = ca_pem {
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(ca)?);
        }
        Ok(Self {
            base,
            token_file: token_file.into(),
            scheme: scheme.into(),
            client: builder.build()?,
            telemetry: None,
        })
    }
    pub fn with_telemetry(mut self, telemetry: crate::Telemetry, peer: crate::Peer) -> Self {
        self.telemetry = Some((telemetry, peer));
        self
    }
    pub(crate) async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        if let Some((telemetry, peer)) = &self.telemetry {
            telemetry
                .infrastructure(
                    *peer,
                    method.as_str(),
                    self.request_inner(method.clone(), path, body),
                )
                .await
        } else {
            self.request_inner(method, path, body).await
        }
    }
    async fn request_inner(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        // Projected service-account tokens rotate. Read the current token for
        // each request instead of retaining a credential from process startup.
        let token = tokio::fs::read_to_string(&self.token_file)
            .await
            .context("Cannot read infrastructure credential")?;
        anyhow::ensure!(
            !token.trim().is_empty(),
            "Infrastructure credential is empty"
        );
        let mut request = self
            .client
            .request(method.clone(), self.base.join(path)?)
            .header("authorization", format!("{} {}", self.scheme, token.trim()));
        // Propagate only the standard trace identity, never baggage or credentials.
        if self.telemetry.is_some() {
            use opentelemetry::{propagation::TextMapPropagator, Context};
            let mut headers = std::collections::HashMap::new();
            opentelemetry_sdk::propagation::TraceContextPropagator::new()
                .inject_context(&Context::current(), &mut headers);
            if let Some(parent) = headers.get("traceparent") {
                request = request.header("traceparent", parent);
            }
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        if method == Method::PATCH {
            request = request.header("content-type", "application/merge-patch+json");
        }
        let mut response = request
            .send()
            .await
            .context("Infrastructure API is unreachable")?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= 8 * 1024 * 1024,
                "Infrastructure response exceeds the supported size"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).context("Infrastructure API returned invalid JSON")?
        };
        Ok((status, value))
    }
    pub(crate) async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let (status, value) = self.request(method, path, body).await?;
        anyhow::ensure!(
            (200..300).contains(&status),
            "Infrastructure API returned HTTP {status} for {path}"
        );
        Ok(value)
    }
    async fn delete(&self, path: &str) -> Result<()> {
        let (status, _) = self.request(Method::DELETE, path, None).await?;
        anyhow::ensure!(
            (200..300).contains(&status) || status == 404,
            "Infrastructure removal returned HTTP {status} for {path}"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterConfig {
    pub server_url: String,
    pub ca_hash: String,
    pub netbird_management_url: String,
    pub workers_group_id: String,
}
#[derive(Clone)]
pub struct Provisioner {
    pub(crate) probe: Option<crate::ProbeConfig>,
    pub(crate) config: ClusterConfig,
    pub(crate) kube: ApiClient,
    pub(crate) netbird: ApiClient,
    pub(crate) db: SqlitePool,
}
pub(crate) fn api_id(value: &str) -> Result<&str> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 120
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'),
        "Infrastructure API returned an invalid resource identity"
    );
    Ok(value)
}
fn random_component(length: usize) -> Result<String> {
    let mut result = String::new();
    while result.len() < length {
        let mut byte = [0];
        getrandom::fill(&mut byte)
            .map_err(|_| anyhow::anyhow!("Secure randomness is unavailable"))?;
        // Rejection sampling avoids bias when choosing from 36 characters.
        if byte[0] < 252 {
            result.push(b"abcdefghijklmnopqrstuvwxyz0123456789"[usize::from(byte[0] % 36)] as char);
        }
    }
    Ok(result)
}
impl Provisioner {
    pub async fn new(
        config: ClusterConfig,
        kube: ApiClient,
        netbird: ApiClient,
        db: SqlitePool,
    ) -> Result<Self> {
        for endpoint in [&config.server_url, &config.netbird_management_url] {
            let url = Url::parse(endpoint)?;
            anyhow::ensure!(
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "Worker endpoints require HTTPS without credentials"
            );
        }
        anyhow::ensure!(
            config.ca_hash.len() == 64
                && config
                    .ca_hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "A verified K3s CA hash is required"
        );
        api_id(&config.workers_group_id)?;
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS setup_grants (device_id TEXT NOT NULL, key_id TEXT PRIMARY KEY, expires_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS bootstrap_grants (device_id TEXT NOT NULL, token_id TEXT PRIMARY KEY, expires_at TEXT NOT NULL);").execute(&db).await?;
        Ok(Self {
            config,
            probe: None,
            kube,
            netbird,
            db,
        })
    }
    pub(crate) async fn group(&self, device: &DeviceIdentity) -> Result<Option<Value>> {
        uuid::Uuid::parse_str(&device.id)?;
        let groups = self
            .netbird
            .call(
                Method::GET,
                &format!("/api/groups?name={}", device.node_name()),
                None,
            )
            .await?;
        let groups = groups
            .as_array()
            .context("NetBird returned an invalid group inventory")?;
        let found: Vec<_> = groups
            .iter()
            .filter(|g| g["name"] == device.node_name())
            .collect();
        anyhow::ensure!(
            found.len() <= 1,
            "Multiple network groups claim this device"
        );
        Ok(found.first().map(|v| (*v).clone()))
    }
    pub(crate) async fn node(&self, device: &DeviceIdentity) -> Result<Option<Value>> {
        uuid::Uuid::parse_str(&device.id)?;
        let (status, node) = self
            .kube
            .request(
                Method::GET,
                &format!("/api/v1/nodes/{}", device.node_name()),
                None,
            )
            .await?;
        if status == 404 {
            return Ok(None);
        }
        anyhow::ensure!(
            status == 200,
            "Kubernetes could not inspect the worker: HTTP {status}"
        );
        anyhow::ensure!(
            node["metadata"]["name"] == device.node_name()
                && node["metadata"]["labels"]["nodeharbor.sikalio.dev/device"] == device.id,
            "The Kubernetes node does not match this enrolled device"
        );
        Ok(Some(node))
    }
    async fn remove_keys(&self, device: &DeviceIdentity) -> Result<()> {
        let rows = sqlx::query("SELECT key_id FROM setup_grants WHERE device_id=?")
            .bind(&device.id)
            .fetch_all(&self.db)
            .await?;
        for row in rows {
            let key: String = row.get("key_id");
            self.netbird
                .delete(&format!("/api/setup-keys/{}", api_id(&key)?))
                .await?;
            sqlx::query("DELETE FROM setup_grants WHERE key_id=?")
                .bind(key)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }
    pub async fn expire_grants(&self) -> Result<()> {
        let rows = sqlx::query("SELECT key_id FROM setup_grants WHERE expires_at<=?")
            .bind(Utc::now().to_rfc3339())
            .fetch_all(&self.db)
            .await?;
        for row in rows {
            let key: String = row.get("key_id");
            self.netbird
                .delete(&format!("/api/setup-keys/{}", api_id(&key)?))
                .await?;
            sqlx::query("DELETE FROM setup_grants WHERE key_id=?")
                .bind(key)
                .execute(&self.db)
                .await?;
        }
        let rows = sqlx::query("SELECT token_id FROM bootstrap_grants WHERE expires_at<=?")
            .bind(Utc::now().to_rfc3339())
            .fetch_all(&self.db)
            .await?;
        for row in rows {
            let id: String = row.get("token_id");
            self.kube
                .delete(&format!(
                    "/api/v1/namespaces/kube-system/secrets/bootstrap-token-{}",
                    api_id(&id)?
                ))
                .await?;
            sqlx::query("DELETE FROM bootstrap_grants WHERE token_id=?")
                .bind(id)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Cluster for Provisioner {
    async fn bootstrap(&self, device: &DeviceIdentity) -> Result<Value> {
        uuid::Uuid::parse_str(&device.id)?;
        anyhow::ensure!(
            ["amd64", "arm64"].contains(&device.architecture.as_str()),
            "Unsupported worker architecture"
        );
        let group = match self.group(device).await? {
            Some(group) => group,
            None => {
                self.netbird
                    .call(
                        Method::POST,
                        "/api/groups",
                        Some(json!({"name":device.node_name(),"peers":[]})),
                    )
                    .await?
            }
        };
        let group_id = api_id(
            group["id"]
                .as_str()
                .context("NetBird returned no device group identity")?,
        )?;
        let peers = group["peers"]
            .as_array()
            .context("NetBird returned no group membership")?;
        anyhow::ensure!(
            peers.len() <= 1,
            "More than one network peer claims this device"
        );
        self.remove_keys(device).await?;
        let expires_at = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let setup_key = if peers.is_empty() {
            let key = self.netbird.call(Method::POST, "/api/setup-keys", Some(json!({
                "name":device.node_name(),"type":"one-off","expires_in":86400,"usage_limit":1,
                "auto_groups":[self.config.workers_group_id,group_id],"ephemeral":false,"allow_extra_dns_labels":false
            }))).await?;
            let id = api_id(
                key["id"]
                    .as_str()
                    .context("NetBird returned no setup key identity")?,
            )?;
            sqlx::query("INSERT INTO setup_grants(device_id,key_id,expires_at) VALUES(?,?,?)")
                .bind(&device.id)
                .bind(id)
                .bind(&expires_at)
                .execute(&self.db)
                .await?;
            Some(
                key["key"]
                    .as_str()
                    .context("NetBird returned no one-time setup key")?
                    .to_owned(),
            )
        } else {
            None
        };
        let token_id = random_component(6)?;
        let token_secret = random_component(16)?;
        let secret = json!({"apiVersion":"v1","kind":"Secret","type":"bootstrap.kubernetes.io/token",
            "metadata":{"name":format!("bootstrap-token-{token_id}"),"namespace":"kube-system","labels":{"nodeharbor.sikalio.dev/device":device.id}},
            "stringData":{"token-id":token_id,"token-secret":token_secret,"expiration":expires_at,
                "usage-bootstrap-authentication":"true","usage-bootstrap-signing":"true",
                "auth-extra-groups":"system:bootstrappers:k3s:default-node-token"}});
        self.kube
            .call(
                Method::POST,
                "/api/v1/namespaces/kube-system/secrets",
                Some(secret),
            )
            .await?;
        sqlx::query("INSERT INTO bootstrap_grants(device_id,token_id,expires_at) VALUES(?,?,?)")
            .bind(&device.id)
            .bind(&token_id)
            .bind(&expires_at)
            .execute(&self.db)
            .await?;
        let runtime: Value = serde_json::from_str(include_str!("../../../runtime-lock.json"))?;
        Ok(
            json!({"deviceId":device.id,"nodeName":device.node_name(),"netbirdManagementUrl":self.config.netbird_management_url,
            "netbirdSetupKey":setup_key,"serverUrl":self.config.server_url,"k3sToken":format!("K10{}::{token_id}.{token_secret}",self.config.ca_hash),
            "expiresAt":expires_at,"runtime":runtime}),
        )
    }
    async fn drain(&self, device: &DeviceIdentity) -> Result<()> {
        if self.node(device).await?.is_none() {
            return Ok(());
        }
        self.kube.call(Method::PATCH, &format!("/api/v1/nodes/{}", device.node_name()), Some(json!({
            "spec":{"unschedulable":true},"metadata":{"labels":{CI_LABEL:Value::Null,SERVICES_LABEL:Value::Null}}
        }))).await?;
        let pods = self
            .kube
            .call(
                Method::GET,
                &format!(
                    "/api/v1/pods?fieldSelector=spec.nodeName%3D{}",
                    device.node_name()
                ),
                None,
            )
            .await?;
        for pod in pods["items"]
            .as_array()
            .context("Kubernetes returned no workload inventory")?
        {
            if pod["metadata"]["ownerReferences"]
                .as_array()
                .is_some_and(|owners| owners.iter().any(|owner| owner["kind"] == "DaemonSet"))
                || pod["metadata"]["annotations"]["kubernetes.io/config.mirror"].is_string()
                || ["Succeeded", "Failed"]
                    .contains(&pod["status"]["phase"].as_str().unwrap_or_default())
            {
                continue;
            }
            let name = api_id(
                pod["metadata"]["name"]
                    .as_str()
                    .context("Workload has no name")?,
            )?;
            let namespace = api_id(
                pod["metadata"]["namespace"]
                    .as_str()
                    .context("Workload has no namespace")?,
            )?;
            let (status, _) = self.kube.request(Method::POST, &format!("/api/v1/namespaces/{namespace}/pods/{name}/eviction"),
                Some(json!({"apiVersion":"policy/v1","kind":"Eviction","metadata":{"name":name,"namespace":namespace}}))).await?;
            // A disruption budget can defer eviction. The host owns the finite
            // drain deadline; subsequent drain requests retry these workloads.
            anyhow::ensure!(
                (200..300).contains(&status) || [404, 409, 429].contains(&status),
                "Workload eviction returned HTTP {status}"
            );
        }
        Ok(())
    }
    async fn resume(&self, device: &DeviceIdentity) -> Result<()> {
        if self.node(device).await?.is_none() {
            return Ok(());
        }
        // Eligibility stays absent until independent health verification.
        self.kube
            .call(
                Method::PATCH,
                &format!("/api/v1/nodes/{}", device.node_name()),
                Some(json!({"spec":{"unschedulable":false},
            "metadata":{"labels":{CI_LABEL:Value::Null,SERVICES_LABEL:Value::Null}}})),
            )
            .await?;
        Ok(())
    }
    async fn revoke(&self, device: &DeviceIdentity) -> Result<()> {
        uuid::Uuid::parse_str(&device.id)?;
        self.remove_keys(device).await?;
        if let Some(group) = self.group(device).await? {
            for peer in group["peers"]
                .as_array()
                .context("NetBird returned no group membership")?
            {
                let id = api_id(
                    peer["id"]
                        .as_str()
                        .context("Network peer has no identity")?,
                )?;
                self.netbird.delete(&format!("/api/peers/{id}")).await?;
            }
        }
        if self.node(device).await?.is_some() {
            self.kube
                .delete(&format!("/api/v1/nodes/{}", device.node_name()))
                .await?;
        }
        let rows = sqlx::query("SELECT token_id FROM bootstrap_grants WHERE device_id=?")
            .bind(&device.id)
            .fetch_all(&self.db)
            .await?;
        for row in rows {
            let id: String = row.get("token_id");
            self.kube
                .delete(&format!(
                    "/api/v1/namespaces/kube-system/secrets/bootstrap-token-{}",
                    api_id(&id)?
                ))
                .await?;
            sqlx::query("DELETE FROM bootstrap_grants WHERE token_id=?")
                .bind(id)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }
}
