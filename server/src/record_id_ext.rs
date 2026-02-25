use std::fmt;
use surrealdb::types::{RecordId, RecordIdKey};

/// Wrapper around RecordId that implements Display
///
/// Use via the `RecordIdExt::display()` method or `DisplayRecordId(&id)` directly.
pub struct DisplayRecordId<'a>(pub &'a RecordId);

impl fmt::Display for DisplayRecordId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.0.table, format_key(&self.0.key))
    }
}

fn format_key(key: &RecordIdKey) -> String {
    match key {
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Uuid(u) => u.to_string(),
        _ => format!("{:?}", key),
    }
}

/// Extension trait for RecordId that provides Display-like functionality
pub trait RecordIdExt {
    /// Returns a Display-able wrapper for use in format strings
    fn display(&self) -> DisplayRecordId<'_>;

    /// Returns the string representation as "table:key"
    fn to_raw_string(&self) -> String;

    /// Returns just the key portion as a string
    fn key_string(&self) -> String;
}

impl RecordIdExt for RecordId {
    fn display(&self) -> DisplayRecordId<'_> {
        DisplayRecordId(self)
    }

    fn to_raw_string(&self) -> String {
        format!("{}:{}", self.table, format_key(&self.key))
    }

    fn key_string(&self) -> String {
        format_key(&self.key)
    }
}
