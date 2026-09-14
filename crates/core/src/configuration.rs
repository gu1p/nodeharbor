//! Versioned remote edits. Consent is local state, never part of an edit.
use crate::{validate_policy, Policy, Resources};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<ConfigurationOperation>,
    pub request_id: String,
    pub expected_revision: u64,
    pub policy: Policy,
    pub acknowledge_interruption: bool,
}
/// Storage payloads use the node's local storage schema. The node deserializes
/// them strictly and repeats its local preview before accepting an application.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConfigurationOperation {
    StoragePreview {
        selections: Vec<serde_json::Value>,
        options: serde_json::Value,
    },
    StorageApply {
        plan: serde_json::Value,
    },
    StorageRecovery {
        enabled: bool,
    },
    StorageRetry {},
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationCommand {
    #[serde(flatten)]
    pub edit: ConfigurationEdit,
    pub actor: String,
    pub requested_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationReceipt {
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub effective_storage: Option<serde_json::Value>,
    pub request_id: String,
    pub status: String,
    pub revision: u64,
    pub at: String,
    pub effective_policy: Option<Policy>,
    pub effective_resources: Option<Resources>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteConfiguration {
    pub consent: bool,
    pub revision: u64,
    pub pending: Option<ConfigurationCommand>,
    pub receipts: Vec<ConfigurationReceipt>,
    /// A failed or interrupted runtime mutation cannot qualify as effective.
    pub repair_required: bool,
    pub applying: bool,
    /// Internal revisions made by the currently authorized storage lifecycle.
    pub execution_revision: Option<u64>,
    pub storage_started: bool,
}
impl RemoteConfiguration {
    pub fn record(&mut self, receipt: ConfigurationReceipt) {
        self.receipts.retain(|r| r.request_id != receipt.request_id);
        self.receipts.push(receipt);
        if self.receipts.len() > 64 {
            self.receipts.remove(0);
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationCapabilities {
    #[serde(default)]
    pub disk_growth: bool,
    #[serde(default)]
    pub disk_growth_reason: String,
    pub storage: bool,
    pub storage_reason: String,
    pub local_approval: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationReport {
    #[serde(default)]
    pub storage: Option<serde_json::Value>,
    #[serde(default)]
    pub worker_disk_location: Option<String>,
    pub consent: bool,
    pub revision: u64,
    pub policy: Option<Policy>,
    pub hardware: Option<Resources>,
    pub allocated_resources: Option<Resources>,
    pub capabilities: ConfigurationCapabilities,
    pub storage_inventory: Vec<serde_json::Value>,
    pub receipts: Vec<ConfigurationReceipt>,
}
pub fn validate_edit(
    edit: &ConfigurationEdit,
    current: &Policy,
    hardware: &Resources,
    consent: bool,
    revision: u64,
) -> Result<(), String> {
    if !consent {
        return Err("The local owner has not allowed remote configuration".into());
    }
    if edit.expected_revision != revision {
        return Err("Settings changed locally or remotely; reload current settings".into());
    }
    if edit.request_id.is_empty()
        || edit.request_id.len() > 128
        || !edit
            .request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(
            "Use a unique request ID of at most 128 letters, digits, dashes or underscores".into(),
        );
    }
    if !edit.acknowledge_interruption
        && !matches!(
            edit.operation,
            Some(ConfigurationOperation::StoragePreview { .. })
        )
    {
        return Err(
            "Acknowledge the worker drain, restart and possible interruption before applying"
                .into(),
        );
    }
    if edit.policy.start_at_login != current.start_at_login {
        return Err("Start at login requires local approval in the desktop application".into());
    }
    if edit.operation.is_some() {
        if &edit.policy != current {
            return Err(
                "Storage operations cannot also change sharing rules; reload current settings"
                    .into(),
            );
        }
        Ok(())
    } else {
        validate_policy(&edit.policy, hardware)
    }
}
