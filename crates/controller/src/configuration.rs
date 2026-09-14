use super::*;
use nodeharbor_core::configuration::{
    validate_edit, ConfigurationCommand, ConfigurationEdit, ConfigurationReport,
};

pub(super) async fn initialize(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS node_configuration(device_id TEXT PRIMARY KEY, report TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS configuration_requests(device_id TEXT NOT NULL, request_id TEXT NOT NULL,
          edit TEXT NOT NULL, actor TEXT NOT NULL, at TEXT NOT NULL, status TEXT NOT NULL,
          before_policy TEXT, effective_policy TEXT, effective_resources TEXT, error TEXT, ack_at TEXT,
          PRIMARY KEY(device_id,request_id));
        CREATE UNIQUE INDEX IF NOT EXISTS one_configuration_request ON configuration_requests(device_id)
          WHERE status IN ('requested','pending');").execute(db).await?;
    Ok(())
}
fn conflict(message: &str) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}
fn decode<T: serde::de::DeserializeOwned>(text: &str) -> ApiResult<T> {
    serde_json::from_str(text).map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Stored configuration is unreadable; controller maintenance is required".into(),
        )
    })
}
async fn current(state: &State, id: &str) -> ApiResult<Option<ConfigurationReport>> {
    let text: Option<String> =
        sqlx::query_scalar("SELECT report FROM node_configuration WHERE device_id=?")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::internal)?;
    text.as_deref().map(decode).transpose()
}
async fn online(state: &State, id: &str) -> ApiResult<bool> {
    let seen: Option<String> =
        sqlx::query_scalar("SELECT last_seen FROM devices WHERE id=? AND revoked=0")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::internal)?
            .flatten();
    Ok(seen
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .is_some_and(|seen| Utc::now().signed_duration_since(seen).num_seconds() <= 90))
}
fn view(row: &sqlx::sqlite::SqliteRow) -> ApiResult<Value> {
    let edit: ConfigurationEdit = decode(row.get("edit"))?;
    let value = |key| -> ApiResult<Value> {
        row.get::<Option<String>, _>(key)
            .as_deref()
            .map(decode)
            .transpose()
            .map(|v| v.unwrap_or(Value::Null))
    };
    Ok(
        json!({"requestId":edit.request_id,"expectedRevision":edit.expected_revision,"policy":edit.policy,
        "actor":row.get::<String,_>("actor"),"requestedAt":row.get::<String,_>("at"),"status":row.get::<String,_>("status"),
        "beforePolicy":value("before_policy")?,"effectivePolicy":value("effective_policy")?,"effectiveResources":value("effective_resources")?,
        "error":row.get::<Option<String>,_>("error"),"acknowledgedAt":row.get::<Option<String>,_>("ack_at")}),
    )
}
pub(super) async fn inspect(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let _operation = state.operations.lock().await;
    identity(&state, &id).await?;
    let report = current(&state, &id).await?;
    let rows = sqlx::query(
        "SELECT * FROM configuration_requests WHERE device_id=? ORDER BY rowid DESC LIMIT 100",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::internal)?;
    let requests = rows.iter().map(view).collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(
        json!({"online":online(&state,&id).await?,"report":report,"requests":requests}),
    ))
}
pub(super) async fn request(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(edit): Json<ConfigurationEdit>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    admin(&state, &headers)?;
    let actor = if bearer(&headers).is_ok_and(|token| hash(token) == *state.admin_hash) {
        "administrator token".to_owned()
    } else {
        headers
            .get("x-auth-request-email")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let _operation = state.operations.lock().await;
    identity(&state, &id).await?;
    if let Some(row) =
        sqlx::query("SELECT * FROM configuration_requests WHERE device_id=? AND request_id=?")
            .bind(&id)
            .bind(&edit.request_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::internal)?
    {
        let saved: ConfigurationEdit = decode(row.get("edit"))?;
        if saved != edit {
            return Err(conflict(
                "This request ID already belongs to a different edit",
            ));
        }
        return Ok((StatusCode::ACCEPTED, Json(view(&row)?)));
    }
    let report = current(&state, &id).await?.ok_or_else(|| {
        conflict("This node has not reported configuration support; update it locally")
    })?;
    if !report.consent {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "The local owner has not allowed remote configuration".into(),
        ));
    }
    if !online(&state, &id).await? {
        return Err(conflict(
            "The node is offline; wait for fresh inventory before configuring it",
        ));
    }
    if report.revision != edit.expected_revision {
        return Err(conflict(
            "Settings changed locally or remotely; reload current settings",
        ));
    }
    let policy = report
        .policy
        .as_ref()
        .ok_or_else(|| conflict("The node has not reported its current settings"))?;
    if edit.policy.resources.disk_gib != policy.resources.disk_gib
        && !report.capabilities.disk_growth
    {
        return Err(ApiError::bad("Disk growth requires local approval: the node cannot verify its physical storage location"));
    }
    let hardware = report
        .hardware
        .as_ref()
        .ok_or_else(|| conflict("The node has not reported its available capacity"))?;
    validate_edit(&edit, policy, hardware, report.consent, report.revision)
        .map_err(|e| ApiError::bad(&e))?;
    let pending: i64=sqlx::query_scalar("SELECT COUNT(*) FROM configuration_requests WHERE device_id=? AND status IN ('requested','pending')")
        .bind(&id).fetch_one(&state.db).await.map_err(ApiError::internal)?;
    if pending != 0 {
        return Err(conflict(
            "A configuration request is pending; wait for its acknowledgment",
        ));
    }
    let at = Utc::now().to_rfc3339();
    let mut tx = state.db.begin().await.map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO configuration_requests(device_id,request_id,edit,actor,at,status,before_policy) VALUES(?,?,?,?,?,'requested',?)")
        .bind(&id).bind(&edit.request_id).bind(json!(edit).to_string()).bind(&actor).bind(&at).bind(json!(policy).to_string())
        .execute(&mut *tx).await.map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO audit(at,device_id,action) VALUES(?,?,?)").bind(&at).bind(&id)
        .bind(json!({"action":"configuration_requested","actor":actor,"edit":edit,"beforePolicy":policy}).to_string())
        .execute(&mut *tx).await.map_err(ApiError::internal)?;
    tx.commit().await.map_err(ApiError::internal)?;
    let row =
        sqlx::query("SELECT * FROM configuration_requests WHERE device_id=? AND request_id=?")
            .bind(&id)
            .bind(&edit.request_id)
            .fetch_one(&state.db)
            .await
            .map_err(ApiError::internal)?;
    Ok((StatusCode::ACCEPTED, Json(view(&row)?)))
}
/// Called under the controller operation lock, using the authenticated device ID.
/// A heartbeat from another device cannot read or acknowledge this queue.
pub(super) async fn exchange(
    state: &State,
    id: &str,
    report: Option<ConfigurationReport>,
) -> ApiResult<Option<ConfigurationCommand>> {
    let Some(mut report) = report else {
        return Ok(None);
    };
    if report.receipts.len() > 64 {
        return Err(ApiError::bad("Too many configuration acknowledgments"));
    }
    let previous = current(state, id).await?;
    if previous
        .as_ref()
        .is_some_and(|p| p.revision > report.revision)
    {
        return Ok(None);
    }
    if !report.consent {
        report.policy = None;
        report.hardware = None;
        report.storage_inventory.clear();
        report.worker_disk_location = None;
    }
    let mut tx = state.db.begin().await.map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO node_configuration(device_id,report) VALUES(?,?) ON CONFLICT(device_id) DO UPDATE SET report=excluded.report")
        .bind(id).bind(json!(report).to_string()).execute(&mut *tx).await.map_err(ApiError::internal)?;
    for receipt in &report.receipts {
        if !["pending", "applied", "rejected"].contains(&receipt.status.as_str()) {
            return Err(ApiError::bad("Invalid configuration acknowledgment"));
        }
        let row=sqlx::query("SELECT * FROM configuration_requests WHERE device_id=? AND request_id=? AND status IN ('requested','pending')")
            .bind(id).bind(&receipt.request_id).fetch_optional(&mut *tx).await.map_err(ApiError::internal)?;
        let Some(row) = row else { continue };
        let edit: ConfigurationEdit = decode(row.get("edit"))?;
        if receipt.revision < edit.expected_revision {
            continue;
        }
        sqlx::query("UPDATE configuration_requests SET status=?,effective_policy=?,effective_resources=?,error=?,ack_at=? WHERE device_id=? AND request_id=?")
            .bind(&receipt.status).bind(receipt.effective_policy.as_ref().map(|p|json!(p).to_string()))
            .bind(receipt.effective_resources.as_ref().map(|r|json!(r).to_string())).bind(&receipt.error).bind(&receipt.at)
            .bind(id).bind(&receipt.request_id).execute(&mut *tx).await.map_err(ApiError::internal)?;
        if receipt.status != row.get::<String, _>("status") {
            sqlx::query("INSERT INTO audit(at,device_id,action) VALUES(?,?,?)").bind(Utc::now().to_rfc3339()).bind(id)
                .bind(json!({"action":"configuration_acknowledged","actor":row.get::<String,_>("actor"),"receipt":receipt}).to_string())
                .execute(&mut *tx).await.map_err(ApiError::internal)?;
        }
    }
    let rows=sqlx::query("SELECT * FROM configuration_requests WHERE device_id=? AND status IN ('requested','pending')")
        .bind(id).fetch_all(&mut *tx).await.map_err(ApiError::internal)?;
    let mut command = None;
    for row in rows {
        let edit: ConfigurationEdit = decode(row.get("edit"))?;
        if !report.consent || report.revision != edit.expected_revision {
            sqlx::query("UPDATE configuration_requests SET status='rejected',error=?,ack_at=? WHERE device_id=? AND request_id=?")
                .bind("The owner changed settings or consent; reload current settings").bind(Utc::now().to_rfc3339())
                .bind(id).bind(&edit.request_id).execute(&mut *tx).await.map_err(ApiError::internal)?;
        } else {
            command = Some(ConfigurationCommand {
                edit,
                actor: row.get("actor"),
                requested_at: row.get("at"),
            });
        }
    }
    tx.commit().await.map_err(ApiError::internal)?;
    Ok(command)
}
