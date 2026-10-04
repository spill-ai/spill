//! Cursor's documented postToolUse contract is isolated here.
//! https://cursor.com/docs/hooks#posttooluse
use crate::{db::Store, policy::THRESHOLD};
use anyhow::Result;
use serde_json::{json, Value};

/// Empty output is Cursor's pass-through response, including on storage errors.
pub fn hook(input: &str, store: &Store) -> Value {
    match process(input, store) {
        Ok(Some(output)) => output,
        Ok(None) => json!({}),
        Err(_) => {
            // Database errors can include arbitrarily large source values.
            eprintln!("spill: materialization failed; preserving original MCP output");
            json!({})
        }
    }
}

fn process(input: &str, store: &Store) -> Result<Option<Value>> {
    let event: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if event
        .get("hook_event_name")
        .and_then(Value::as_str)
        .is_some_and(|n| n != "postToolUse")
    {
        return Ok(None);
    }
    let Some(tool) = event.get("tool_name").and_then(Value::as_str) else {
        return Ok(None);
    };
    // Cursor identifies MCP tools with the documented MCP:<tool_name> form.
    if !tool.starts_with("MCP:") {
        return Ok(None);
    }
    let stem = tool.strip_prefix("MCP:").unwrap();
    if stem.starts_with("spill__") || stem.starts_with("spill_") || stem == "spill" {
        return Ok(None);
    }
    let Some(raw) = event.get("tool_output").and_then(Value::as_str) else {
        return Ok(None);
    };
    if raw.len() < THRESHOLD {
        return Ok(None);
    }
    let payload: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    Ok(crate::result::spill(
        store,
        stem,
        event.get("tool_use_id").and_then(Value::as_str),
        raw,
        &payload,
    )?
    .map(|replacement| json!({"updated_mcp_tool_output": replacement})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::extract;
    #[test]
    fn pass_through_and_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("missing/db"));
        for input in [
            "not json".to_string(),
            json!({"tool_name":"MCP:list", "tool_output":"[]"}).to_string(),
            json!({"tool_name":"Shell", "tool_output": json!([{"s":"x".repeat(100000)}]).to_string()}).to_string(),
            json!({"tool_name":"MCP:spill__query", "tool_output": json!([{"s":"x".repeat(100000)}]).to_string()}).to_string(),
        ] {
            assert_eq!(hook(&input, &store), json!({}));
        }
        assert!(!store.path().exists());
        let store = Store::new(tmp.path()); // A directory cannot be opened as DuckDB.
        assert_eq!(hook(&json!({"tool_name":"MCP:list", "tool_output":json!([{"s":"x".repeat(100000)}]).to_string()}).to_string(), &store), json!({}));
    }
    #[test]
    fn supported_envelopes() {
        let rows = json!([{"a":1}]);
        assert_eq!(
            extract(&json!({"content":[{"type":"text", "text": rows.to_string()}]})),
            Some(rows.clone())
        );
        assert_eq!(
            extract(&json!({"structuredContent": rows})),
            Some(rows.clone())
        );
        assert_eq!(
            extract(
                &json!({"structuredContent":rows,"content":[{"type":"text","text":"Found 2000 issues"}]})
            ),
            Some(rows.clone())
        );
        let different_rows = json!([{"other": 2}]);
        assert!(extract(
            &json!({"structuredContent":rows,"content":[{"type":"text","text":different_rows.to_string()}]})
        )
        .is_none());
        assert!(extract(&json!({"content":[{"type":"text", "text":rows.to_string()}, {"type":"image","data":"abc"}]})).is_none());
        assert!(extract(&json!({"structuredContent":{"items":rows}})).is_none());
        // structuredContent with a non-text content block must not be accepted —
        // replacing the entire response would silently drop the image/audio/etc.
        assert!(extract(
            &json!({"structuredContent":rows,"content":[{"type":"image","data":"abc"}]})
        )
        .is_none());
        assert!(extract(&json!({"structuredContent":rows,"content":[{"type":"text","text":"ok"},{"type":"image","data":"abc"}]})).is_none());
    }
}
