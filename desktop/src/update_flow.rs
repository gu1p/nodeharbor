#[async_trait::async_trait]
pub trait UpdateRuntime: Sync {
    async fn download(&self) -> Result<(), String>;
    async fn prepare(&self) -> Result<(), String>;
    async fn ready(&self) -> Result<bool, String>;
    async fn wait(&self);
    fn cancelled(&self) -> bool;
    async fn install(&self) -> Result<(), String>;
    async fn release(&self) -> Result<(), String>;
}

/// A successful install keeps maintenance held until the new app starts.
/// Every cancellation or failure after maintenance begins releases that hold.
pub async fn apply(runtime: &impl UpdateRuntime) -> Result<bool, String> {
    runtime.download().await?;
    if runtime.cancelled() {
        return Ok(false);
    }
    let result = async {
        runtime.prepare().await?;
        loop {
            if runtime.cancelled() {
                return Ok(false);
            }
            if runtime.ready().await? {
                break;
            }
            runtime.wait().await;
        }
        if runtime.cancelled() {
            return Ok(false);
        }
        runtime.install().await?;
        Ok(true)
    }
    .await;
    if result != Ok(true) {
        runtime.release().await.map_err(|error| {
            format!(
                "{}; restoring sharing failed: {error}",
                result
                    .as_ref()
                    .err()
                    .map(String::as_str)
                    .unwrap_or("Update cancelled")
            )
        })?;
    }
    result
}
