//! Codex `PostToolUse` adapter.
//!
//! Codex exposes completed MCP output as `tool_response` and replaces the
//! model-visible result when a hook returns `continue: false` with feedback.
use crate::{db::Store, result};
use anyhow::Result;
use serde_json::{json, Value};

/// Parse and handle one Codex PostToolUse event. Unsupported results pass through.
pub fn process(input: &str, store: &Store) -> Result<Option<Value>> {
    let event: Value = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if event.get("hook_event_name").and_then(Value::as_str) != Some("PostToolUse") {
        return Ok(None);
    }
    let Some(tool) = event.get("tool_name").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some((server, name)) = tool.strip_prefix("mcp__").and_then(|s| s.split_once("__")) else {
        return Ok(None);
    };
    if server.is_empty() || name.is_empty() || server == "spill" {
        return Ok(None);
    }
    let Some(response) = event.get("tool_response") else {
        return Ok(None);
    };
    // Measure the original bytes before re-serialization can shrink whitespace.
    let raw_string;
    let (raw, decoded_from_str);
    let payload = if let Some(text) = response.as_str() {
        if text.len() < crate::policy::THRESHOLD {
            return Ok(None);
        }
        raw = text;
        decoded_from_str = match serde_json::from_str::<Value>(text) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        &decoded_from_str
    } else {
        let inner_text_len = response
            .get("content")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|b| b.get("text"))
            .and_then(Value::as_str)
            .map(str::len)
            .unwrap_or(0);
        raw_string = serde_json::to_string(response)?;
        // Mirror result::spill's AND logic: skip only when *both* the envelope
        // and the inner text block are below the threshold. A response with a
        // large structuredContent but a short summary text block must be spilled.
        if raw_string.len() < crate::policy::THRESHOLD && inner_text_len < crate::policy::THRESHOLD
        {
            return Ok(None);
        }
        raw = raw_string.as_str();
        response
    };
    let Some(replacement) = result::spill(
        store,
        name,
        event.get("tool_use_id").and_then(Value::as_str),
        raw,
        payload,
    )?
    else {
        return Ok(None);
    };
    let stop_reason = replacement["content"][0]["text"]
        .as_str()
        .unwrap_or("Spill materialization error")
        .to_string();
    Ok(Some(json!({
        "continue": false,
        "stopReason": stop_reason
    })))
}
