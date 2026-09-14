use crate::Desktop;
use nodeharbor_agent::{
    storage::{ChangePlan, Selection},
    Snapshot,
};
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub async fn preview_storage(
    state: State<'_, Desktop>,
    selections: Vec<Selection>,
    options: Option<nodeharbor_agent::storage_lifecycle::ReviewOptions>,
) -> Result<ChangePlan, String> {
    let _settings = state.settings.lock().await;
    state
        .agent
        .preview_storage_request(selections, options.unwrap_or_default())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_storage_recovery(
    state: State<'_, Desktop>,
    enabled: bool,
) -> Result<Snapshot, String> {
    let _settings = state.settings.lock().await;
    state
        .agent
        .set_storage_recovery(enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn retry_storage_maintenance(state: State<'_, Desktop>) -> Result<Snapshot, String> {
    state
        .agent
        .retry_storage_maintenance()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn apply_storage(
    state: State<'_, Desktop>,
    plan: ChangePlan,
) -> Result<Snapshot, String> {
    let _settings = state.settings.lock().await;
    state
        .agent
        .apply_storage(plan)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn choose_storage_directory<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose worker storage folder");
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_folder(move |selection| {
        let selection = selection
            .map(|path| path.into_path().map_err(|error| error.to_string()))
            .transpose()
            .and_then(crate::storage_dialog::selected_directory);
        let _ = sender.send(selection);
    });
    receiver
        .await
        .map_err(|_| "The storage folder picker closed unexpectedly".to_owned())?
}
