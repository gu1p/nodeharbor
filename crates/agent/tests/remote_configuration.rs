use nodeharbor_agent::Agent;
use nodeharbor_core::configuration::{ConfigurationCommand, ConfigurationEdit};

fn command(revision: u64) -> ConfigurationCommand {
    ConfigurationCommand {
        edit: ConfigurationEdit {
            operation: None,
            request_id: "request-726".into(),
            expected_revision: revision,
            policy: nodeharbor_core::Policy {
                resources: nodeharbor_core::Resources {
                    cpus: 1,
                    memory_mib: 2048,
                    disk_gib: 30,
                },
                ..Default::default()
            },
            acknowledge_interruption: true,
        },
        actor: "administrator@example.test".into(),
        requested_at: "2026-09-13T12:00:00Z".into(),
    }
}
#[tokio::test]
async fn consent_defaults_off_persists_and_revocation_invalidates_pending_and_in_flight_requests() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    assert!(!agent.configuration_report().unwrap().consent);
    agent.set_remote_consent(true).await.unwrap();
    let revision = agent.configuration_report().unwrap().revision;
    let request = command(revision);
    agent.receive_configuration(request.clone()).unwrap();
    assert_eq!(
        agent
            .configuration_report()
            .unwrap()
            .receipts
            .last()
            .unwrap()
            .status,
        "pending"
    );
    agent.set_remote_consent(false).await.unwrap();
    let reopened = Agent::open(dir.path()).unwrap();
    assert!(!reopened.configuration_report().unwrap().consent);
    assert_eq!(
        reopened
            .configuration_report()
            .unwrap()
            .receipts
            .last()
            .unwrap()
            .status,
        "rejected"
    );
    reopened.set_remote_consent(true).await.unwrap();
    assert!(reopened.receive_configuration(command(revision)).is_err());
    assert!(reopened.store.load().unwrap().remote.pending.is_none());
}
#[tokio::test]
async fn local_edits_advance_revision_and_cannot_be_overwritten_by_queued_requests() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent.set_remote_consent(true).await.unwrap();
    let revision = agent.configuration_report().unwrap().revision;
    agent.receive_configuration(command(revision)).unwrap();
    agent
        .store
        .update(|c| {
            c.policy.idle_only = true;
            Ok(())
        })
        .unwrap();
    assert!(agent.store.load().unwrap().remote.pending.is_none());
    assert!(agent.configuration_report().unwrap().revision > revision);
    assert!(agent.receive_configuration(command(revision)).is_err());
    assert!(agent.store.load().unwrap().policy.idle_only);
}
#[tokio::test]
async fn retries_are_idempotent_but_reusing_a_request_id_with_different_values_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent.set_remote_consent(true).await.unwrap();
    let request = command(agent.configuration_report().unwrap().revision);
    agent.receive_configuration(request.clone()).unwrap();
    agent.receive_configuration(request.clone()).unwrap();
    assert_eq!(agent.configuration_report().unwrap().receipts.len(), 1);
    let mut changed = request;
    changed.edit.policy.idle_only = true;
    assert!(agent.receive_configuration(changed).is_err());
}

#[tokio::test]
async fn revocation_during_an_authenticated_heartbeat_rejects_the_late_response() {
    use axum::{routing::post, Json, Router};
    use serde_json::{json, Value};
    use std::{future::IntoFuture, sync::Arc};
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    agent
        .store
        .update(|c| {
            c.controller_url = Some(url);
            c.device_token = Some("node-token".into());
            Ok(())
        })
        .unwrap();
    agent.set_remote_consent(true).await.unwrap();
    let request = command(agent.configuration_report().unwrap().revision);
    let endpoint = {
        let entered = entered.clone();
        let release = release.clone();
        move |headers: axum::http::HeaderMap, Json(_): Json<Value>| {
            let entered = entered.clone();
            let release = release.clone();
            let request = request.clone();
            async move {
                assert_eq!(headers["authorization"], "Bearer node-token");
                entered.notify_one();
                release.notified().await;
                Json(json!({"remotePaused":false,"configurationRequest":request}))
            }
        }
    };
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route("/api/v1/heartbeat", post(endpoint)),
        )
        .into_future(),
    );
    let task = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.tick().await })
    };
    entered.notified().await;
    agent.set_remote_consent(false).await.unwrap();
    release.notify_one();
    let _ = task.await.unwrap();
    assert!(agent.store.load().unwrap().remote.pending.is_none());
    assert!(!agent.configuration_report().unwrap().consent);
    server.abort();
}

