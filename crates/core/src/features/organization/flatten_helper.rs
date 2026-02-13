use std::collections::HashMap;

/// Recursive JSON flattener
pub fn flatten_json_value(
    value: &serde_json::Value,
    acc: &mut HashMap<String, String>,
    prefix: &str,
) {
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
                flatten_json_value(v, acc, &new_prefix);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flatten_simple_object() {
        let json = serde_json::json!({
            "title": "My Game",
            "price": 1980,
            "active": true,
        });
        let mut acc = HashMap::new();
        flatten_json_value(&json, &mut acc, "");

        assert_eq!(acc["title"], "My Game");
        assert_eq!(acc["price"], "1980");
        assert_eq!(acc["active"], "true");
    }

    #[test]
    fn test_flatten_nested_object() {
        let json = serde_json::json!({
            "common": {
                "title": "Nested Title",
                "creator": "Circle",
            }
        });
        let mut acc = HashMap::new();
        flatten_json_value(&json, &mut acc, "");

        assert_eq!(acc["common.title"], "Nested Title");
        assert_eq!(acc["common.creator"], "Circle");
    }

    #[test]
    fn test_flatten_with_prefix() {
        let json = serde_json::json!({"key": "value"});
        let mut acc = HashMap::new();
        flatten_json_value(&json, &mut acc, "dlsite");

        assert_eq!(acc["dlsite.key"], "value");
    }

    #[test]
    fn test_flatten_skips_null_and_arrays() {
        let json = serde_json::json!({
            "present": "yes",
            "missing": null,
            "list": [1, 2, 3],
        });
        let mut acc = HashMap::new();
        flatten_json_value(&json, &mut acc, "");

        assert_eq!(acc.len(), 1);
        assert_eq!(acc["present"], "yes");
    }
}
