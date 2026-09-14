use super::*;
use crate::storage::ChangePlan;

pub(super) struct SharingSettings {
    pub policy: Policy,
    pub revision: u64,
}
impl SharingSettings {
    pub fn validate(&self, config: &Configuration) -> Result<()> {
        anyhow::ensure!(
            config.remote.revision == self.revision,
            "Settings changed locally or remotely; reload current settings before saving"
        );
        anyhow::ensure!(
            !self.policy.enabled || config.device_token.is_some(),
            "Connect this computer to a fleet before enabling sharing"
        );
        Ok(())
    }
    pub fn apply(&self, config: &mut Configuration) {
        config.policy = self.policy.clone();
    }
}

impl Agent {
    /// Persist owner rules and the reviewed storage request in the same settings
    /// transaction. The supervisor alone performs subsequent VM maintenance.
    pub async fn save_policy_with_storage(
        &self,
        policy: Policy,
        expected_revision: u64,
        plan: ChangePlan,
    ) -> Result<Snapshot> {
        let _operation = self.operation.lock().await;
        let current = self.store.load()?;
        let settings = SharingSettings {
            policy,
            revision: expected_revision,
        };
        settings.validate(&current)?;
        anyhow::ensure!(
            plan.maintenance
                .as_ref()
                .is_none_or(|review| review.kind == crate::storage_lifecycle::Kind::ResizeRemove),
            "Use the explicit storage recovery or deletion confirmation for this operation"
        );
        anyhow::ensure!(
            settings.policy.resources.disk_gib == plan.total_gib,
            "Sharing rules must use the reviewed storage allocation"
        );
        // CPU, memory and scheduling still use the observed host. Physical disk
        // capacity is checked by the shared storage planner against each picked
        // filesystem before anything is committed, not against the old pool.
        let mut host = crate::observe::observation(&self.store.directory, 0).resources;
        host.disk_gib = plan.total_gib.saturating_add(10);
        validate_policy(&settings.policy, &host).map_err(anyhow::Error::msg)?;
        self.apply_storage_settings(plan, Some(settings)).await
    }
}
