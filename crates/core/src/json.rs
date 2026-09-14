use serde_json::Value;

/// Also a numeric string: Jellyfin's own clients send some tick fields quoted.
pub(crate) fn coerce_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

pub(crate) fn coerce_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| v.as_f64().map(|f| f as u64))
}

pub fn coerce_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_u64().map(|n| n as f64))
}

/// A list field, which arrives as a bare string when it holds a single id.
pub(crate) fn string_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(arr) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "json_test.rs"]
mod tests;
