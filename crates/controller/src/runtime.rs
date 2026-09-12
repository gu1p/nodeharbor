use crate::{ApiClient, ClusterConfig, ProbeConfig, Provisioner, Reconciler, State};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiConfig {
    url: String,
    token_file: PathBuf,
    ca_file: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClusterRuntime {
    kubernetes: ApiConfig,
    netbird: ApiConfig,
    worker: ClusterConfig,
    probe: ProbeConfig,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Gateway {
    secret_file: PathBuf,
    allowed_emails: Vec<String>,
    origin: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    cluster: Option<ClusterRuntime>,
    gateway: Option<Gateway>,
}
#[derive(Clone)]
pub struct ConfiguredController {
    pub state: State,
    pub provisioner: Option<Arc<Provisioner>>,
    pub reconciler: Option<Reconciler>,
}
impl ConfiguredController {
    pub async fn maintain_once(&self) -> Result<()> {
        let mut errors = Vec::new();
        if let Err(error) = self.state.retry_revocations().await {
            errors.push(error.to_string());
        }
        if let Some(provisioner) = &self.provisioner {
            let _operation = self.state.operations.lock().await;
            if let Err(error) = provisioner.expire_grants().await {
                errors.push(error.to_string());
            }
        }
        if let Some(reconciler) = &self.reconciler {
            if let Err(error) = reconciler.tick_at(chrono::Utc::now()).await {
                errors.push(error.to_string());
            }
        }
        anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
        Ok(())
    }
    pub async fn maintain(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = self.maintain_once().await {
                tracing::error!(error=%error,"controller maintenance will retry");
            }
        }
    }
}
fn credential(base: &Path, path: &Path) -> Result<(PathBuf, String)> {
    let path = base.join(path);
    let value = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read credential file {}", path.display()))?;
    anyhow::ensure!(
        !value.trim().is_empty(),
        "Credential file {} is empty",
        path.display()
    );
    Ok((path, value.trim().to_owned()))
}
fn api(config: ApiConfig, base: &Path, scheme: &str) -> Result<ApiClient> {
    let (token, _) = credential(base, &config.token_file)?;
    let ca = config
        .ca_file
        .map(|path| std::fs::read(base.join(path)))
        .transpose()?;
    ApiClient::new(&config.url, &token, scheme, ca.as_deref())
}
pub async fn configure_runtime(mut state: State, path: &Path) -> Result<ConfiguredController> {
    let config: Config = serde_json::from_slice(&std::fs::read(path)?)
        .context("Invalid controller configuration")?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    if let Some(gateway) = config.gateway {
        let (_, secret) = credential(base, &gateway.secret_file)?;
        state = state.with_proxy_auth(&secret, gateway.allowed_emails, &gateway.origin)?;
    }
    let provisioner = if let Some(cluster) = config.cluster {
        let provisioner = Arc::new(
            Provisioner::new(
                cluster.worker,
                api(cluster.kubernetes, base, "Bearer")?,
                api(cluster.netbird, base, "Token")?,
                state.db.clone(),
            )
            .await?
            .with_probe(cluster.probe)?,
        );
        state = state.with_cluster(provisioner.clone());
        Some(provisioner)
    } else {
        None
    };
    let reconciler = provisioner
        .as_ref()
        .map(|backend| Reconciler::new(state.clone(), backend.clone()));
    Ok(ConfiguredController {
        state,
        provisioner,
        reconciler,
    })
}
