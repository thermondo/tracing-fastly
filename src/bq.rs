use serde::{Serialize, Serializer, ser::Error as _};
use std::time::Duration;
use tracing_subscriber::fmt::MakeWriter;

pub fn write_ndjson_row<W, T>(writer: &W, row: &T)
where
    W: for<'a> MakeWriter<'a>,
    T: Serialize,
{
    let _ = serde_json::to_writer(writer.make_writer(), row);
}

pub fn ser_unix_seconds<S: Serializer>(t: &std::time::SystemTime, s: S) -> Result<S::Ok, S::Error> {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    s.serialize_f64(secs)
}

pub fn ser_duration_ms<S: Serializer>(duration: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(duration.as_secs_f64() * 1000.0)
}

pub fn ser_http_status<S: Serializer>(
    status: &fastly::http::StatusCode,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_u16(status.as_u16())
}

pub fn ser_json_as_string<T, S>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    match v {
        Some(val) => {
            let encoded = serde_json::to_string(val).map_err(S::Error::custom)?;
            s.serialize_str(&encoded)
        }
        None => s.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use serde_json::json;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn unix_seconds_is_a_number() {
        #[derive(Serialize)]
        struct Row {
            #[serde(serialize_with = "ser_unix_seconds")]
            t: SystemTime,
        }
        let v = serde_json::to_value(Row {
            t: UNIX_EPOCH + Duration::from_millis(1500),
        })
        .unwrap();
        assert!(v["t"].is_number());
        assert_eq!(v["t"].as_f64().unwrap(), 1.5);
    }

    #[test]
    fn pre_epoch_falls_back_to_zero() {
        #[derive(Serialize)]
        struct Row {
            #[serde(serialize_with = "ser_unix_seconds")]
            t: SystemTime,
        }
        let v = serde_json::to_value(Row {
            t: UNIX_EPOCH - Duration::from_secs(10),
        })
        .unwrap();
        assert_eq!(v["t"].as_f64().unwrap(), 0.0);
    }

    #[test]
    fn duration_serializes_as_millis() {
        #[derive(Serialize)]
        struct Row {
            #[serde(serialize_with = "ser_duration_ms")]
            d: Duration,
        }
        let v = serde_json::to_value(Row {
            d: Duration::from_millis(12),
        })
        .unwrap();
        assert_eq!(v["d"].as_f64().unwrap(), 12.0);
    }

    #[test]
    fn status_serializes_as_u16() {
        #[derive(Serialize)]
        struct Row {
            #[serde(serialize_with = "ser_http_status")]
            s: fastly::http::StatusCode,
        }
        let v = serde_json::to_value(Row {
            s: fastly::http::StatusCode::NOT_FOUND,
        })
        .unwrap();
        assert_eq!(v["s"], json!(404));
        assert!(v["s"].is_number());
    }

    #[test]
    fn json_payload_is_a_string_not_an_object() {
        #[derive(Serialize)]
        struct Row<'a> {
            #[serde(serialize_with = "ser_json_as_string")]
            p: Option<&'a serde_json::Value>,
        }
        let payload = json!({ "k": 1 });
        let v = serde_json::to_value(Row { p: Some(&payload) }).unwrap();

        let s = v["p"].as_str().expect("payload must be a string");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(s).unwrap(),
            json!({ "k": 1 })
        );
    }

    #[test]
    fn json_payload_none_serializes_as_null() {
        #[derive(Serialize)]
        struct Row<'a> {
            #[serde(serialize_with = "ser_json_as_string")]
            p: Option<&'a serde_json::Value>,
        }

        let v = serde_json::to_value(Row { p: None }).unwrap();
        assert!(v["p"].is_null());
    }
}
