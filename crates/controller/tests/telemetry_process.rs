use axum::{body::Bytes, routing::post, Router};
use std::{future::IntoFuture, process::Stdio, time::Duration};

fn unused_address() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

#[tokio::test]
async fn the_controller_exports_real_otlp_and_keeps_metrics_off_the_public_router() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/traces", listener.local_addr().unwrap());
    let (send, mut batches) = tokio::sync::mpsc::channel(32);
    let collector = tokio::spawn({
        let app = Router::new().route(
            "/v1/traces",
            post(move |bytes: Bytes| {
                let send = send.clone();
                async move {
                    send.send(bytes).await.unwrap();
                    ([("content-type", "application/x-protobuf")], Bytes::new())
                }
            }),
        );
        async move {
            axum::serve(listener, app).into_future().await.unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let ui = directory.path().join("ui");
    std::fs::create_dir(&ui).unwrap();
    std::fs::write(ui.join("index.html"), "<!doctype html><title>Fleet</title>").unwrap();
    let token = directory.path().join("admin-token");
    std::fs::write(&token, "private-process-credential").unwrap();
    let api_address = unused_address();
    let metrics_address = unused_address();
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_nodeharbor-controller"))
        .args([
            "--database",
            "sqlite::memory:",
            "--listen",
            &api_address.to_string(),
            "--metrics-listen",
            &metrics_address.to_string(),
            "--admin-token-file",
        ])
        .arg(token)
        .arg("--ui-dir")
        .arg(ui)
        .env_remove("NODEHARBOR_CONFIG_FILE")
        .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", &endpoint)
        .env(
            "OTEL_RESOURCE_ATTRIBUTES",
            "service.namespace=testing,private.attribute=private-resource",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let base = format!("http://{api_address}");
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(status) = process.try_wait().unwrap() {
                panic!("Controller exited before accepting requests: {status}");
            }
            if client
                .get(format!("{base}/healthz"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let response = client
        .get(format!("{base}/api/v1/fleet?secret=private-query"))
        .bearer_auth("private-process-credential")
        .header(
            "traceparent",
            "00-11111111111111111111111111111111-2222222222222222-01",
        )
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(
        client
            .post(format!("{base}/api/v1/enroll"))
            .header("content-type", "application/json")
            .body(r#"{"code":"private-body",invalid}"#)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        client
            .get(format!("{base}/metrics"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let metrics = client
        .get(format!("http://{metrics_address}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("nodeharbor_http_requests_total"));
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let bytes = batches.recv().await.expect("The OTLP receiver was closed");
            let data = String::from_utf8_lossy(&bytes);
            assert!(!data.contains("private-process-credential"));
            assert!(!data.contains("private-query"));
            assert!(!data.contains("private-resource"));
            if data.contains("/api/v1/fleet") {
                assert!(data.contains("nodeharbor-controller"));
                assert!(data.contains("testing"));
                break;
            }
        }
    })
    .await
    .expect("The real SDK must export a completed request to OTLP");
    collector.abort();
    // Exporting to an unavailable collector must not block request handling.
    assert!(client
        .get(format!("{base}/healthz"))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    #[cfg(unix)]
    {
        let status = std::process::Command::new("kill")
            .args(["-TERM", &process.id().unwrap().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        let output = tokio::time::timeout(Duration::from_secs(12), process.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success());
        let logs = String::from_utf8(output.stdout).unwrap();
        for private in [
            "private-query",
            "private-body",
            "private-process-credential",
        ] {
            assert!(!logs.contains(private), "Completion logs exposed {private}");
        }
        let events: Vec<serde_json::Value> = logs
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let request = events
            .iter()
            .find(|event| event["fields"]["route"] == "/api/v1/fleet")
            .expect("API completion must be correlated with its real trace");
        assert_eq!(request["fields"]["event"], "http_request_completed");
        assert_eq!(request["fields"]["status"], 200);
        assert_eq!(
            request["fields"]["trace_id"],
            "11111111111111111111111111111111"
        );
        assert_eq!(request["fields"]["span_id"].as_str().unwrap().len(), 16);
        assert!(request["fields"]["duration_ms"].is_number());
        assert!(events
            .iter()
            .any(|event| event["fields"]["route"] == "/api/v1/enroll"
                && event["fields"]["status"] == 400));
        assert!(!events
            .iter()
            .any(|event| event["fields"]["route"] == "/healthz"
                || event["fields"]["route"] == "/readyz"));
        let operation = events
            .iter()
            .find(|event| event["fields"]["operation"] == "maintenance")
            .expect("Background operations must emit a correlated completion");
        assert_eq!(
            operation["fields"]["event"],
            "controller_operation_completed"
        );
        assert_eq!(operation["fields"]["outcome"], "success");
        assert_eq!(operation["fields"]["trace_id"].as_str().unwrap().len(), 32);
    }
    #[cfg(windows)]
    process.kill().await.unwrap();
}
