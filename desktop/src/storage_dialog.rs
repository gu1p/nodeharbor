use std::path::PathBuf;

pub fn selected_directory(path: Option<PathBuf>) -> Result<Option<String>, String> {
    let Some(path) = path else { return Ok(None) };
    if !path.is_absolute() {
        return Err("The selected storage folder must have an absolute path".into());
    }
    path.into_os_string()
        .into_string()
        .map(Some)
        .map_err(|_| "Choose a storage folder whose path uses valid UTF-8 characters".into())
}