#[tokio::test]
async fn disk_runtime_failure_preserves_owner_policy_and_never_reports_an_applied_allocation() {
    use async_trait::async_trait;
    use nodeharbor_agent::{CommandOutput, Runner};
    use serde_json::json;
    use std::sync::Arc;
    struct FailingDisk;
    #[async_trait]
    impl Runner for FailingDisk {
        fn provider(&self) -> nodeharbor_agent::VmProvider {
            nodeharbor_agent::VmProvider::Lima
        }
        async fn run(
            &self,
            args: &[String],
            _: Option<Vec<u8>>,
            _: u64,
        ) -> anyhow::Result<CommandOutput> {
            let failed = args.iter().any(|a| a.starts_with("--disk="));
            Ok(CommandOutput {
                success: !failed,
                stdout: if args[0] == "list" {
                    json!({"name":"worker","status":"Stopped"}).to_string()
                } else {
                    String::new()
                },
                stderr: if failed {
                    "disk capacity changed".into()
                } else {
                    String::new()
                },
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open_with_runner(dir.path(), Arc::new(FailingDisk)).unwrap();
    agent
        .store
        .update(|c| {
            c.device_id = "9511182e-9c48-4d20-a15b-1da8bb441386".into();
            c.device_token = Some("test".into());
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.format_version = 2;
            c.vm_provider = nodeharbor_agent::VmProvider::Lima;
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(c.policy.resources.clone());
            Ok(())
        })
        .unwrap();
    std::fs::write(dir.path().join("worker.receipt.json"),json!({"version":2,"provider":"lima","deviceId":"9511182e-9c48-4d20-a15b-1da8bb441386","name":"worker"}).to_string()).unwrap();
    agent.set_remote_consent(true).await.unwrap();
    let mut request = command(agent.configuration_report().unwrap().revision);
    request.edit.policy.resources.disk_gib = 35;
    agent.receive_configuration(request).unwrap();
    let _ = agent.tick().await;
    let report = agent.configuration_report().unwrap();
    assert_eq!(report.receipts.last().unwrap().status, "rejected");
    assert!(report
        .receipts
        .last()
        .unwrap()
        .error
        .as_ref()
        .unwrap()
        .contains("locally"));
    assert_eq!(agent.store.load().unwrap().policy.resources.disk_gib, 30);
    assert!(report.allocated_resources.is_none());
    assert!(agent.store.load().unwrap().remote.repair_required);
}

#[tokio::test]
async fn an_owner_stop_cancels_a_request_even_when_sharing_was_already_off() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent.set_remote_consent(true).await.unwrap();
    agent
        .receive_configuration(command(agent.configuration_report().unwrap().revision))
        .unwrap();
    agent.action("stop").await.unwrap();
    assert!(agent.store.load().unwrap().remote.pending.is_none());
}

#[tokio::test]
async fn a_supervisor_stop_at_the_owner_deadline_does_not_cancel_the_edit_it_is_draining_for() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent
        .store
        .update(|c| {
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.device_token = Some("test".into());
            Ok(())
        })
        .unwrap();
    agent.set_remote_consent(true).await.unwrap();
    agent
        .receive_configuration(command(agent.configuration_report().unwrap().revision))
        .unwrap();
    agent
        .store
        .update(|c| {
            c.draining_since = Some(1);
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    assert!(
        agent.store.load().unwrap().remote.pending.is_some(),
        "the supervisor's deadline shutdown must preserve the authorized edit"
    );
}

#[tokio::test]
async fn a_stale_local_form_cannot_overwrite_an_applied_remote_choice() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    let snapshot = agent.snapshot().await.unwrap();
    agent
        .store
        .update(|c| {
            c.policy.idle_only = true;
            Ok(())
        })
        .unwrap();
    assert!(agent
        .save_policy_versioned(snapshot.policy, Some(snapshot.configuration.revision))
        .await
        .is_err());
    assert!(agent.store.load().unwrap().policy.idle_only);
}

struct RuntimeFixture {
    running: std::sync::atomic::AtomicBool,
    calls: std::sync::Mutex<Vec<Vec<String>>>,
    mutation_started: Option<std::sync::Arc<tokio::sync::Notify>>,
}
#[async_trait::async_trait]
impl nodeharbor_agent::Runner for RuntimeFixture {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<nodeharbor_agent::CommandOutput> {
        use std::sync::atomic::Ordering;
        self.calls.lock().unwrap().push(args.to_vec());
        let stdout = if args[0] == "list" {
            serde_json::json!({"list":[{"name":"nodeharbor-9511182e9c484d20a15b1da8bb441386","state":if self.running.load(Ordering::SeqCst){"Running"}else{"Stopped"}}]}).to_string()
        } else if args
            .last()
            .is_some_and(|a| a == "/etc/nodeharbor/device-id")
        {
            "9511182e-9c48-4d20-a15b-1da8bb441386".into()
        } else {
            String::new()
        };
        if args[0] == "stop" {
            self.running.store(false, Ordering::SeqCst);
        }
        if args[0] == "set" {
            if let Some(notify) = &self.mutation_started {
                notify.notify_one();
                std::future::pending::<()>().await;
            }
        }
        Ok(nodeharbor_agent::CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}
fn owned_agent(dir: &std::path::Path, host: std::sync::Arc<RuntimeFixture>) -> Agent {
    let agent = Agent::open_with_runner(dir, host).unwrap();
    agent
        .store
        .update(|c| {
            c.device_id = "9511182e-9c48-4d20-a15b-1da8bb441386".into();
            c.device_token = Some("test".into());
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(c.policy.resources.clone());
            c.policy.drain_seconds = 0;
            Ok(())
        })
        .unwrap();
    std::fs::write(dir.join("nodeharbor-9511182e9c484d20a15b1da8bb441386.receipt.json"),serde_json::json!({"version":1,"deviceId":"9511182e-9c48-4d20-a15b-1da8bb441386","name":"nodeharbor-9511182e9c484d20a15b1da8bb441386"}).to_string()).unwrap();
    agent
}
#[tokio::test]
async fn remote_resource_changes_stop_the_owned_worker_then_report_the_acknowledged_values() {
    use std::sync::{atomic::AtomicBool, Arc, Mutex};
    let host = Arc::new(RuntimeFixture {
        running: AtomicBool::new(true),
        calls: Mutex::new(vec![]),
        mutation_started: None,
    });
    let dir = tempfile::tempdir().unwrap();
    let agent = owned_agent(dir.path(), host.clone());
    agent.set_remote_consent(true).await.unwrap();
    let mut request = command(agent.configuration_report().unwrap().revision);
    request.edit.policy.resources = nodeharbor_core::Resources {
        cpus: 1,
        memory_mib: 2048,
        disk_gib: 30,
    };
    let desired = request.edit.policy.resources.clone();
    agent.receive_configuration(request).unwrap();
    for _ in 0..3 {
        let _ = agent.tick().await;
    }
    let saved = agent.store.load().unwrap();
    assert_eq!(saved.allocated_resources, Some(desired));
    assert_eq!(saved.remote.receipts.last().unwrap().status, "applied");
    assert!(!saved.policy.enabled);
    let calls = host.calls.lock().unwrap();
    assert!(
        calls.iter().position(|a| a[0] == "stop").unwrap()
            < calls.iter().position(|a| a[0] == "set").unwrap()
    );
    assert!(!calls
        .iter()
        .any(|a| ["launch", "delete", "start"].contains(&a[0].as_str())));
}
#[tokio::test]
async fn revocation_during_runtime_application_stops_subsequent_commands_and_preserves_uncertainty()
{
    use std::sync::{atomic::AtomicBool, Arc, Mutex};
    let started = Arc::new(tokio::sync::Notify::new());
    let host = Arc::new(RuntimeFixture {
        running: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
        mutation_started: Some(started.clone()),
    });
    let dir = tempfile::tempdir().unwrap();
    let agent = owned_agent(dir.path(), host.clone());
    agent.set_remote_consent(true).await.unwrap();
    let mut request = command(agent.configuration_report().unwrap().revision);
    request.edit.policy.resources.cpus = 1;
    agent.receive_configuration(request).unwrap();
    let task = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.tick().await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    agent.set_remote_consent(false).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let saved = agent.store.load().unwrap();
    assert!(!saved.remote.consent);
    assert!(saved.remote.repair_required);
    assert_eq!(saved.policy.resources.cpus, 2);
    assert!(saved.allocated_resources.is_none());
    assert_eq!(
        host.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a[0] == "set")
            .count(),
        1
    );
    assert_eq!(saved.remote.receipts.last().unwrap().status, "rejected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_snapshots_never_pair_an_older_policy_with_a_newer_configuration_revision() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = stop.clone();
        let store = agent.store.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                store
                    .update(|c| {
                        c.policy.idle_after_minutes = c.policy.idle_after_minutes.saturating_add(1);
                        Ok(())
                    })
                    .unwrap();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    };
    let mut coherent = true;
    for _ in 0..8 {
        let snapshot = agent.snapshot().await.unwrap();
        coherent &= Some(snapshot.policy) == snapshot.configuration.policy;
    }
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    assert!(
        coherent,
        "snapshot policy and its versioned report must come from the same atomic settings read"
    );
}

#[tokio::test]
async fn stale_local_replacement_confirmation_cannot_overwrite_newer_settings() {
    use std::sync::{atomic::AtomicBool, Arc, Mutex};
    let host = Arc::new(RuntimeFixture {
        running: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
        mutation_started: None,
    });
    let dir = tempfile::tempdir().unwrap();
    let agent = owned_agent(dir.path(), host);
    let snapshot = agent.snapshot().await.unwrap();
    agent
        .store
        .update(|c| {
            c.policy.idle_only = true;
            Ok(())
        })
        .unwrap();
    assert!(agent
        .recreate_worker_versioned(snapshot.policy, Some(snapshot.configuration.revision))
        .await
        .is_err());
    let config = agent.store.load().unwrap();
    assert!(config.policy.idle_only);
    assert!(config.recreation.is_none());
}

#[tokio::test]
async fn remote_disk_growth_requires_a_runtime_with_a_known_host_storage_location() {
    use std::sync::{atomic::AtomicBool, Arc, Mutex};
    let host = Arc::new(RuntimeFixture {
        running: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
        mutation_started: None,
    });
    let dir = tempfile::tempdir().unwrap();
    let agent = owned_agent(dir.path(), host.clone());
    agent.set_remote_consent(true).await.unwrap();
    let mut request = command(agent.configuration_report().unwrap().revision);
    request.edit.policy.resources.disk_gib = 35;
    assert!(agent.receive_configuration(request).is_err(),"Multipass does not expose the physical storage location needed for remote capacity validation");
    assert!(host.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lima_disk_growth_uses_owned_runtime_storage_and_returns_effective_values() {
    use std::sync::{Arc, Mutex};
    struct Lima {
        calls: Mutex<Vec<Vec<String>>>,
    }
    #[async_trait::async_trait]
    impl nodeharbor_agent::Runner for Lima {
        fn provider(&self) -> nodeharbor_agent::VmProvider {
            nodeharbor_agent::VmProvider::Lima
        }
        async fn run(
            &self,
            args: &[String],
            _: Option<Vec<u8>>,
            _: u64,
        ) -> anyhow::Result<nodeharbor_agent::CommandOutput> {
            self.calls.lock().unwrap().push(args.to_vec());
            Ok(nodeharbor_agent::CommandOutput {
                success: true,
                stdout: if args[0] == "list" {
                    serde_json::json!({"name":"worker","status":"Stopped"}).to_string()
                } else {
                    String::new()
                },
                stderr: String::new(),
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(Lima {
        calls: Mutex::new(vec![]),
    });
    let agent = Agent::open_with_runner(dir.path(), runtime.clone()).unwrap();
    agent
        .store
        .update(|c| {
            c.vm_provider = nodeharbor_agent::VmProvider::Lima;
            c.format_version = 2;
            c.vm_created = true;
            c.vm_configured = true;
            c.device_token = Some("test".into());
            c.controller_url = Some("http://127.0.0.1:9".into());
            c.allocated_resources = Some(c.policy.resources.clone());
            Ok(())
        })
        .unwrap();
    let id = agent.store.load().unwrap().device_id;
    std::fs::write(
        dir.path().join("worker.receipt.json"),
        serde_json::json!({"version":2,"provider":"lima","name":"worker","deviceId":id})
            .to_string(),
    )
    .unwrap();
    agent.set_remote_consent(true).await.unwrap();
    let mut request = command(agent.configuration_report().unwrap().revision);
    request.edit.policy.resources.disk_gib = 35;
    assert!(
        agent
            .configuration_report()
            .unwrap()
            .capabilities
            .disk_growth
    );
    agent.receive_configuration(request).unwrap();
    agent.tick().await.unwrap();
    let report = agent.configuration_report().unwrap();
    assert_eq!(report.receipts.last().unwrap().status, "applied");
    assert_eq!(report.allocated_resources.unwrap().disk_gib, 35);
    assert!(runtime
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|a| a == &["edit", "worker", "--cpus=1", "--memory=2", "--disk=35"]));
}

#[tokio::test]
async fn local_storage_choices_and_recovery_preferences_invalidate_pending_remote_edits() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent.set_remote_consent(true).await.unwrap();
    let revision = agent.configuration_report().unwrap().revision;
    agent.receive_configuration(command(revision)).unwrap();
    agent.set_storage_recovery(true).await.unwrap();
    assert!(agent.store.load().unwrap().remote.pending.is_none());
    assert!(agent.configuration_report().unwrap().revision > revision);
    let revision = agent.configuration_report().unwrap().revision;
    let mut next = command(revision);
    next.edit.request_id = "storage-local-726".into();
    agent.receive_configuration(next).unwrap();
    agent
        .store
        .update(|c| {
            c.format_version = 4;
            c.storage_revision += 1;
            Ok(())
        })
        .unwrap();
    assert!(agent.store.load().unwrap().remote.pending.is_none());
}

#[tokio::test]
async fn remote_consent_requires_a_settings_format_that_older_agents_cannot_apply_without_authority(
) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent.set_remote_consent(true).await.unwrap();
    assert!(
        agent.store.load().unwrap().format_version >= 6,
        "Older agents cannot understand the remote authority attached to pending storage journals"
    );
    agent.set_remote_consent(false).await.unwrap();
    let reopened = Agent::open(dir.path()).unwrap();
    assert!(!reopened.configuration_report().unwrap().consent);
    assert!(reopened.store.load().unwrap().format_version >= 6);
}
