use std::future::Future;

pub trait StartupRegistration {
    fn is_enabled(&self) -> Result<bool, String>;
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
}

/// Resolve the OS setting before committing a request that the supervisor may
/// execute immediately. Roll back to the observed OS state on any failure.
pub async fn save_with_startup<T, F: Future<Output = Result<T, String>>>(
    startup: &impl StartupRegistration,
    enabled: bool,
    commit: impl FnOnce() -> F,
) -> Result<T, String> {
    let previous = startup.is_enabled()?;
    let changed = previous != enabled;
    let registration = if changed {
        startup.set_enabled(enabled)
    } else {
        Ok(())
    };
    let result = match registration {
        Ok(()) => commit().await,
        Err(error) => Err(format!("Could not update start at login: {error}")),
    };
    if let Err(error) = result {
        if changed {
            startup.set_enabled(previous).map_err(|rollback| {
                format!("{error}. Restoring the previous start-at-login setting failed: {rollback}")
            })?;
        }
        return Err(error);
    }
    result
}
