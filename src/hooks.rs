//! Dispatch to the installed client's hook adapter.
use crate::{client::Client, db::Store};
use serde_json::{json, Value};

pub fn hook(client: Client, input: &str, store: &Store) -> Value {
    if client == Client::Cursor {
        return crate::cursor::hook(input, store);
    }
    let result = match client {
        Client::Codex => crate::codex::process(input, store),
        Client::Claude => crate::claude::process(input, store),
        Client::Cursor => unreachable!(),
    };
    match result {
        Ok(Some(output)) => output,
        Ok(None) => json!({}),
        Err(_) => {
            eprintln!("spill: materialization failed; preserving original MCP output");
            json!({})
        }
    }
}
