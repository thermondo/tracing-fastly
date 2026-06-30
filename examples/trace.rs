use fastly::log::Endpoint;
use serde::Serialize;
use serde_with::{DisplayFromStr, serde_as, skip_serializing_none};
use std::{sync::Mutex, time::SystemTime};
use tracing_fastly::{CorrelationLayer, EventSink, StructuredEvent, bq};
use tracing_subscriber::prelude::*;

fn setup_logging(service_name: &str) {
    let bq_layer = Endpoint::try_from_name("bq_trace_logs")
        .ok()
        .map(|endpoint| {
            CorrelationLayer::new(BqTraceSink {
                service_name: service_name.to_owned(),
                endpoint: Mutex::new(endpoint),
            })
            .correlate("request_id")
        });

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().with_ansi(false))
        .with(bq_layer)
        .init();
}

struct BqTraceSink {
    service_name: String,
    endpoint: Mutex<Endpoint>,
}

impl EventSink for BqTraceSink {
    fn emit(&self, event: &StructuredEvent<'_>) {
        let payload = event
            .payload()
            .map(|fields| serde_json::Value::Object(fields.clone()));

        let row = TraceLog {
            timestamp: event.timestamp,
            service_name: &self.service_name,
            request_id: event.correlation.get("request_id"),
            level: event.level,
            message: event.message,
            payload: payload.as_ref(),
        };
        bq::write_ndjson_row(&self.endpoint, &row);
    }
}

#[skip_serializing_none]
#[serde_as]
#[derive(Serialize)]
struct TraceLog<'a> {
    #[serde(serialize_with = "bq::ser_unix_seconds")]
    timestamp: SystemTime,
    service_name: &'a str,
    request_id: Option<&'a str>,
    #[serde_as(as = "DisplayFromStr")]
    level: tracing::Level,
    message: &'a str,

    #[serde(serialize_with = "bq::ser_json_as_string")]
    payload: Option<&'a serde_json::Value>,
}

fn main() {
    setup_logging("example_service");

    let span = tracing::info_span!("request", request_id = tracing::field::Empty);
    span.record("request_id", "req-abc-123");
    let _guard = span.enter();

    tracing::info!(status = 200, backend = "origin", "handled request");
    tracing::warn!(reason = "stale", "cache miss");
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn trace_log_columns_and_payload_is_a_string() {
        let payload = json!({ "k": 1 });
        let row = TraceLog {
            timestamp: SystemTime::UNIX_EPOCH + Duration::from_millis(1500),
            service_name: "svc",
            request_id: Some("rid"),
            level: tracing::Level::INFO,
            message: "hi",
            payload: Some(&payload),
        };

        let v = serde_json::to_value(&row).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "level",
                "message",
                "payload",
                "request_id",
                "service_name",
                "timestamp"
            ]
        );

        let s = v["payload"]
            .as_str()
            .expect("payload must be a JSON string");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(s).unwrap(),
            json!({ "k": 1 })
        );
    }

    #[test]
    fn trace_log_omits_none_payload_and_request_id() {
        let row = TraceLog {
            timestamp: SystemTime::UNIX_EPOCH,
            service_name: "svc",
            request_id: None,
            level: tracing::Level::INFO,
            message: "",
            payload: None,
        };
        let v = serde_json::to_value(&row).unwrap();
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("request_id"));
        assert!(!obj.contains_key("payload"));
    }
}
