use serde_json::Value;

/// Also a numeric string: Jellyfin's own clients send some tick fields quoted.
pub(crate) fn coerce_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

pub(crate) fn coerce_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64))
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
