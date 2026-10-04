use serde_json::Value;

pub const THRESHOLD: usize = 32 * 1024;

/// Deliberately does not search arbitrary nested objects for candidate arrays.
pub fn object_rows(value: &Value) -> Option<&[Value]> {
    let rows = value.as_array()?;
    (!rows.is_empty() && rows.iter().all(Value::is_object)).then_some(rows.as_slice())
}

pub fn eligible_json(raw: &str) -> Option<Value> {
    if raw.len() < THRESHOLD {
        return None;
    }
    let value: Value = serde_json::from_str(raw).ok()?;
    object_rows(&value)?;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spill_policy() {
        assert!(eligible_json(&json!([{ "s": "x".repeat(1024) }]).to_string()).is_none());
        assert!(eligible_json(&json!([{ "s": "x".repeat(100 * 1024) }]).to_string()).is_some());
        assert!(eligible_json(&"x".repeat(100 * 1024)).is_none());
        assert!(eligible_json(&json!(["x".repeat(100 * 1024)]).to_string()).is_none());
        assert!(eligible_json(&format!("[{{{}", "x".repeat(100 * 1024))).is_none());
        assert!(eligible_json(&json!({"s": "x".repeat(100 * 1024)}).to_string()).is_none());
        assert!(object_rows(&json!([{}, 1])).is_none());
        assert!(object_rows(&json!([])).is_none());
    }

    #[test]
    fn exact_boundary() {
        let base = json!([{"s": ""}]).to_string().len();
        assert!(eligible_json(&json!([{"s": "x".repeat(THRESHOLD-base)}]).to_string()).is_some());
        assert!(eligible_json(&json!([{"s": "x".repeat(THRESHOLD-base-1)}]).to_string()).is_none());
    }
}
