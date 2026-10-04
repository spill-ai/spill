//! Claude Code `PostToolUse` adapter.
//!
//! MCP tools expose their result as `tool_response`; Claude Code consumes
//! `hookSpecificOutput.updatedMCPToolOutput` to replace it before the model.
use crate::{db::Store, result};
use anyhow::Result;
use serde_json::{json, Value};

/// Parse and handle one Claude Code PostToolUse event. Unsupported results pass through.
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
    // Claude may supply the MCP content-block list directly instead of wrapping
    // it in a result object. Do not mistake image or other blocks for dataset rows.
    let content_blocks = payload
        .as_array()
        .is_some_and(|items| !items.is_empty() && items.iter().all(is_content_block));
    let normalized;
    let payload = if content_blocks {
        normalized = json!({"content": payload});
        &normalized
    } else {
        payload
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
    let replacement = if content_blocks {
        replacement["content"].clone()
    } else {
        replacement
    };
    Ok(Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedMCPToolOutput": replacement
        }
    })))
}

fn is_content_block(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("text" | "image" | "audio" | "resource" | "resource_link")
    ) && (value.get("text").is_some()
        || value.get("data").is_some()
        || value.get("resource").is_some()
        || value.get("uri").is_some())
}
