use crate::{assess_health, DeviceIdentity, HealthSample, State};
use anyhow::Result;
use chrono::{DateTime, Duration, TimeZone, Utc};
use nodeharbor_core::Resources;
use sqlx::Row;
use std::sync::Arc;

#[async_trait::async_trait]
pub trait HealthBackend: Send + Sync {
    async fn observe(&self, device: &DeviceIdentity, budget: &Resources) -> Result<f64>;
    async fn place(
        &self,
        device: &DeviceIdentity,
        ci: bool,
        services: bool,
        accepting: bool,
    ) -> Result<()>;
}
#[derive(Clone)]
pub struct Reconciler {
    state: State,
    backend: Arc<dyn HealthBackend>,
}
impl Reconciler {
    pub fn new(state: State, backend: Arc<dyn HealthBackend>) -> Self {
        Self { state, backend }
    }
    pub async fn tick_at(&self, now: DateTime<Utc>) -> Result<()> {
        self.state
            .telemetry
            .operation(crate::Operation::Reconcile, self.reconcile_at(now))
            .await
    }
    async fn reconcile_at(&self, now: DateTime<Utc>) -> Result<()> {
        // Serialize admission against revocation and explicit device controls.
        let _operation = self.state.operations.lock().await;
        let rows = sqlx::query("SELECT d.id,d.architecture,d.last_seen,d.remote_paused,d.state,d.resources,COALESCE(p.allow_ci,0) AS allow_ci,COALESCE(p.allow_services,0) AS allow_services,COALESCE(p.permitted,0) AS permitted FROM devices d LEFT JOIN device_policy p ON p.device_id=d.id WHERE d.revoked=0")
            .fetch_all(&self.state.db).await?;
        let mut errors = Vec::new();
        for row in rows {
            let device = DeviceIdentity {
                id: row.get("id"),
                architecture: row.get("architecture"),
            };
            let fresh = row
                .get::<Option<String>, _>("last_seen")
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .is_some_and(|seen| {
                    (0..=90).contains(&now.signed_duration_since(seen).num_seconds())
                });
            let accepting = fresh
                && !row.get::<bool, _>("remote_paused")
                && row.get::<bool, _>("permitted")
                && row.get::<String, _>("state") == "sharing";
            let resources = row
                .get::<Option<String>, _>("resources")
                .and_then(|s| serde_json::from_str::<Resources>(&s).ok());
            let observation = if let Some(resources) = resources.filter(|_| accepting) {
                self.backend.observe(&device, &resources).await
            } else {
                Err(anyhow::anyhow!(
                    "Sharing is paused or the owner heartbeat is unavailable"
                ))
            };
            let rtt = observation
                .as_ref()
                .ok()
                .copied()
                .filter(|rtt| rtt.is_finite() && *rtt >= 0.0);
            let slot = Utc
                .timestamp_opt(now.timestamp().div_euclid(30) * 30, 0)
                .single()
                .unwrap_or(now);
            sqlx::query("INSERT INTO health_samples(device_id,at,ready,rtt_ms) VALUES(?,?,?,?) ON CONFLICT(device_id,at) DO UPDATE SET ready=MIN(health_samples.ready,excluded.ready),rtt_ms=MAX(health_samples.rtt_ms,excluded.rtt_ms)")
                .bind(&device.id).bind(slot.to_rfc3339()).bind(rtt.is_some()).bind(rtt).execute(&self.state.db).await?;
            let samples = sqlx::query("SELECT at,ready,rtt_ms FROM health_samples WHERE device_id=? AND at>=? ORDER BY at")
                .bind(&device.id).bind((slot-Duration::days(1)).to_rfc3339()).fetch_all(&self.state.db).await?;
            let samples: Vec<_> = samples
                .iter()
                .filter_map(|row| {
                    Some(HealthSample {
                        at: DateTime::parse_from_rfc3339(row.get::<&str, _>("at"))
                            .ok()?
                            .timestamp(),
                        ready: row.get("ready"),
                        rtt_ms: row.get("rtt_ms"),
                    })
                })
                .collect();
            let qualified = assess_health(&samples, now.timestamp());
            let ci = accepting && qualified.ci && row.get::<bool, _>("allow_ci");
            let services = accepting && qualified.services && row.get::<bool, _>("allow_services");
            // Clear the dashboard before external admission changes. Publish eligibility
            // only after Kubernetes acknowledges the protected labels.
            sqlx::query("UPDATE devices SET eligible_ci=0,eligible_services=0 WHERE id=?")
                .bind(&device.id)
                .execute(&self.state.db)
                .await?;
            let placement = self.backend.place(&device, ci, services, accepting).await;
            let reason = match &placement {
                Err(error) => {
                    errors.push(format!("{}: {error}", device.id));
                    "Cannot confirm worker admission; retrying".to_owned()
                }
                Ok(()) => {
                    sqlx::query("UPDATE devices SET eligible_ci=?,eligible_services=? WHERE id=? AND revoked=0")
                        .bind(ci).bind(services).bind(&device.id).execute(&self.state.db).await?;
                    match observation {
                        Ok(_) => qualified.reason,
                        Err(error) => error.to_string(),
                    }
                }
            };
            sqlx::query("INSERT INTO device_health(device_id,reason,observed_at) VALUES(?,?,?) ON CONFLICT(device_id) DO UPDATE SET reason=excluded.reason,observed_at=excluded.observed_at")
                .bind(&device.id).bind(reason).bind(now.to_rfc3339()).execute(&self.state.db).await?;
        }
        sqlx::query("DELETE FROM health_samples WHERE at<?")
            .bind((now - Duration::hours(25)).to_rfc3339())
            .execute(&self.state.db)
            .await?;
        anyhow::ensure!(
            errors.is_empty(),
            "Admission reconciliation failed: {}",
            errors.join("; ")
        );
        Ok(())
    }
}
