use crate::{
    provision::{api_id, CI_LABEL, SERVICES_LABEL},
    verify_worker_evidence, DeviceIdentity, HealthBackend, Provisioner,
};
use anyhow::{Context, Result};
use nodeharbor_core::Resources;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};

const CONTRIBUTED: &str = "nodeharbor.sikalio.dev/contributed";
const QUARANTINE: &str = "nodeharbor.sikalio.dev/quarantine";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeConfig {
    pub namespace: String,
    pub cluster_cidr: String,
    pub port: u16,
}
impl Provisioner {
    pub(crate) async fn verified_probe_pods(&self, device: &DeviceIdentity) -> Result<Vec<Value>> {
        let Some(config) = &self.probe else {
            return Ok(Vec::new());
        };
        let (status, daemonset) = self
            .kube
            .request(
                Method::GET,
                &format!(
                    "/apis/apps/v1/namespaces/{}/daemonsets/nodeharbor-probe",
                    config.namespace
                ),
                None,
            )
            .await?;
        if status == 404 {
            return Ok(Vec::new());
        }
        anyhow::ensure!(
            (200..300).contains(&status),
            "Cannot inspect the worker probe's DaemonSet"
        );
        let uid = daemonset["metadata"]["uid"]
            .as_str()
            .context("Probe DaemonSet identity is unavailable")?;
        uuid::Uuid::parse_str(uid).context("Invalid probe DaemonSet identity")?;
        anyhow::ensure!(
            daemonset["apiVersion"] == "apps/v1"
                && daemonset["kind"] == "DaemonSet"
                && daemonset["metadata"]["name"] == "nodeharbor-probe"
                && daemonset["metadata"]["namespace"] == config.namespace
                && daemonset["metadata"]["deletionTimestamp"].is_null(),
            "Probe DaemonSet identity changed"
        );
        let pods = self.kube.call(Method::GET,
            &format!("/api/v1/namespaces/{}/pods?fieldSelector=spec.nodeName%3D{}&labelSelector=app.kubernetes.io/name%3Dnodeharbor-probe", config.namespace, device.node_name()), None).await?;
        Ok(pods["items"]
            .as_array()
            .context("Probe inventory is unavailable")?
            .iter()
            .filter(|pod| {
                pod["metadata"]["namespace"] == config.namespace
                    && pod["spec"]["nodeName"] == device.node_name()
                    && pod["metadata"]["uid"]
                        .as_str()
                        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                    && pod["metadata"]["annotations"]["kubernetes.io/config.mirror"].is_null()
                    && pod["metadata"]["ownerReferences"]
                        .as_array()
                        .is_some_and(|owners| {
                            owners.iter().any(|owner| {
                                owner["apiVersion"] == "apps/v1"
                                    && owner["kind"] == "DaemonSet"
                                    && owner["name"] == "nodeharbor-probe"
                                    && owner["uid"] == uid
                                    && owner["controller"] == true
                            })
                        })
            })
            .cloned()
            .collect())
    }
    pub fn with_probe(mut self, config: ProbeConfig) -> Result<Self> {
        api_id(&config.namespace)?;
        config.cluster_cidr.parse::<ipnet::Ipv4Net>()?;
        anyhow::ensure!(config.port > 0, "Probe port is required");
        self.probe = Some(config);
        Ok(self)
    }
}
#[async_trait::async_trait]
impl HealthBackend for Provisioner {
    async fn observe(&self, device: &DeviceIdentity, budget: &Resources) -> Result<f64> {
        let config = self
            .probe
            .as_ref()
            .context("Worker network probe is not configured")?;
        let node = self
            .node(device)
            .await?
            .context("Waiting for Kubernetes node registration")?;
        let group = self
            .group(device)
            .await?
            .context("The device has no enrolled network group")?;
        let peers = group["peers"]
            .as_array()
            .context("Network group membership is unavailable")?;
        anyhow::ensure!(
            peers.len() == 1,
            "The device must have exactly one enrolled network peer"
        );
        let id = api_id(
            peers[0]["id"]
                .as_str()
                .context("Network peer identity is missing")?,
        )?;
        let peer = self
            .netbird
            .call(Method::GET, &format!("/api/peers/{id}"), None)
            .await?;
        anyhow::ensure!(
            peer["id"] == id,
            "Network peer identity changed during inspection"
        );
        verify_worker_evidence(&device.id, &device.architecture, budget, &node, &peer)?;
        let cluster = config.cluster_cidr.parse::<ipnet::Ipv4Net>()?;
        let subnet = node["spec"]["podCIDR"]
            .as_str()
            .context("Node pod subnet is not assigned")?
            .parse::<ipnet::Ipv4Net>()?;
        anyhow::ensure!(
            cluster.contains(&subnet.network()) && cluster.contains(&subnet.broadcast()),
            "Node pod subnet is outside the configured cluster network"
        );
        let pods = self.verified_probe_pods(device).await?;
        let pod = pods
            .iter()
            .find(|pod| {
                pod["status"]["phase"] == "Running"
                    && pod["metadata"]["deletionTimestamp"].is_null()
                    && pod["status"]["conditions"]
                        .as_array()
                        .is_some_and(|conditions| {
                            conditions
                                .iter()
                                .any(|c| c["type"] == "Ready" && c["status"] == "True")
                        })
            })
            .context("Waiting for the worker DNS and overlay probe")?;
        let ip: Ipv4Addr = pod["status"]["podIP"]
            .as_str()
            .context("Probe pod has no address")?
            .parse()?;
        anyhow::ensure!(
            subnet.contains(&ip),
            "Probe pod address is outside this node’s assigned subnet"
        );
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        // Measure a TCP handshake to the verified pod as network RTT. Timing
        // the full HTTP request also counts the probe's separate DNS round trips.
        let start = Instant::now();
        let connection = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect((ip, config.port)),
        )
        .await
        .context("Worker network probe connection timed out")??;
        let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
        drop(connection);
        // Direct pod traffic tests the overlay. Never send infrastructure tokens
        // or honor a host HTTP proxy for this request.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(3))
            .build()?;
        let mut response = client
            .get(format!("http://{ip}:{}/readyz?nonce={nonce}", config.port))
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "Worker DNS or overlay probe failed"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= 8192,
                "Worker probe response is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let reply: Value = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            reply["nodeName"] == device.node_name()
                && reply["dns"] == true
                && reply["nonce"] == nonce
                && reply["padding"]
                    .as_str()
                    .is_some_and(|p| p.len() == 4096 && p.bytes().all(|b| b == b'x')),
            "Worker probe did not verify DNS, identity, and a complete multi-packet response"
        );
        Ok(rtt_ms)
    }
    async fn place(
        &self,
        device: &DeviceIdentity,
        ci: bool,
        services: bool,
        accepting: bool,
    ) -> Result<()> {
        let Some(node) = self.node(device).await? else {
            anyhow::ensure!(!ci && !services, "Cannot qualify a missing node");
            return Ok(());
        };
        let mut taints: Vec<Value> = node["spec"]["taints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t["key"] != CONTRIBUTED && t["key"] != QUARANTINE)
            .collect();
        taints.push(json!({"key":CONTRIBUTED,"value":"true","effect":"NoSchedule"}));
        if !accepting || (!ci && !services) {
            taints.push(json!({"key":QUARANTINE,"value":"true","effect":"NoSchedule"}));
        }
        let eligible = |allowed: bool| {
            if accepting && allowed {
                json!("true")
            } else {
                Value::Null
            }
        };
        let version = node["metadata"]["resourceVersion"]
            .as_str()
            .context("Node resource version is missing")?;
        self.kube.call(Method::PATCH,&format!("/api/v1/nodes/{}",device.node_name()),Some(json!({
            "metadata":{"resourceVersion":version,"labels":{CI_LABEL:eligible(ci),SERVICES_LABEL:eligible(services)}},
            "spec":{"unschedulable":!accepting,"taints":taints}
        }))).await?;
        Ok(())
    }
}
