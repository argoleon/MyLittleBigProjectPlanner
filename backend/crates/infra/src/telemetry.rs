//! Logs, traces and metrics of every service (specification §19).
//!
//! - **Logs**: `tracing` events on standard output, one JSON object per line
//!   by default (`LOG_FORMAT=text` for development). A request's events carry
//!   the fields of its span, including `request_id`, the correlation ID also
//!   returned in the `x-request-id` header.
//! - **Traces and metrics**: OpenTelemetry, exported in OTLP over HTTP to the
//!   collector named by `OTEL_EXPORTER_OTLP_ENDPOINT` (Alloy, in the
//!   `observability` profile), only when it is set.
//!
//! [`init`] runs first in `main`, before the Tokio runtime starts: the OTLP
//! exporters use a blocking HTTP client in their own thread. [`Telemetry::
//! shutdown`] runs last, once the runtime has stopped, to flush what is
//! pending.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use sqlx::postgres::PgPool;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::{LogFormat, TelemetryConfig};

/// Why telemetry could not start.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("cannot build the OTLP exporter: {0}")]
    Exporter(String),
    #[error("a global logger is already installed: {0}")]
    AlreadyInstalled(String),
}

/// The running exporters; flush them with [`Telemetry::shutdown`].
#[derive(Debug)]
pub struct Telemetry {
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
}

impl Telemetry {
    /// Whether traces and metrics are exported.
    pub fn exports(&self) -> bool {
        self.tracer.is_some()
    }

    /// Sends what is still buffered. Call it outside of any Tokio runtime.
    pub fn shutdown(self) {
        // The logger may already be gone: report on standard error.
        if let Some(tracer) = self.tracer
            && let Err(err) = tracer.shutdown()
        {
            eprintln!("telemetry: cannot flush the traces: {err}");
        }
        if let Some(meter) = self.meter
            && let Err(err) = meter.shutdown()
        {
            eprintln!("telemetry: cannot flush the metrics: {err}");
        }
    }
}

/// The log layer of the services, writing to `writer` in `format`. Exposed
/// so that tests capture exactly what production writes.
pub fn log_layer<S, W>(format: LogFormat, writer: W) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    match format {
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_writer(writer)
            .boxed(),
        LogFormat::Text => tracing_subscriber::fmt::layer().with_writer(writer).boxed(),
    }
}

/// Installs logging, and tracing and metrics export when configured, for
/// `service` (the `service.name` of its traces and metrics).
pub fn init(service: &'static str, config: &TelemetryConfig) -> Result<Telemetry, TelemetryError> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let (tracer, meter) = match &config.otlp_endpoint {
        Some(endpoint) => {
            let (tracer, meter) = exporters(service, endpoint)?;
            (Some(tracer), Some(meter))
        }
        None => (None, None),
    };
    let otel_layer = tracer
        .as_ref()
        .map(|provider| tracing_opentelemetry::layer().with_tracer(provider.tracer(service)));

    tracing_subscriber::registry()
        .with(filter)
        .with(log_layer(config.log_format, std::io::stdout))
        .with(otel_layer)
        .try_init()
        .map_err(|e| TelemetryError::AlreadyInstalled(e.to_string()))?;

    // Only once installed: a refused second `init` must not replace the
    // providers of the first one.
    if let (Some(tracer), Some(meter)) = (&tracer, &meter) {
        // W3C trace context: a request's trace continues the caller's.
        global::set_text_map_propagator(TraceContextPropagator::new());
        global::set_tracer_provider(tracer.clone());
        global::set_meter_provider(meter.clone());
    }
    if let Some(endpoint) = &config.otlp_endpoint {
        tracing::info!(service, endpoint, "exporting traces and metrics");
    }
    Ok(Telemetry { tracer, meter })
}

fn exporters(
    service: &'static str,
    endpoint: &str,
) -> Result<(SdkTracerProvider, SdkMeterProvider), TelemetryError> {
    let resource = Resource::builder()
        .with_service_name(service)
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build();

    let spans = SpanExporter::builder()
        .with_http()
        .with_endpoint(format!("{endpoint}/v1/traces"))
        .build()
        .map_err(|e| TelemetryError::Exporter(e.to_string()))?;
    let tracer = SdkTracerProvider::builder()
        .with_batch_exporter(spans)
        .with_resource(resource.clone())
        .build();

    let metrics = MetricExporter::builder()
        .with_http()
        .with_endpoint(format!("{endpoint}/v1/metrics"))
        .build()
        .map_err(|e| TelemetryError::Exporter(e.to_string()))?;
    let meter = SdkMeterProvider::builder()
        .with_periodic_exporter(metrics)
        .with_resource(resource)
        .build();
    Ok((tracer, meter))
}

/// Reports the connections of `pool` as the `db.client.connection.count`
/// metric (OpenTelemetry semantic conventions), by state: used or idle.
pub fn observe_pool(pool: &PgPool) {
    let pool = pool.clone();
    global::meter("mlbpp")
        .i64_observable_up_down_counter("db.client.connection.count")
        .with_description("Connections of the PostgreSQL pool, by state")
        .with_unit("{connection}")
        .with_callback(move |observer| {
            let idle = i64::try_from(pool.num_idle()).unwrap_or(i64::MAX);
            let size = i64::from(pool.size());
            let name = KeyValue::new("db.client.connection.pool.name", "mlbpp");
            observer.observe(
                size - idle,
                &[name.clone(), KeyValue::new("db.client.connection.state", "used")],
            );
            observer.observe(idle, &[name, KeyValue::new("db.client.connection.state", "idle")]);
        })
        .build();
}
