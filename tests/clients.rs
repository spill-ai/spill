use serde_json::{json, Value};
use spill::{
    client::Client,
    db::Store,
    hooks,
    install::{install_client, uninstall_client, ConfigPaths},
};
use std::{fs, path::Path};

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn large_rows() -> Value {
    json!((0..2000)
        .map(|id| json!({"id":id,"state":"OPEN"}))
        .collect::<Vec<_>>())
}
fn event(response: Value) -> Value {
    json!({"hook_event_name":"PostToolUse","tool_name":"mcp__demo__list_issues","tool_use_id":"call-42","tool_response":response})
}

#[test]
fn codex_and_claude_spill_and_query() {
    for client in [Client::Codex, Client::Claude] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("db.duckdb"));
        let response = json!({"content":[{"type":"text","text":large_rows().to_string()}]});
        let output = hooks::hook(client, &event(response.clone()).to_string(), &store);
        assert!(output.to_string().len() < 8192);
        let text = match client {
            Client::Codex => {
                assert_eq!(output["continue"], false);
                assert!(output.get("decision").is_none());
                assert!(output.get("updatedMCPToolOutput").is_none());
                output["stopReason"].as_str().unwrap()
            }
            Client::Claude => {
                assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PostToolUse");
                output["hookSpecificOutput"]["updatedMCPToolOutput"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
            }
            _ => unreachable!(),
        };
        assert!(text.contains("Rows: 2000"));
        let datasets = store.list().unwrap();
        let name = datasets[0]["dataset"].as_str().unwrap();
        assert_eq!(
            store
                .query(&format!("SELECT count(*) FROM {name}"))
                .unwrap()["rows"],
            json!([[2000]])
        );
        // Full payload is retained even when the source used an MCP envelope.
        let conn = duckdb::Connection::open(store.path()).unwrap();
        let (original, call): (String, String) = conn
            .query_row(
                "SELECT original_json, source_call_id FROM _spill_datasets",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&original).unwrap(), response);
        assert_eq!(call, "call-42");
    }
}

#[test]
fn hook_pass_through_and_failure_are_nonblocking() {
    for client in [Client::Codex, Client::Claude] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("missing.duckdb"));
        for response in [
            json!([{"id":1}]),
            json!("x".repeat(100000)),
            json!(["x".repeat(100000)]),
            json!({"isError":true,"content":[{"type":"text","text":large_rows().to_string()}]}),
        ] {
            assert_eq!(
                hooks::hook(client, &event(response).to_string(), &store),
                json!({})
            );
        }
        for (field, value) in [
            ("hook_event_name", "PreToolUse"),
            ("tool_name", "Bash"),
            ("tool_name", "mcp__spill__query"),
            ("tool_name", "mcp__broken"),
        ] {
            let mut ev = event(large_rows());
            ev[field] = json!(value);
            assert_eq!(hooks::hook(client, &ev.to_string(), &store), json!({}));
        }
        assert_eq!(hooks::hook(client, "{invalid", &store), json!({}));
        assert!(!store.path().exists());
        let unavailable = Store::new(tmp.path());
        assert_eq!(
            hooks::hook(client, &event(large_rows()).to_string(), &unavailable),
            json!({})
        );
    }
}

#[test]
fn claude_content_blocks_are_unwrapped_not_stored_as_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("db.duckdb"));
    let blocks = json!([{"type":"text","text":large_rows().to_string()}]);
    let output = hooks::hook(Client::Claude, &event(blocks).to_string(), &store);
    assert!(output["hookSpecificOutput"]["updatedMCPToolOutput"].is_array());
    assert_eq!(store.list().unwrap()[0]["rows"], 2000);
    for blocks in [
        json!([{"type":"image","data":"x".repeat(100000)}]),
        json!([{"type":"text","text":large_rows().to_string()},{"type":"image","data":"abc"}]),
    ] {
        assert_eq!(
            hooks::hook(Client::Claude, &event(blocks).to_string(), &store),
            json!({})
        );
    }
    assert_eq!(store.list().unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn structured_content_with_short_text_spills_in_codex_and_claude() {
    for client in [Client::Codex, Client::Claude] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("db.duckdb"));
        let response = json!({
            "content": [{"type": "text", "text": "Found 2000 matching items"}],
            "structuredContent": large_rows()
        });
        let output = hooks::hook(client, &event(response).to_string(), &store);
        match client {
            Client::Codex => {
                assert_eq!(output["continue"], false);
                assert!(output["stopReason"]
                    .as_str()
                    .unwrap()
                    .contains("Rows: 2000"));
            }
            Client::Claude => {
                let text = output["hookSpecificOutput"]["updatedMCPToolOutput"]["content"][0]
                    ["text"]
                    .as_str()
                    .unwrap();
                assert!(text.contains("Rows: 2000"));
            }
            _ => unreachable!(),
        }
        assert_eq!(store.list().unwrap()[0]["rows"], 2000);
    }
}

