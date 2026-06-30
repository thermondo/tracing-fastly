use fastly::log::Endpoint;
use std::io;
use tracing::{info, info_span, warn};
use tracing_subscriber::{
    EnvFilter, filter::LevelFilter, fmt, fmt::writer::MakeWriterExt, prelude::*,
};

fn setup_logging() {
    let writer = io::stdout.and(|| Endpoint::from_name("my_logs"));

    tracing_subscriber::registry()
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with(fmt::layer().compact().with_ansi(false).with_writer(writer))
        .init();
}

fn main() {
    setup_logging();

    let _guard = info_span!("request", request_id = "req-abc-123").entered();
    info!(status = 200, backend = "origin", "handled request");
    warn!(reason = "stale", "cache miss");
}
