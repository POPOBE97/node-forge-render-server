/// Utility formatters shared by timeline tooltip and interaction bridge.

/// Format an f64 to two decimal places.
/// NaN returns `"NaN"`, infinity returns `"Inf"`.
pub fn format_f64_2dp(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return "Inf".to_string();
    }
    format!("{:.2}", v)
}

/// Format a `serde_json::Value` for display with two-decimal numeric precision.
pub fn format_json_value_2dp(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Number(n) => format_f64_2dp(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .map(|v| {
                    v.as_f64()
                        .map(format_f64_2dp)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            format!("[{}]", parts.join(", "))
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
