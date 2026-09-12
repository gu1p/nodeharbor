#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use nodeharbor_agent::{close_behavior, Agent, CloseBehavior, Snapshot, Store};
use nodeharbor_core::Policy;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, State,
};
use tauri_plugin_autostart::ManagerExt;

struct Desktop {
    agent: Agent,
    settings: tokio::sync::Mutex<()>,
    quitting: AtomicBool,
}

#[tauri::command]
async fn snapshot(state: State<'_, Desktop>) -> Result<Snapshot, String> {
    state.agent.snapshot().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_policy(
    app: AppHandle,
    state: State<'_, Desktop>,
    policy: Policy,
) -> Result<Snapshot, String> {
    let _settings = state.settings.lock().await;
    let previous = state.agent.store.load().map_err(|e| e.to_string())?.policy;
    let result = state
        .agent
        .save_policy(policy.clone())
        .await
        .map_err(|e| e.to_string())?;
    let autostart = app.autolaunch();
    let registered = if policy.start_at_login {
        autostart.enable()
    } else {
        autostart.disable()
    };
    if let Err(error) = registered {
        state.agent.store.update(|config| { config.policy = previous; Ok(()) })
            .map_err(|rollback| format!("Could not update start at login: {error}. Settings rollback failed: {rollback}"))?;
        return Err(format!(
            "Could not update start at login: {error}. Your previous settings were restored."
        ));
    }
    Ok(result)
}

#[tauri::command]
async fn worker_action(state: State<'_, Desktop>, action: String) -> Result<Snapshot, String> {
    state.agent.action(&action).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn enroll(state: State<'_, Desktop>, url: String, code: String) -> Result<Snapshot, String> {
    state
        .agent
        .enroll(&url, &code)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn fleet(state: State<'_, Desktop>) -> Result<Vec<Value>, String> {
    state.agent.fleet().await.map_err(|e| e.to_string())
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn request_close(app: &AppHandle, explicit_quit: bool) {
    let state = app.state::<Desktop>();
    let Ok(config) = state.agent.store.load() else {
        // A damaged configuration must stay visible instead of silently
        // abandoning a worker whose ownership cannot be established.
        show(app);
        return;
    };
    match close_behavior(
        config.policy.background,
        config.vm_created || config.prepare_requested,
        explicit_quit,
    ) {
        CloseBehavior::Hide => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
        }
        CloseBehavior::Exit => app.exit(0),
        CloseBehavior::DrainThenExit => {
            if state.quitting.swap(true, Ordering::SeqCst) {
                return;
            }
            let handle = app.clone();
            let agent = state.agent.clone();
            tauri::async_runtime::spawn(async move {
                if agent.action("pause").await.is_err() {
                    handle
                        .state::<Desktop>()
                        .quitting
                        .store(false, Ordering::SeqCst);
                    show(&handle);
                    return;
                }
                let deadline = Instant::now()
                    + Duration::from_secs(u64::from(config.policy.drain_seconds) + 150);
                loop {
                    // The supervisor owns all VM operations. In a connectivity
                    // failure, the guest lease expires within 120 seconds.
                    if let Ok(s) = agent.snapshot().await {
                        if !s.worker.running
                            && !["preparing", "connecting"].contains(&s.state.as_str())
                        {
                            break;
                        }
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                handle.exit(0);
            });
        }
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!(
            "NodeHarbor {} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("NODEHARBOR_COMMIT").unwrap_or("development")
        );
        return;
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if args.iter().any(|arg| arg == "--quit") {
                request_close(app, true);
            } else {
                show(app);
            }
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("NodeHarbor")
                .arg("--background")
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            snapshot,
            save_policy,
            worker_action,
            enroll,
            fleet
        ])
        .setup(|app| {
            if std::env::args().any(|arg| arg == "--quit") {
                app.handle().exit(0);
                return Ok(());
            }
            let directory = std::env::var_os("NODEHARBOR_CONFIG_DIR")
                .map(std::path::PathBuf::from)
                .map(Ok)
                .unwrap_or_else(Store::default_directory)?;
            let agent = Agent::open(&directory)?;
            let supervisor = agent.clone();
            app.manage(Desktop {
                agent,
                settings: tokio::sync::Mutex::new(()),
                quitting: AtomicBool::new(false),
            });
            let open = MenuItem::with_id(app, "open", "Open NodeHarbor", true, None::<&str>)?;
            let pause = MenuItem::with_id(app, "pause", "Pause sharing", true, None::<&str>)?;
            let resume = MenuItem::with_id(app, "resume", "Start sharing", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Drain and quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &pause, &resume, &quit])?;
            TrayIconBuilder::new()
                .icon(
                    app.default_window_icon()
                        .ok_or("Application icon is missing")?
                        .clone(),
                )
                .tooltip("NodeHarbor — your computer, your limits")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "quit" => request_close(app, true),
                    "pause" | "resume" => {
                        let handle = app.clone();
                        let action = event.id.as_ref().to_owned();
                        let agent = app.state::<Desktop>().agent.clone();
                        tauri::async_runtime::spawn(async move {
                            if agent.action(&action).await.is_err() {
                                show(&handle);
                            }
                        });
                    }
                    _ => {}
                })
                .build(app)?;
            tauri::async_runtime::spawn(async move {
                if let Err(error) = supervisor.run().await {
                    eprintln!("NodeHarbor supervisor: {error}");
                }
            });
            if !std::env::args().any(|arg| arg == "--background") {
                show(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                request_close(window.app_handle(), false);
            }
        })
        .build(tauri::generate_context!())
        .expect("Could not start NodeHarbor");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
            request_close(app, true);
        }
    });
}
