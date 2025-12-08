
/// Recursive JSON flattener
fn flatten_json_value(value: &serde_json::Value, acc: &mut HashMap<String, String>, prefix: &str) {
    match value {
        serde_json::Value::Null => {}
        serde_json::Value::Bool(b) => {
            acc.insert(prefix.to_string(), b.to_string());
        }
        serde_json::Value::Number(n) => {
            acc.insert(prefix.to_string(), n.to_string());
        }
        serde_json::Value::String(s) => {
            acc.insert(prefix.to_string(), s.clone());
        }
        serde_json::Value::Array(_) => {
            // Skip arrays for simple variable resolution for now
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let new_prefix = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };
                Self::flatten_json_value(v, acc, &new_prefix);
            }
        }
    }
}