#[test]
fn codex_config_comments_hooks_backups_idempotence_uninstall() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::new(tmp.path(), Client::Codex, None);
    fs::create_dir_all(paths.mcp.parent().unwrap()).unwrap();
    let original = "# Keep my settings\nmodel = 'my-model' # comment\n\n[mcp_servers.github]\ncommand = 'github-mcp'\nargs = ['serve']\n\n[mcp_servers.github.env]\nUSER_OPTION = 'keep'\n";
    let hooks = json!({"description":"mine","hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"audit"}]}],"Stop":[{"hooks":[{"type":"command","command":"stop"}]}]}});
    fs::write(&paths.mcp, original).unwrap();
    fs::write(&paths.hooks, hooks.to_string()).unwrap();
    install_client(&paths, Path::new("/path with spaces/spill"), Client::Codex).unwrap();
    let installed = fs::read_to_string(&paths.mcp).unwrap();
    assert!(installed.starts_with(original));
    let doc = installed.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        doc["mcp_servers"]["spill"]["command"].as_str(),
        Some("/path with spaces/spill")
    );
    assert_eq!(
        doc["mcp_servers"]["spill"]["args"]
            .as_array()
            .unwrap()
            .get(0)
            .unwrap()
            .as_str(),
        Some("mcp")
    );
    let count = fs::read_dir(paths.mcp.parent().unwrap()).unwrap().count();
    assert_eq!(count, 4);
    install_client(&paths, Path::new("/path with spaces/spill"), Client::Codex).unwrap();
    assert_eq!(fs::read_to_string(&paths.mcp).unwrap(), installed);
    assert_eq!(
        fs::read_dir(paths.mcp.parent().unwrap()).unwrap().count(),
        count
    );
    assert_eq!(
        read(&paths.hooks)["hooks"]["PostToolUse"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    uninstall_client(&paths, Client::Codex).unwrap();
    assert_eq!(read(&paths.hooks), hooks);
    assert_eq!(fs::read_to_string(&paths.mcp).unwrap(), original);
    uninstall_client(&paths, Client::Codex).unwrap();
}

#[test]
fn claude_merges_user_scope_and_preserves_other_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::new(tmp.path(), Client::Claude, None);
    fs::create_dir_all(paths.hooks.parent().unwrap()).unwrap();
    let original = json!({"mcpServers":{"github":{"command":"github-mcp"}},"projects":{"/repo":{"mcpServers":{"local":{"command":"local"}}}},"theme":"dark"});
    let settings = json!({"permissions":{"deny":["Bash(rm *)"]},"hooks":{"PostToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"format"}]}]}});
    fs::write(&paths.mcp, original.to_string()).unwrap();
    fs::write(&paths.hooks, settings.to_string()).unwrap();
    install_client(&paths, Path::new("/bin/spill"), Client::Claude).unwrap();
    let mcp = read(&paths.mcp);
    assert_eq!(
        mcp["mcpServers"]["spill"],
        json!({"type":"stdio","command":"/bin/spill","args":["mcp"]})
    );
    assert_eq!(mcp["projects"], original["projects"]);
    install_client(&paths, Path::new("/bin/spill"), Client::Claude).unwrap();
    assert_eq!(
        read(&paths.hooks)["hooks"]["PostToolUse"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    uninstall_client(&paths, Client::Claude).unwrap();
    assert_eq!(read(&paths.mcp), original);
    assert_eq!(read(&paths.hooks), settings);
}

#[test]
fn codex_preserves_modified_owned_entries_and_refuses_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::new(tmp.path(), Client::Codex, None);
    install_client(&paths, Path::new("/bin/spill"), Client::Codex).unwrap();
    let mut doc = fs::read_to_string(&paths.mcp)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    doc["mcp_servers"]["spill"]["enabled"] = toml_edit::value(false);
    fs::write(&paths.mcp, doc.to_string()).unwrap();
    assert!(install_client(&paths, Path::new("/bin/spill"), Client::Codex).is_err());
    uninstall_client(&paths, Client::Codex).unwrap();
    assert_eq!(fs::read_to_string(&paths.mcp).unwrap(), doc.to_string());
    assert!(install_client(&paths, Path::new("/bin/spill"), Client::Codex).is_err());
}

#[test]
fn malformed_configs_are_untouched_and_custom_roots_are_tracked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("custom");
    let paths = ConfigPaths::new(tmp.path(), Client::Codex, Some(&root));
    fs::create_dir_all(&root).unwrap();
    fs::write(&paths.mcp, "model = [invalid").unwrap();
    assert!(install_client(&paths, Path::new("/bin/spill"), Client::Codex).is_err());
    assert!(!paths.manifest.exists());
    assert!(!paths.hooks.exists());
    fs::write(&paths.mcp, "").unwrap();
    install_client(&paths, Path::new("/bin/spill"), Client::Codex).unwrap();
    let default = ConfigPaths::new(tmp.path(), Client::Codex, None);
    assert!(install_client(&default, Path::new("/bin/spill"), Client::Codex).is_err());
    assert!(uninstall_client(&default, Client::Codex).is_err());
    uninstall_client(&paths, Client::Codex).unwrap();
    let claude = ConfigPaths::new(tmp.path(), Client::Claude, Some(&root));
    assert_eq!(claude.mcp, root.join(".claude.json"));
    assert_eq!(claude.hooks, root.join("settings.json"));
}

#[test]
fn codex_inline_tables_are_supported_and_preexisting_entries_not_owned() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::new(tmp.path(), Client::Codex, None);
    fs::create_dir_all(paths.mcp.parent().unwrap()).unwrap();
    fs::write(
        &paths.mcp,
        "mcp_servers = { github = { command = 'github' } } # keep\n",
    )
    .unwrap();
    install_client(&paths, Path::new("/bin/spill"), Client::Codex).unwrap();
    let doc = fs::read_to_string(&paths.mcp)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(
        doc["mcp_servers"]["github"]["command"].as_str(),
        Some("github")
    );
    assert_eq!(
        doc["mcp_servers"]["spill"]["command"].as_str(),
        Some("/bin/spill")
    );
    fs::remove_file(&paths.manifest).unwrap();
    let before = fs::read(&paths.mcp).unwrap();
    let hooks = fs::read(&paths.hooks).unwrap();
    install_client(&paths, Path::new("/bin/spill"), Client::Codex).unwrap();
    uninstall_client(&paths, Client::Codex).unwrap();
    assert_eq!(fs::read(&paths.mcp).unwrap(), before);
    assert_eq!(fs::read(&paths.hooks).unwrap(), hooks);
}
