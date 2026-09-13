use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use nodeharbor_controller::{metrics_router, router, State, Telemetry};
use opentelemetry::trace::SpanKind;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tower::ServiceExt;

#[tokio::test]
async fn private_metrics_and_http_traces_keep_routes_but_exclude_identity_and_credentials() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let telemetry = Telemetry::new(Some(provider.clone())).unwrap();
    let state = State::open("sqlite::memory:", "private-admin")
        .await
        .unwrap()
        .with_telemetry(telemetry);
    let app = router(state.clone());
    for (method, path, status) in [
        ("GET", "/api/v1/fleet?token=private-query", StatusCode::OK),
        (
            "POST",
            "/api/v1/devices/private-device/pause",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "PRIVATE-METHOD",
            "/api/v1/fleet",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        ("GET", "/private-unmatched-path", StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(
                        "authorization",
                        if method == "GET" {
                            "Bearer private-admin"
                        } else {
                            "Bearer private-credential"
                        },
                    )
                    .header("x-auth-request-email", "private-owner@example.com")
                    .header(
                        "traceparent",
                        "00-11111111111111111111111111111111-2222222222222222-01",
                    )
                    .header("tracestate", "private=private-tracestate")
                    .header("baggage", "secret=private-baggage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 4);
    assert!(spans.iter().all(|span| span.span_kind == SpanKind::Server));
    assert!(
        spans
            .iter()
            .all(|span| span.span_context.trace_id().to_string()
                == "11111111111111111111111111111111")
    );
    assert!(spans
        .iter()
        .all(|span| span.parent_span_id.to_string() == "2222222222222222"));
    let encoded = format!("{spans:?}");
    for secret in [
        "private-query",
        "private-device",
        "private-admin",
        "private-credential",
        "private-owner",
        "private-baggage",
        "private-tracestate",
        "PRIVATE-METHOD",
        "private-unmatched-path",
    ] {
        assert!(!encoded.contains(secret), "Trace exposed {secret}");
    }
    assert!(encoded.contains("/api/v1/devices/{id}/{action}"));
    assert!(encoded.contains("OTHER"));
    let response = metrics_router(state)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let metrics = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(metrics.contains("nodeharbor_http_requests_total"));
    assert!(metrics.contains("nodeharbor_http_request_duration_seconds_bucket"));
    assert!(metrics.contains("nodeharbor_devices 0"));
    assert!(metrics.contains("method=\"OTHER\""));
    assert!(!metrics.contains("private-"));
    assert_eq!(
        app.oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn maintenance_records_real_success_and_failure_without_exporting_error_details() {
    struct IdleHealth;
    #[async_trait::async_trait]
    impl nodeharbor_controller::HealthBackend for IdleHealth {
        async fn observe(
            &self,
            _: &nodeharbor_controller::DeviceIdentity,
            _: &nodeharbor_core::Resources,
        ) -> anyhow::Result<f64> {
            Ok(0.0)
        }
        async fn place(
            &self,
            _: &nodeharbor_controller::DeviceIdentity,
            _: bool,
            _: bool,
            _: bool,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let state = State::open("sqlite::memory:", "admin")
        .await
        .unwrap()
        .with_telemetry(Telemetry::new(Some(provider.clone())).unwrap());
    let controller = nodeharbor_controller::ConfiguredController {
        state: state.clone(),
        provisioner: None,
        reconciler: Some(nodeharbor_controller::Reconciler::new(
            state.clone(),
            std::sync::Arc::new(IdleHealth),
        )),
    };
    controller.maintain_once().await.unwrap();
    let response = metrics_router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let metrics = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(metrics.contains("nodeharbor_maintenance_last_success_unixtime_seconds"));
    assert!(metrics
        .contains("nodeharbor_operations_total{operation=\"maintenance\",outcome=\"success\"} 1"));
    sqlx::query("DROP TABLE health_samples")
        .execute(&state.db)
        .await
        .unwrap();
    assert!(controller.maintain_once().await.is_err());
    let response = metrics_router(state)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let metrics = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(metrics
        .contains("nodeharbor_operations_total{operation=\"maintenance\",outcome=\"error\"} 1"));
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    assert!(spans.iter().any(|s| s.name == "maintenance"));
    assert!(spans.iter().any(|s| s.name == "reconcile"));
    assert_eq!(
        spans
            .iter()
            .filter(|s| matches!(s.status, opentelemetry::trace::Status::Error { .. }))
            .count(),
        2
    );
    assert!(!format!("{spans:?}").contains("health_samples"));
}

#[tokio::test]
async fn infrastructure_spans_connect_to_the_operation_without_exporting_resource_paths() {
    use axum::{
        extract::State as Extract,
        http::{HeaderMap, Method, Uri},
        Json, Router,
    };
    use nodeharbor_controller::{
        ApiClient, Cluster, ClusterConfig, DeviceIdentity, Operation, Peer, Provisioner,
    };
    use serde_json::{json, Value};
    use std::{
        future::IntoFuture,
        sync::{Arc, Mutex},
    };
    const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
    let parents = Arc::new(Mutex::new(Vec::<String>::new()));
    async fn upstream(
        Extract(parents): Extract<Arc<Mutex<Vec<String>>>>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> Json<Value> {
        parents
            .lock()
            .unwrap()
            .push(headers.get("traceparent").unwrap().to_str().unwrap().into());
        Json(
            if method == Method::GET && uri.path().starts_with("/api/v1/nodes/") {
                json!({"metadata":{"name":"nodeharbor-9511182e9c484d20a15b1da8bb441386","labels":{"nodeharbor.sikalio.dev/device":ID}}})
            } else {
                json!({"items":[]})
            },
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(upstream).with_state(parents.clone()),
        )
        .into_future(),
    );
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let telemetry = Telemetry::new(Some(provider.clone())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "private-infrastructure-token").unwrap();
    let state = State::open("sqlite::memory:", "admin").await.unwrap();
    let provisioner = Provisioner::new(
        ClusterConfig {
            server_url: "https://cluster.example.com:6443".into(),
            ca_hash: "a".repeat(64),
            netbird_management_url: "https://network.example.com".into(),
            workers_group_id: "private-group".into(),
        },
        ApiClient::new(&base, &token, "Bearer", None)
            .unwrap()
            .with_telemetry(telemetry.clone(), Peer::Kubernetes),
        ApiClient::new(&base, &token, "Token", None)
            .unwrap()
            .with_telemetry(telemetry.clone(), Peer::NetBird),
        state.db,
    )
    .await
    .unwrap();
    telemetry
        .operation(
            Operation::Maintenance,
            provisioner.drain(&DeviceIdentity {
                id: ID.into(),
                architecture: "arm64".into(),
            }),
        )
        .await
        .unwrap();
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    let operation = spans
        .iter()
        .find(|span| span.name == "maintenance")
        .unwrap();
    let clients: Vec<_> = spans
        .iter()
        .filter(|span| span.span_kind == SpanKind::Client)
        .collect();
    assert_eq!(clients.len(), 3);
    assert_eq!(parents.lock().unwrap().len(), 3);
    for span in clients {
        assert_eq!(span.parent_span_id, operation.span_context.span_id());
        assert_eq!(
            span.span_context.trace_id(),
            operation.span_context.trace_id()
        );
        assert!(format!("{:?}", span.attributes).contains("kubernetes"));
        assert!(!format!("{span:?}").contains(ID));
        assert!(!format!("{span:?}").contains("private-"));
    }
    server.abort();
}
