//! Integration tests: the OpenTelemetry metrics and traces of the API,
//! captured by in-memory exporters. One test only: it installs the global
//! meter provider and propagator of this test process.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use api::{AppState, HttpSettings};
use domain::tenancy::Tenancy;
use infra::sqlx::postgres::PgPoolOptions;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

#[tokio::test(flavor = "current_thread")]
async fn requests_produce_a_duration_metric_and_a_server_span_continuing_the_callers_trace() {
    // Metrics: global provider, read on demand.
    let metrics = InMemoryMetricExporter::default();
    let meter_provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(metrics.clone()).build())
        .build();
    opentelemetry::global::set_meter_provider(meter_provider.clone());
    // Traces: the tracing → OpenTelemetry layer on this thread.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let spans = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(spans.clone())
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("api")));
    let _guard = tracing::subscriber::set_default(subscriber);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    let state = AppState::new(
        pool,
        Tenancy::Single {
            default_slug: "default".to_owned(),
        },
    );
    let settings = HttpSettings {
        request_timeout: Duration::from_secs(10),
        body_limit_bytes: 1024,
    };
    // The app is built after the providers: its instruments use them.
    let app = api::app(state, &settings);
    tokio::spawn(api::serve(
        listener,
        app,
        std::future::pending(),
        Duration::from_secs(1),
    ));

    let client = reqwest::Client::new();
    let traced = client
        .get(format!("{base}/api/v1/ping"))
        .header("traceparent", format!("00-{TRACE_ID}-00f067aa0ba902b7-01"))
        .send()
        .await
        .unwrap();
    assert_eq!(traced.status(), 200);
    assert_eq!(
        client
            .get(format!("{base}/nowhere"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // Health probes are not measured.
    client.get(format!("{base}/health/live")).send().await.unwrap();

    // --- Metrics
    meter_provider.force_flush().unwrap();
    let exported = metrics.get_finished_metrics().unwrap();
    let histogram = exported
        .iter()
        .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
        .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
        .find(|metric| metric.name() == "http.server.request.duration")
        .expect("http.server.request.duration is exported");
    assert_eq!(histogram.unit(), "s");
    let AggregatedMetrics::F64(MetricData::Histogram(data)) = histogram.data() else {
        panic!("not a histogram: {:?}", histogram.data());
    };
    let mut series: Vec<(String, String, i64, u64)> = data
        .data_points()
        .map(|point| {
            let attribute = |key: &str| {
                point
                    .attributes()
                    .find(|kv| kv.key.as_str() == key)
                    .map(|kv| kv.value.clone())
                    .unwrap()
            };
            let status = match attribute("http.response.status_code") {
                opentelemetry::Value::I64(code) => code,
                other => panic!("status code is {other:?}"),
            };
            (
                attribute("http.request.method").to_string(),
                attribute("http.route").to_string(),
                status,
                point.count(),
            )
        })
        .collect();
    series.sort();
    assert_eq!(
        series,
        vec![
            ("GET".to_owned(), "/api/v1/ping".to_owned(), 200, 1),
            ("GET".to_owned(), "unmatched".to_owned(), 404, 1),
        ]
    );

    // --- Traces
    let finished = spans.get_finished_spans().unwrap();
    let ping = finished
        .iter()
        .find(|span| span.name == "GET /api/v1/ping")
        .unwrap_or_else(|| panic!("no server span: {finished:#?}"));
    assert_eq!(
        ping.span_context.trace_id().to_string(),
        TRACE_ID,
        "continues the caller's trace"
    );
    assert_eq!(ping.parent_span_id.to_string(), "00f067aa0ba902b7");
    assert_eq!(ping.span_kind, opentelemetry::trace::SpanKind::Server);
}
