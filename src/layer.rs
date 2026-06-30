use crate::event::{CorrelationFields, EventSink, EventVisitor, StructuredEvent};
use std::time::SystemTime;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{
    layer::{Context, Layer},
    registry::LookupSpan,
};

pub struct CorrelationLayer<K> {
    sink: K,
    correlate: Vec<&'static str>,
}

impl<K> CorrelationLayer<K> {
    pub fn new(sink: K) -> Self {
        Self {
            sink,
            correlate: Vec::new(),
        }
    }

    pub fn correlate(mut self, name: &'static str) -> Self {
        self.correlate.push(name);
        self
    }

    pub fn correlate_all(mut self, names: impl IntoIterator<Item = &'static str>) -> Self {
        self.correlate.extend(names);
        self
    }
}

#[derive(Default)]
struct SpanState {
    values: Vec<(&'static str, String)>,
}

struct CorrelationVisitor<'a> {
    wanted: &'a [&'static str],
    out: &'a mut Vec<(&'static str, String)>,
}

impl CorrelationVisitor<'_> {
    fn capture(&mut self, name: &str, value: impl FnOnce() -> String) {
        let Some(&key) = self.wanted.iter().find(|w| **w == name) else {
            return;
        };
        let value = value();

        match self.out.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.out.push((key, value)),
        }
    }
}

impl Visit for CorrelationVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.capture(field.name(), || value.to_owned());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.capture(field.name(), || {
            format!("{value:?}").trim_matches('"').to_owned()
        });
    }
}

impl<S, K> Layer<S> for CorrelationLayer<K>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    K: EventSink,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut state = SpanState::default();
        attrs.record(&mut CorrelationVisitor {
            wanted: &self.correlate,
            out: &mut state.values,
        });
        span.extensions_mut().insert(state);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut ext = span.extensions_mut();
        if let Some(state) = ext.get_mut::<SpanState>() {
            values.record(&mut CorrelationVisitor {
                wanted: &self.correlate,
                out: &mut state.values,
            });
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = EventVisitor::new();
        event.record(&mut visitor);

        let mut correlation = CorrelationFields::default();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(state) = span.extensions().get::<SpanState>() {
                    for (k, v) in &state.values {
                        if correlation.get(k).is_none() {
                            correlation.values.push((*k, v.clone()));
                        }
                    }
                }
            }
        }

        let structured = StructuredEvent {
            timestamp: SystemTime::now(),
            level: *event.metadata().level(),
            message: &visitor.message,
            fields: &visitor.fields,
            correlation: &correlation,
        };
        self.sink.emit(&structured);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::StructuredEvent;
    use std::sync::{Arc, Mutex};
    use tracing::{info_span, subscriber::with_default};
    use tracing_subscriber::{prelude::*, registry};

    #[derive(Default, Clone)]
    struct Captured {
        message: String,
        correlation: Vec<(String, String)>,
    }

    #[derive(Clone, Default)]
    struct CaptureSink(Arc<Mutex<Option<Captured>>>);

    impl EventSink for CaptureSink {
        fn emit(&self, event: &StructuredEvent<'_>) {
            *self.0.lock().unwrap() = Some(Captured {
                message: event.message.to_owned(),
                correlation: event
                    .correlation
                    .iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
            });
        }
    }

    fn capture<F: FnOnce()>(layer: CorrelationLayer<CaptureSink>, f: F) -> Captured {
        let sink = CaptureSink::default();
        let slot = sink.0.clone();
        let layer = CorrelationLayer { sink, ..layer };
        with_default(registry().with(layer), f);
        slot.lock().unwrap().take().unwrap_or_default()
    }

    fn rid(c: &Captured) -> Option<&str> {
        c.correlation
            .iter()
            .find(|(k, _)| k == "request_id")
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn request_id_captured_from_span_creation() {
        let c = capture(
            CorrelationLayer::new(CaptureSink::default()).correlate("request_id"),
            || {
                let span = info_span!("req", request_id = "abc-123");
                let _g = span.enter();
                tracing::info!("hello");
            },
        );
        assert_eq!(rid(&c), Some("abc-123"));
        assert_eq!(c.message, "hello");
    }

    #[test]
    fn request_id_captured_when_recorded_after_span_creation() {
        let c = capture(
            CorrelationLayer::new(CaptureSink::default()).correlate("request_id"),
            || {
                let span = info_span!("req", request_id = tracing::field::Empty);
                span.record("request_id", "late-id");
                let _g = span.enter();
                tracing::info!("hello");
            },
        );
        assert_eq!(rid(&c), Some("late-id"));
    }

    #[test]
    fn request_id_inherited_from_ancestor_span() {
        let c = capture(
            CorrelationLayer::new(CaptureSink::default()).correlate("request_id"),
            || {
                let outer = info_span!("req", request_id = "root-id");
                let _g = outer.enter();
                let inner = info_span!("inner");
                let _g2 = inner.enter();
                tracing::info!("hello");
            },
        );
        assert_eq!(rid(&c), Some("root-id"));
    }

    #[test]
    fn outermost_span_wins() {
        let c = capture(
            CorrelationLayer::new(CaptureSink::default()).correlate("request_id"),
            || {
                let outer = info_span!("req", request_id = "root-id");
                let _g = outer.enter();
                let inner = info_span!("inner", request_id = "inner-id");
                let _g2 = inner.enter();
                tracing::info!("hello");
            },
        );
        assert_eq!(rid(&c), Some("root-id"));
    }

    #[test]
    fn absent_when_no_ancestor_has_it() {
        let c = capture(
            CorrelationLayer::new(CaptureSink::default()).correlate("request_id"),
            || {
                let span = info_span!("plain");
                let _g = span.enter();
                tracing::info!("hello");
            },
        );
        assert_eq!(rid(&c), None);
    }

    #[test]
    fn only_configured_fields_are_correlated() {
        let c = capture(
            CorrelationLayer::new(CaptureSink::default()).correlate("request_id"),
            || {
                let span = info_span!("req", other = "nope", request_id = "real");
                let _g = span.enter();
                tracing::info!("hello");
            },
        );
        assert_eq!(rid(&c), Some("real"));

        assert!(c.correlation.iter().all(|(k, _)| k != "other"));
    }

    #[test]
    fn multiple_fields_correlated() {
        let c = capture(
            CorrelationLayer::new(CaptureSink::default()).correlate_all(["request_id", "trace_id"]),
            || {
                let span = info_span!("req", request_id = "r1", trace_id = "t1");
                let _g = span.enter();
                tracing::info!("hello");
            },
        );
        assert_eq!(rid(&c), Some("r1"));
        assert_eq!(
            c.correlation
                .iter()
                .find(|(k, _)| k == "trace_id")
                .map(|(_, v)| v.as_str()),
            Some("t1")
        );
    }
}
