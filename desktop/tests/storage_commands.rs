#[path = "../src/storage_commands.rs"]
mod storage_commands;
#[path = "../src/storage_dialog.rs"]
mod storage_dialog;

use nodeharbor_agent::{Agent, Store};
use serde_json::{json, Value};
use tauri::{test, webview::InvokeRequest};

struct Desktop {
    agent: Agent,
    settings: tokio::sync::Mutex<()>,
}

fn invoke(command: &str, body: Value) -> (Result<Value, Value>, Vec<u8>, Vec<u8>) {
    let directory = tempfile::tempdir().unwrap();
    // Existing Multipass settings must stay intact even on a new Lima host.
    let store = Store::open(directory.path()).unwrap();
    let before = std::fs::read(directory.path().join("config.json")).unwrap();
    let app = test::mock_builder()
        .manage(Desktop {
            agent: Agent::open(directory.path()).unwrap(),
            settings: tokio::sync::Mutex::new(()),
        })
        .invoke_handler(tauri::generate_handler![
            storage_commands::preview_storage,
            storage_commands::apply_storage,
            storage_commands::set_storage_recovery,
            storage_commands::retry_storage_maintenance,
            storage_commands::choose_storage_directory,
        ])
        .build(test::mock_context(test::noop_assets()))
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let response = test::get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: test::INVOKE_KEY.to_owned(),
        },
    )
    .map(|body| body.deserialize::<Value>().unwrap());
    let after = std::fs::read(store.directory.join("config.json")).unwrap();
    (response, before, after)
}

#[test]
fn preview_command_preserves_the_agents_unsupported_runtime_error_and_settings() {
    let (result, before, after) = invoke("preview_storage", json!({"selections": []}));
    assert!(result.unwrap_err().as_str().unwrap().contains("Multipass"));
    assert_eq!(before, after);
}

#[test]
fn apply_command_cannot_bypass_the_agents_runtime_and_owner_validation() {
    let (result, before, after) = invoke(
        "apply_storage",
        json!({"plan": {"revision": 0, "locations": [], "totalGib": 999, "requiresRestart": false}}),
    );
    assert!(result.unwrap_err().as_str().unwrap().contains("Multipass"));
    assert_eq!(before, after);
}

#[test]
fn malformed_selections_are_rejected_at_the_ipc_boundary_without_mutation() {
    let (result, before, after) = invoke(
        "preview_storage",
        json!({"selections": [{"directory": "/storage", "allocationGib": -1}]}),
    );
    let error = result.unwrap_err();
    assert!(error.as_str().unwrap().contains("invalid args"));
    assert_eq!(before, after);
}
