//! Client-independent extraction, persistence and bounded result description.
use crate::{
    db::{Dataset, Store},
    policy::{object_rows, THRESHOLD},
};
use anyhow::Result;
use serde_json::{json, Value};

/// Format a byte count as a human-readable size string matching the spec (e.g. "9.2 MB").
fn format_bytes(bytes: i64) -> String {
    const MB: i64 = 1024 * 1024;
    const KB: i64 = 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

pub(crate) fn spill(
    store: &Store,
    tool: &str,
    call_id: Option<&str>,
    raw: &str,
    payload: &Value,
) -> Result<Option<Value>> {
    if payload.get("isError").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let inner_len = payload
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|b| b.get("text"))
        .and_then(Value::as_str)
        .map(str::len)
        .unwrap_or(0);
    if raw.len() < THRESHOLD && inner_len < THRESHOLD {
        return Ok(None);
    }
    let Some(data) = extract(payload) else {
        return Ok(None);
    };
    let Some(rows) = object_rows(&data) else {
        return Ok(None);
    };
    let dataset = store.materialize(tool, call_id, rows, raw)?;
    Ok(Some(
        json!({"content": [{"type": "text", "text": descriptor(&dataset, &data)}], "isError": false}),
    ))
}

/// Accept only unambiguous result shapes. Multiple blocks, binary blocks, or
/// extra structured fields stay untouched rather than being silently discarded.
pub(crate) fn extract(payload: &Value) -> Option<Value> {
    if object_rows(payload).is_some() {
        return Some(payload.clone());
    }
    let content = payload.get("content").and_then(Value::as_array);
    let text_data = content
        .filter(|c| c.len() == 1)
        .and_then(|c| {
            (c[0].get("type")?.as_str()? == "text").then_some(())?;
            serde_json::from_str::<Value>(c[0].get("text")?.as_str()?).ok()
        })
        .filter(|v| object_rows(v).is_some());
    if let Some(structured) = payload.get("structuredContent") {
        object_rows(structured)?;
        // Only accept when content is absent, empty, or a single text block.
        // Any other block type (image, audio, resource) means the full response
        // carries non-text content that must not be silently replaced.
        let content_ok = match content {
            None => true,
            Some(c) if c.is_empty() => true,
            Some(c) if c.len() == 1 && c[0].get("type").and_then(Value::as_str) == Some("text") => {
                // If the text parses as a conflicting row array, reject.
                if let Some(text) = &text_data {
                    if text != structured {
                        return None;
                    }
                }
                true
            }
            _ => false,
        };
        if !content_ok {
            return None;
        }
        return Some(structured.clone());
    }
    text_data
}

pub(crate) fn descriptor(dataset: &Dataset, data: &Value) -> String {
    let mut result = format!(
        "Large MCP result spilled to local dataset.\n\nDataset: {}\nRows: {}\nOriginal size: {}\n\nColumns:\n",
        dataset.dataset,
        dataset.rows,
        format_bytes(dataset.original_bytes)
    );
    for column in &dataset.columns {
        let type_note = if column.mixed {
            format!(
                "{} (mixed numeric types; use CAST for predicates)",
                column.data_type
            )
        } else {
            column.data_type.clone()
        };
        let line = format!(
            "{} {}\n",
            serde_json::to_string(&column.name).unwrap(),
            type_note
        );
        if result.len() + line.len() > 4096 {
            result.push_str("Additional columns omitted. Use Spill describe for schema details.\n");
            break;
        }
        result.push_str(&line);
    }
    result.push_str("\nUse the Spill MCP `query` tool to inspect this dataset. Nested and mixed values are JSON-encoded VARCHAR.\n");
    let mut preview = Vec::new();
    for row in data.as_array().unwrap().iter().take(3) {
        let encoded = row.to_string();
        if encoded.len() > 512
            || preview.iter().map(String::len).sum::<usize>() + encoded.len() > 1024
        {
            break;
        }
        preview.push(encoded);
    }
    if !preview.is_empty() {
        result.push_str(&format!("\nPreview:\n{}", preview.join("\n")));
    }
    result
}
