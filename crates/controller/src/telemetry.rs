//! Explicit, bounded instrumentation. Request bodies, credentials, resource IDs,
//! raw URLs, baggage, and exception messages are never span attributes.
use crate::State;
use anyhow::Result;
use axum::{
    extract::{MatchedPath, Request, State as Extract},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use opentelemetry::{
    propagation::{Extractor, TextMapPropagator},
    trace::{FutureExt, SpanKind, Status, TraceContextExt, Tracer, TracerProvider},
    Context, KeyValue,
};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::SdkTracerProvider, Resource};
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub enum Operation {
    Maintenance,
    Reconcile,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Maintenance => "maintenance",
            Self::Reconcile => "reconcile",
        }
    }
}
#[derive(Clone, Copy)]
pub enum Peer {
    Kubernetes,
    NetBird,
}
impl Peer {
    fn name(self) -> &'static str {
        match self {
            Self::Kubernetes => "kubernetes",
            Self::NetBird => "netbird",
        }
    }
}
struct Metrics {
    registry: Registry,
    requests: IntCounterVec,
    duration: HistogramVec,
    upstream: IntCounterVec,
    operations: IntCounterVec,
    last_success: IntGauge,
    devices: IntGauge,
    ci: IntGauge,
    services: IntGauge,
}
#[derive(Clone)]
pub struct Telemetry {
    provider: SdkTracerProvider,
    metrics: Arc<Metrics>,
}
impl Telemetry {
    pub fn new(provider: Option<SdkTracerProvider>) -> Result<Self> {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "nodeharbor_http_requests_total",
                "Completed controller HTTP requests",
            ),
            &["method", "route", "status"],
        )?;
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "nodeharbor_http_request_duration_seconds",
                "Controller request duration",
            )
            .buckets(vec![0.005, 0.025, 0.1, 0.5, 2.0, 10.0, 30.0]),
            &["method", "route"],
        )?;
        let upstream = IntCounterVec::new(
            Opts::new(
                "nodeharbor_infrastructure_requests_total",
                "Infrastructure API requests",
            ),
            &["peer", "method", "status"],
        )?;
        let operations = IntCounterVec::new(
            Opts::new("nodeharbor_operations_total", "Controller operations"),
            &["operation", "outcome"],
        )?;
        let last_success = IntGauge::new(
            "nodeharbor_maintenance_last_success_unixtime_seconds",
            "Last fully successful maintenance pass",
        )?;
        let devices = IntGauge::new(
            "nodeharbor_devices",
            "Enrolled devices whose access has not been revoked",
        )?;
        let ci = IntGauge::new(
            "nodeharbor_devices_eligible_ci",
            "Devices with confirmed CI admission",
        )?;
        let services = IntGauge::new(
            "nodeharbor_devices_eligible_services",
            "Devices with confirmed service admission",
        )?;
        for collector in [
            Box::new(requests.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(duration.clone()),
            Box::new(upstream.clone()),
            Box::new(operations.clone()),
            Box::new(last_success.clone()),
            Box::new(devices.clone()),
            Box::new(ci.clone()),
            Box::new(services.clone()),
        ] {
            registry.register(collector)?;
        }
        Ok(Self {
            provider: provider.unwrap_or_else(|| {
                SdkTracerProvider::builder()
                    .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOff)
                    .build()
            }),
            metrics: Arc::new(Metrics {
                registry,
                requests,
                duration,
                upstream,
                operations,
                last_success,
                devices,
                ci,
                services,
            }),
        })
    }
    /// Called on a blocking thread because the SDK's batch exporter owns its HTTP client.
    pub fn from_env() -> Result<Self> {
        use opentelemetry_otlp::WithExportConfig;
        let enabled = [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        ]
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.is_empty()));
        if !enabled {
            return Self::new(None);
        }
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .with_timeout(Duration::from_secs(5))
            .build()?;
        let mut attributes = vec![
            KeyValue::new("service.name", "nodeharbor-controller"),
            KeyValue::new("service.version", nodeharbor_core::build_version()),
        ];
        // Keep only deployment metadata explicitly configured by the operator.
        for item in std::env::var("OTEL_RESOURCE_ATTRIBUTES")
            .unwrap_or_default()
            .split(',')
        {
            if let Some((key, value)) = item.split_once('=') {
                if [
                    "service.namespace",
                    "service.instance.id",
                    "deployment.environment.name",
                    "k8s.namespace.name",
                    "k8s.pod.name",
                ]
                .contains(&key.trim())
                {
                    attributes.push(KeyValue::new(
                        key.trim().to_owned(),
                        value.trim().to_owned(),
                    ));
                }
            }
        }
        Self::new(Some(
            SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(
                    Resource::builder_empty()
                        .with_attributes(attributes)
                        .build(),
                )
                .build(),
        ))
    }
    pub fn shutdown(&self) -> Result<()> {
        self.provider.shutdown().map_err(Into::into)
    }
    fn context(
        &self,
        name: &str,
        kind: SpanKind,
        attributes: Vec<KeyValue>,
        parent: &Context,
    ) -> Context {
        let tracer = self.provider.tracer("nodeharbor-controller");
        let span = tracer.build_with_context(
            tracer
                .span_builder(name.to_owned())
                .with_kind(kind)
                .with_attributes(attributes),
            parent,
        );
        parent.with_span(span)
    }
    pub async fn operation<T>(
        &self,
        operation: Operation,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let name = operation.name();
        let context = self.context(
            name,
            SpanKind::Internal,
            vec![KeyValue::new("sikalio.operation", name)],
            &Context::current(),
        );
        let result = future.with_context(context.clone()).await;
        self.metrics
            .operations
            .with_label_values(&[name, if result.is_ok() { "success" } else { "error" }])
            .inc();
        if result.is_err() {
            context.span().set_status(Status::error(""));
            context
                .span()
                .set_attribute(KeyValue::new("error.type", "operation_failed"));
        } else if matches!(operation, Operation::Maintenance) {
            self.metrics
                .last_success
                .set(chrono::Utc::now().timestamp());
        }
        context.span().end();
        result
    }
    pub(crate) async fn infrastructure(
        &self,
        peer: Peer,
        method: &str,
        future: impl Future<Output = Result<(u16, serde_json::Value)>>,
    ) -> Result<(u16, serde_json::Value)> {
        let method = safe_method(method);
        let context = self.context(
            &format!("HTTP {method}"),
            SpanKind::Client,
            vec![
                KeyValue::new("http.request.method", method),
                KeyValue::new("peer.service", peer.name()),
            ],
            &Context::current(),
        );
        let result = future.with_context(context.clone()).await;
        let status = match &result {
            Ok((status, _)) => {
                context.span().set_attribute(KeyValue::new(
                    "http.response.status_code",
                    i64::from(*status),
                ));
                status.to_string()
            }
            Err(_) => "error".into(),
        };
        if result.as_ref().map_or(true, |(status, _)| *status >= 400) {
            context.span().set_status(Status::error(""));
            context
                .span()
                .set_attribute(KeyValue::new("error.type", "infrastructure_request_failed"));
        }
        self.metrics
            .upstream
            .with_label_values(&[peer.name(), method, &status])
            .inc();
        context.span().end();
        result
    }
}
fn safe_method(method: &str) -> &'static str {
    match method {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "OPTIONS" => "OPTIONS",
        "CONNECT" => "CONNECT",
        "TRACE" => "TRACE",
        _ => "OTHER",
    }
}
struct Parent<'a>(&'a HeaderMap);
impl Extractor for Parent<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        if key == "traceparent" {
            self.0.get(key)?.to_str().ok()
        } else {
            None
        }
    }
    fn keys(&self) -> Vec<&str> {
        vec!["traceparent"]
    }
}
pub(crate) async fn http(
    Extract(telemetry): Extract<Telemetry>,
    request: Request,
    next: Next,
) -> Response {
    let method = safe_method(request.method().as_str());
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let parent = TraceContextPropagator::new()
        .extract_with_context(&Context::new(), &Parent(request.headers()));
    let context = telemetry.context(
        &format!("HTTP {method}"),
        SpanKind::Server,
        vec![
            KeyValue::new("http.request.method", method),
            KeyValue::new("http.route", route.clone()),
        ],
        &parent,
    );
    let start = Instant::now();
    let response = next.run(request).with_context(context.clone()).await;
    let status = response.status();
    context.span().set_attribute(KeyValue::new(
        "http.response.status_code",
        i64::from(status.as_u16()),
    ));
    if status.is_server_error() {
        context.span().set_status(Status::error(""));
    }
    context.span().end();
    telemetry
        .metrics
        .requests
        .with_label_values(&[method, &route, status.as_str()])
        .inc();
    telemetry
        .metrics
        .duration
        .with_label_values(&[method, &route])
        .observe(start.elapsed().as_secs_f64());
    response
}
pub fn metrics_router(state: State) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .with_state(state)
}
async fn metrics(Extract(state): Extract<State>) -> Response {
    let result:Result<String>=async {
        let (devices,ci,services):(i64,i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(eligible_ci),0),COALESCE(SUM(eligible_services),0) FROM devices WHERE revoked=0").fetch_one(&state.db).await?;
        let metrics=&state.telemetry.metrics;
        metrics.devices.set(devices);metrics.ci.set(ci);metrics.services.set(services);
        let mut bytes=Vec::new();TextEncoder::new().encode(&metrics.registry.gather(),&mut bytes)?;
        Ok(String::from_utf8(bytes)?)
    }.await;
    match result {
        Ok(body) => (
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Controller metrics unavailable",
        )
            .into_response(),
    }
}
