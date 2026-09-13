use nodeharbor_agent::Vm;
use nodeharbor_core::Resources;
use std::process::Command;

#[tokio::test]
#[ignore = "Creates a dedicated local Multipass VM; run explicitly on contributed hardware"]
async fn a_native_vm_honors_its_budget_and_contains_the_owned_worker_runtime() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.local/native-vm-acceptance");
    std::fs::create_dir_all(&root).unwrap();
    let receipt = root.join("device-id");
    let id = if receipt.exists() {
        std::fs::read_to_string(&receipt).unwrap()
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        std::fs::write(&receipt, &id).unwrap();
        id
    };
    let vm = Vm::local_in(&id, &root).unwrap();
    let resources = Resources {
        cpus: 2,
        memory_mib: 3072,
        disk_gib: 20,
    };
    if !vm.info().await.unwrap().installed {
        vm.create(&resources, &root, nodeharbor_agent::guest_files())
            .await
            .unwrap();
    } else if !vm.info().await.unwrap().running {
        vm.start().await.unwrap();
    }
    vm.verify_owner().await.unwrap();
    let result = Command::new("/usr/local/bin/multipass").args(["exec", &vm.name, "--", "sudo", "python3", "-c",
        "import json,os,pathlib; print(json.dumps({'cpus':os.cpu_count(),'memoryKiB':int(pathlib.Path('/proc/meminfo').read_text().splitlines()[0].split()[1]),'scripts':[pathlib.Path('/usr/local/lib/nodeharbor/'+p).is_file() for p in ['configure_worker.py','watchdog.py']]}))"]).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let guest: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(guest["cpus"], 2);
    assert!(guest["memoryKiB"].as_u64().unwrap() <= 3072 * 1024);
    assert_eq!(guest["scripts"], serde_json::json!([true, true]));
    vm.renew_lease().await.unwrap();
    vm.stop().await.unwrap();
    assert!(!vm.info().await.unwrap().running);
    println!(
        "Verified owned native VM {}: 2 CPUs, 3 GiB RAM, worker scripts, stop",
        vm.name
    );
}

#[tokio::test]
#[ignore = "Creates one owned Multipass VM to verify cancellation through the real hypervisor"]
async fn a_native_launch_can_be_interrupted_without_guest_networking() {
    use axum::{routing::post, Json, Router};
    use nodeharbor_agent::Agent;
    use serde_json::json;
    use std::{future::IntoFuture, time::Duration};

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.local/native-agent-cancel-acceptance");
    let agent = Agent::open(&root).unwrap();
    // This isolated enrollment server tests local lifecycle only; it cannot
    // register a Kubernetes node or qualify a worker in the real fleet.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let id = uuid::Uuid::new_v4().to_string();
    let enrollment = json!({"deviceId":id,"token":"native-lifecycle-test-credential-only"});
    let app = Router::new()
        .route(
            "/api/v1/enroll",
            post(move || {
                let value = enrollment.clone();
                async { Json(value) }
            }),
        )
        .route(
            "/api/v1/heartbeat",
            post(|| async { Json(json!({"remotePaused":false})) }),
        );
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    agent
        .enroll(&address, "isolated-native-test")
        .await
        .unwrap();
    let vm = Vm::local_in(&id, &root).unwrap();
    assert!(!vm.info().await.unwrap().installed);
    let mut policy = agent.store.load().unwrap().policy;
    policy.resources = Resources {
        cpus: 1,
        memory_mib: 2048,
        disk_gib: 15,
    };
    agent.save_policy(policy).await.unwrap();
    agent.action("prepare").await.unwrap();
    let supervisor = agent.clone();
    let mut prepare = tokio::spawn(async move { supervisor.tick().await });
    let observed = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            if vm.info().await?.running {
                return Ok::<_, anyhow::Error>(());
            }
            anyhow::ensure!(
                !prepare.is_finished(),
                "Preparation ended before the VM became visible"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await;
    agent.action("stop").await.unwrap();
    let interrupted = tokio::time::timeout(Duration::from_secs(30), &mut prepare).await;
    if interrupted.is_err() {
        prepare.abort();
    }
    let mut errors = vec![];
    // Continue the real supervisor after shutdown, including late hypervisor
    // state updates. A transient Stopped reply alone is insufficient evidence.
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Err(error) = agent.tick().await {
            errors.push(error.to_string());
        }
    }
    let final_state = vm.info().await.unwrap();
    if final_state.running {
        vm.stop_now().await.unwrap();
    }
    server.abort();
    observed
        .expect("The native VM must become visible")
        .unwrap();
    interrupted
        .expect("Stop now must interrupt the native launch")
        .unwrap()
        .unwrap();
    assert!(errors.is_empty(), "Supervisor errors: {errors:?}");
    assert!(
        !final_state.running,
        "The hypervisor must continue reporting Stopped after cancellation"
    );
    println!(
        "Verified native Agent cancellation and subsequent supervision of {}",
        vm.name
    );
}
