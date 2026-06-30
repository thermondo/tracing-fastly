use serde_json::{Map, Value};
use std::time::SystemTime;
use tracing::{
    Level,
    field::{Field, Visit},
};

pub struct StructuredEvent<'a> {
    pub timestamp: SystemTime,
    pub level: Level,
    pub message: &'a str,
    pub fields: &'a Map<String, Value>,
    pub correlation: &'a CorrelationFields,
}

impl StructuredEvent<'_> {
    pub fn payload(&self) -> Option<&Map<String, Value>> {
        (!self.fields.is_empty()).then_some(self.fields)
    }
}

pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: &StructuredEvent<'_>);
}

#[derive(Debug, Default, Clone)]
pub struct CorrelationFields {
    pub(crate) values: Vec<(&'static str, String)>,
}

impl CorrelationFields {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        self.values.iter().map(|(k, v)| (*k, v.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

pub(crate) struct EventVisitor {
    pub(crate) message: String,
    pub(crate) fields: Map<String, Value>,
}

impl EventVisitor {
    pub(crate) fn new() -> Self {
        Self {
            message: String::new(),
            fields: Map::new(),
        }
    }

    fn insert(&mut self, name: &str, value: Value) {
        if name == "message" {
            self.message = match value {
                Value::String(s) => s,
                other => other.to_string(),
            };
        } else {
            self.fields.insert(name.to_owned(), value);
        }
    }
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field.name(), Value::String(value.to_owned()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field.name(), Value::Number(value.into()));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field.name(), Value::Number(value.into()));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field.name(), Value::Bool(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(num) = serde_json::Number::from_f64(value) {
            self.insert(field.name(), Value::Number(num));
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.insert(field.name(), Value::String(format!("{value:?}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routes_message_field_to_message() {
        let mut v = EventVisitor::new();
        v.insert("message", Value::String("hello".into()));

        assert_eq!(v.message, "hello");
        assert!(v.fields.is_empty());
    }

    #[test]
    fn routes_non_message_fields_to_payload() {
        let mut v = EventVisitor::new();
        v.insert("status", Value::Number(200.into()));
        v.insert("backend", Value::String("foo".into()));

        assert_eq!(v.message, "");
        assert_eq!(v.fields.len(), 2);
        assert_eq!(v.fields["status"], json!(200));
        assert_eq!(v.fields["backend"], json!("foo"));
    }

    #[test]
    fn message_last_write_wins() {
        let mut v = EventVisitor::new();
        v.insert("message", Value::String("first".into()));
        v.insert("message", Value::String("second".into()));

        assert_eq!(v.message, "second");
    }

    #[test]
    fn coerces_non_string_message_to_string() {
        let mut v = EventVisitor::new();
        v.insert("message", Value::Number(42.into()));

        assert_eq!(v.message, "42");
    }

    #[test]
    fn separates_message_from_other_fields() {
        let mut v = EventVisitor::new();
        v.insert("message", Value::String("the message".into()));
        v.insert("k", Value::Bool(true));

        assert_eq!(v.message, "the message");
        assert_eq!(v.fields.len(), 1);

        assert!(!v.fields.contains_key("message"));
    }

    #[test]
    fn payload_is_none_when_empty() {
        let fields = Map::new();
        let correlation = CorrelationFields::default();
        let ev = StructuredEvent {
            timestamp: SystemTime::UNIX_EPOCH,
            level: Level::INFO,
            message: "hi",
            fields: &fields,
            correlation: &correlation,
        };
        assert!(ev.payload().is_none());
    }

    #[test]
    fn correlation_fields_lookup() {
        let cf = CorrelationFields {
            values: vec![("request_id", "abc".into()), ("trace_id", "xyz".into())],
        };
        assert_eq!(cf.get("request_id"), Some("abc"));
        assert_eq!(cf.get("trace_id"), Some("xyz"));
        assert_eq!(cf.get("missing"), None);
        assert!(!cf.is_empty());
    }
}
