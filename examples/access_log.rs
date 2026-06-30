use fastly::{
    Request, Response,
    http::{
        self, HeaderName, StatusCode, Url,
        header::{REFERER, USER_AGENT},
    },
    log::Endpoint,
};
use serde::Serialize;
use serde_with::{DisplayFromStr, serde_as, skip_serializing_none};
use std::{
    net::{self, IpAddr},
    time::{Duration, Instant, SystemTime},
};
use tracing::warn;
use tracing_fastly::bq;
use tracing_subscriber::prelude::*;

const FASTLY_CLIENT_IP: HeaderName = HeaderName::from_static("fastly-client-ip");
const X_CACHE: HeaderName = HeaderName::from_static("x-cache");

#[skip_serializing_none]
#[serde_as]
#[derive(Serialize)]
struct AccessLog<'a> {
    #[serde(serialize_with = "bq::ser_unix_seconds")]
    timestamp: SystemTime,
    service_name: &'a str,
    request_id: Option<&'a str>,
    host: Option<&'a str>,
    #[serde_as(as = "DisplayFromStr")]
    method: &'a http::Method,
    #[serde_as(as = "DisplayFromStr")]
    url: &'a Url,
    #[serde(serialize_with = "bq::ser_http_status")]
    status: StatusCode,
    #[serde(serialize_with = "bq::ser_duration_ms", rename = "response_time_ms")]
    response_time: Duration,
    bytes_written: Option<usize>,
    cache_status: Option<&'a str>,
    client_ip: Option<&'a IpAddr>,
    client_country: Option<&'a str>,
    user_agent: Option<&'a str>,
    referer: Option<&'a str>,
    tls_protocol: Option<&'a str>,
    backend_name: Option<&'a str>,
    fastly_pop: Option<&'a str>,
    fastly_server: Option<&'a str>,
}

struct RequestCapture {
    started_at: Instant,
    method: http::Method,
    url: Url,
    client_ip: Option<IpAddr>,
    user_agent: Option<String>,
    referer: Option<String>,
    tls_protocol: Option<String>,
}

impl RequestCapture {
    fn host(&self) -> Option<&str> {
        self.url.host_str()
    }

    fn client_country(&self) -> Option<String> {
        self.client_ip
            .and_then(fastly::geo::geo_lookup)
            .map(|geo| geo.country_code().to_owned())
    }
}

impl From<&Request> for RequestCapture {
    fn from(req: &Request) -> Self {
        let client_ip = req
            .get_header_str_lossy(FASTLY_CLIENT_IP)
            .and_then(|s| match s.parse::<net::IpAddr>() {
                Ok(ip) => Some(ip),
                Err(err) => {
                    warn!(?err, value = s.as_ref(), "error parsing fastly-client-ip");
                    None
                }
            });

        Self {
            started_at: Instant::now(),
            method: req.get_method().to_owned(),
            url: req.get_url().to_owned(),
            client_ip,
            user_agent: req.get_header_str_lossy(USER_AGENT).map(|s| s.to_string()),
            referer: req.get_header_str_lossy(REFERER).map(|s| s.to_string()),
            tls_protocol: req.get_tls_protocol().ok().flatten().map(|s| s.to_owned()),
        }
    }
}

fn emit_access_log(
    capture: &RequestCapture,
    request_id: &str,
    response: &Response,
    backend_name: Option<&str>,
    service_name: &str,
) {
    let response_time = capture.started_at.elapsed();

    println!(
        "access status={} method={} url={} response_time_ms={} backend={} request_id={request_id}",
        response.get_status().as_u16(),
        capture.method,
        capture.url,
        response_time.as_millis(),
        backend_name.unwrap_or("-"),
    );

    let endpoint = Endpoint::from_name("bq_access_log");

    let client_country = capture.client_country();
    let cache_status = response.get_header_str_lossy(X_CACHE);

    let row = AccessLog {
        timestamp: SystemTime::now(),
        service_name,
        request_id: Some(request_id),
        host: capture.host(),
        method: &capture.method,
        url: &capture.url,
        status: response.get_status(),
        response_time,
        bytes_written: response.get_content_length(),
        cache_status: cache_status.as_deref(),
        client_ip: capture.client_ip.as_ref(),
        client_country: client_country.as_deref(),
        user_agent: capture.user_agent.as_deref(),
        referer: capture.referer.as_deref(),
        tls_protocol: capture.tls_protocol.as_deref(),
        backend_name,
        fastly_pop: non_empty(fastly::compute_runtime::pop()),
        fastly_server: non_empty(fastly::compute_runtime::hostname()),
    };
    bq::write_ndjson_row(&|| endpoint.clone(), &row);
}

fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().with_ansi(false))
        .init();

    let req = Request::new(http::Method::GET, "https://example.thermondo.de/")
        .with_header(FASTLY_CLIENT_IP, "203.0.113.5");
    let capture = RequestCapture::from(&req);
    let response = Response::from_status(StatusCode::OK);
    emit_access_log(
        &capture,
        "req-abc-123",
        &response,
        Some("origin"),
        "example_service",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::net::Ipv4Addr;

    #[test]
    fn access_log_column_set_matches_schema() {
        let row = AccessLog {
            timestamp: SystemTime::now(),
            service_name: "svc",
            request_id: Some("rid-1"),
            host: Some("example.thermondo.de"),
            method: &http::Method::GET,
            url: &Url::parse("https://example.thermondo.de/").unwrap(),
            status: StatusCode::OK,
            response_time: Duration::from_millis(12),
            bytes_written: Some(1024),
            cache_status: Some("PASS"),
            client_ip: Some(&IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))),
            client_country: Some("DE"),
            user_agent: Some("curl/8"),
            referer: Some("https://ref"),
            tls_protocol: Some("TLSv1.3"),
            backend_name: Some("my-backend"),
            fastly_pop: Some("FRA"),
            fastly_server: Some("cache-fra19151"),
        };

        let v = serde_json::to_value(&row).unwrap();
        assert!(v["timestamp"].is_number());
        assert!(v["status"].is_number());
        assert!(v["response_time_ms"].is_number());

        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "backend_name",
                "bytes_written",
                "cache_status",
                "client_country",
                "client_ip",
                "fastly_pop",
                "fastly_server",
                "host",
                "method",
                "referer",
                "request_id",
                "response_time_ms",
                "service_name",
                "status",
                "timestamp",
                "tls_protocol",
                "url",
                "user_agent",
            ]
        );
    }
}
