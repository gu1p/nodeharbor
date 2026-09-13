//! Durable cleanup for owner-requested worker recreation. Enrollment survives;
//! old cluster access and qualification evidence do not.
use crate::{audit, cluster_error, device_id, ApiError, ApiResult, DeviceIdentity, State};
use anyhow::{Context, Result};
use axum::{
    extract::State as Extract,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResetRequest {
    request_id: String,
}

impl State {
    pub(crate) async fn ensure_not_resetting(&self, id: &str) -> ApiResult<()> {
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM worker_resets WHERE device_id=? AND complete=0)",
        )
        .bind(id)
        .fetch_one(&self.db)
        .await
        .map_err(ApiError::internal)?;
        if pending {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Worker recreation is still removing the previous worker's access".into(),
            ));
        }
        Ok(())
    }
    async fn finish_worker_reset(&self, id: &str) -> Result<()> {
        let cluster = self
            .cluster
            .as_ref()
            .context("Worker infrastructure is not configured")?;
        let architecture: String =
            sqlx::query_scalar("SELECT architecture FROM devices WHERE id=? AND revoked=0")
                .bind(id)
                .fetch_one(&self.db)
                .await?;
        let device = DeviceIdentity {
            id: id.into(),
            architecture,
        };
        cluster.drain(&device).await?;
        cluster.revoke(&device).await?;
        let mut transaction = self.db.begin().await?;
        sqlx::query("DELETE FROM health_samples WHERE device_id=?")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE devices SET eligible_ci=0,eligible_services=0,state='paused',reason='Prepare the replacement worker to continue' WHERE id=?")
            .bind(id).execute(&mut *transaction).await?;
        sqlx::query("UPDATE device_policy SET permitted=0 WHERE device_id=?")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE worker_resets SET complete=1 WHERE device_id=? AND complete=0")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("INSERT INTO audit(at,device_id,action) VALUES(?,?,'worker_reset_completed')")
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }
    pub(crate) async fn retry_worker_resets(&self) -> Result<()> {
        let _operation = self.operations.lock().await;
        let rows=sqlx::query("SELECT r.device_id FROM worker_resets r JOIN devices d ON d.id=r.device_id WHERE r.complete=0 AND d.revoked=0")
            .fetch_all(&self.db).await?;
        let mut failures = 0;
        for row in rows {
            let id: String = row.get("device_id");
            if self.finish_worker_reset(&id).await.is_err() {
                failures += 1;
            }
        }
        anyhow::ensure!(
            failures == 0,
            "{failures} worker recreation cleanups remain pending"
        );
        Ok(())
    }
}

pub(crate) async fn reset(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Json(input): Json<ResetRequest>,
) -> ApiResult<Json<Value>> {
    let _operation = state.operations.lock().await;
    let id = device_id(&state, &headers).await?;
    if uuid::Uuid::parse_str(&input.request_id).is_err() {
        return Err(ApiError::bad(
            "Worker recreation requires a valid request identity",
        ));
    }
    if state.cluster.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Worker infrastructure is not configured".into(),
        ));
    }
    let existing: Option<bool> =
        sqlx::query_scalar("SELECT complete FROM worker_resets WHERE device_id=? AND request_id=?")
            .bind(&id)
            .bind(&input.request_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::internal)?;
    if existing == Some(true) {
        return Ok(Json(json!({"complete":true})));
    }
    if existing.is_none() {
        state.ensure_not_resetting(&id).await?;
        let mut transaction = state.db.begin().await.map_err(ApiError::internal)?;
        sqlx::query("INSERT INTO worker_resets(device_id,request_id,complete) VALUES(?,?,0)")
            .bind(&id)
            .bind(&input.request_id)
            .execute(&mut *transaction)
            .await
            .map_err(ApiError::internal)?;
        sqlx::query("DELETE FROM health_samples WHERE device_id=?")
            .bind(&id)
            .execute(&mut *transaction)
            .await
            .map_err(ApiError::internal)?;
        sqlx::query("UPDATE devices SET eligible_ci=0,eligible_services=0,state='paused',reason='Worker recreation requested' WHERE id=?")
            .bind(&id).execute(&mut *transaction).await.map_err(ApiError::internal)?;
        sqlx::query("UPDATE device_policy SET permitted=0 WHERE device_id=?")
            .bind(&id)
            .execute(&mut *transaction)
            .await
            .map_err(ApiError::internal)?;
        transaction.commit().await.map_err(ApiError::internal)?;
        audit(&state, Some(&id), "worker_reset_requested").await?;
    }
    state
        .finish_worker_reset(&id)
        .await
        .map_err(cluster_error)?;
    Ok(Json(json!({"complete":true})))
}
