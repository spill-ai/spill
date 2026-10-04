use crate::client::Client;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Ownership {
    server: Value,
    hook: Value,
    added_server: bool,
    added_hook: bool,
    // Optional for compatibility with existing Cursor installation manifests.
    #[serde(default)]
    mcp_path: Option<PathBuf>,
    #[serde(default)]
    hooks_path: Option<PathBuf>,
}

/// Explicit paths keep tests isolated from the real user's client configuration.
pub struct ConfigPaths {
    pub mcp: PathBuf,
    pub hooks: PathBuf,
    pub manifest: PathBuf,
}
impl ConfigPaths {
    pub fn new(home: &Path, client: Client, config_root: Option<&Path>) -> Self {
        let default_root = home.join(format!(".{}", client.name()));
        let root = config_root.unwrap_or(&default_root);
        let (mcp, hooks) = match client {
            Client::Cursor => (root.join("mcp.json"), root.join("hooks.json")),
            Client::Codex => (root.join("config.toml"), root.join("hooks.json")),
            Client::Claude => (
                if config_root.is_some() {
                    root.join(".claude.json")
                } else {
                    home.join(".claude.json")
                },
                root.join("settings.json"),
            ),
        };
        Self {
            mcp,
            hooks,
            manifest: home.join(format!(".spill/{}-install.json", client.name())),
        }
    }
}

fn read(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&fs::read(path)?)
        .with_context(|| format!("Invalid JSON in {}", path.display()))
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() && fs::read(path)? == bytes {
        return Ok(());
    }
    let parent = path.parent().context("Missing config directory")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    if path.exists() {
        temporary
            .as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
        let backup = parent.join(format!(
            "{}.spill-backup-{}",
            path.file_name()
                .context("Missing filename")?
                .to_string_lossy(),
            Uuid::new_v4().simple()
        ));
        fs::copy(path, backup)?;
    }
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn write(path: &Path, value: &Value) -> Result<()> {
    if path.exists() && read(path)? == *value {
        return Ok(());
    }
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_bytes(path, &bytes)
}

fn object_field<'a>(
    value: &'a mut Value,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    value
        .as_object_mut()
        .context("Configuration must be a JSON object")?
        .entry(key.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("{key} must be an object"))
}

enum McpConfig {
    Json(Value),
    Toml(DocumentMut),
}
impl McpConfig {
    fn read(path: &Path, client: Client) -> Result<Self> {
        if client == Client::Codex {
            let text = if path.exists() {
                fs::read_to_string(path)?
            } else {
                String::new()
            };
            let document = text
                .parse::<DocumentMut>()
                .context("Invalid Codex TOML configuration")?;
            if document
                .get("mcp_servers")
                .is_some_and(|v| v.as_table_like().is_none())
            {
                bail!("mcp_servers must be a TOML table");
            }
            Ok(Self::Toml(document))
        } else {
            let value = read(path)?;
            if !value.is_object() || value.get("mcpServers").is_some_and(|v| !v.is_object()) {
                bail!("MCP configuration and mcpServers must be JSON objects");
            }
            Ok(Self::Json(value))
        }
    }

    fn server(&self) -> Option<Value> {
        match self {
            Self::Json(value) => value.get("mcpServers")?.get("spill").cloned(),
            Self::Toml(doc) => {
                let item = doc.get("mcp_servers")?.as_table_like()?.get("spill")?;
                // Only Spill's exact command/args pair is owned. Any added
                // setting (env, enabled, etc.) counts as a user modification.
                if let Some(table) = item.as_table_like() {
                    if table.len() == 2 {
                        if let (Some(command), Some(args)) = (
                            table.get("command").and_then(Item::as_str),
                            table.get("args").and_then(Item::as_array),
                        ) {
                            if let Some(args) =
                                args.iter().map(|v| v.as_str()).collect::<Option<Vec<_>>>()
                            {
                                return Some(json!({"command": command, "args": args}));
                            }
                        }
                    }
                }
                Some(json!({"user_modified_toml": item.to_string()}))
            }
        }
    }

    fn set_server(&mut self, server: &Value) -> Result<()> {
        match self {
            Self::Json(value) => {
                object_field(value, "mcpServers")?.insert("spill".into(), server.clone());
            }
            Self::Toml(doc) => {
                let parent = doc
                    .entry("mcp_servers")
                    .or_insert(Item::Table(Table::new()));
                let mut args = Array::new();
                args.push("mcp");
                let command = server["command"]
                    .as_str()
                    .context("Missing Spill executable")?;
                if parent.is_inline_table() {
                    let mut entry = InlineTable::new();
                    entry.insert("command", command.into());
                    entry.insert("args", args.into());
                    parent
                        .as_table_like_mut()
                        .unwrap()
                        .insert("spill", Item::Value(entry.into()));
                } else {
                    let mut entry = Table::new();
                    entry.insert("command", toml_edit::value(command));
                    entry.insert("args", toml_edit::value(args));
                    parent
                        .as_table_like_mut()
                        .context("Invalid mcp_servers table")?
                        .insert("spill", Item::Table(entry));
                }
            }
        }
        Ok(())
    }

    fn remove_server(&mut self) {
        match self {
            Self::Json(value) => {
                if let Some(servers) = value.get_mut("mcpServers").and_then(Value::as_object_mut) {
                    servers.remove("spill");
                }
            }
            Self::Toml(doc) => {
                if let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut)
                {
                    servers.remove("spill");
                }
            }
        }
    }

    fn write(&self, path: &Path) -> Result<()> {
        match self {
            Self::Json(v) => write(path, v),
            Self::Toml(doc) => write_bytes(path, doc.to_string().as_bytes()),
        }
    }
}

fn check_paths(prior: &Ownership, paths: &ConfigPaths) -> Result<()> {
    if prior.mcp_path.as_ref().is_some_and(|p| p != &paths.mcp)
        || prior.hooks_path.as_ref().is_some_and(|p| p != &paths.hooks)
    {
        bail!("Spill is installed in another configuration directory. Uninstall using that directory before switching.");
    }
    Ok(())
}

pub fn install_client(paths: &ConfigPaths, executable: &Path, client: Client) -> Result<()> {
    let exe = executable
        .to_str()
        .context("Executable path is not UTF-8")?;
    let mut desired_server = json!({"command": exe, "args": ["mcp"]});
    if client == Client::Claude {
        desired_server["type"] = json!("stdio");
    }
    let quoted_exe = format!("'{}'", exe.replace('\'', "'\\''"));
    let command = format!("{quoted_exe} hook {}", client.name());
    let desired_hook = if client == Client::Cursor {
        json!({"command": command, "matcher":"MCP:.*", "timeout": 60})
    } else {
        json!({"matcher":"^mcp__", "hooks":[{"type":"command", "command":command, "timeout":60}]})
    };
    let prior: Option<Ownership> = if paths.manifest.exists() {
        Some(serde_json::from_value(read(&paths.manifest)?)?)
    } else {
        None
    };
    if let Some(prior) = &prior {
        check_paths(prior, paths)?;
    }
    let mut mcp = McpConfig::read(&paths.mcp, client)?;
    let mut hooks = read(&paths.hooks)?;
    let existing_server = mcp.server();
    if let Some(existing) = &existing_server {
        if existing != &desired_server
            && !prior
                .as_ref()
                .is_some_and(|p| p.added_server && existing == &p.server)
        {
            bail!("An unrelated or modified MCP server named 'spill' exists; leaving configuration unchanged");
        }
    }
    let added_server = prior.as_ref().map_or(existing_server.is_none(), |p| {
        p.added_server || existing_server.is_none()
    });
    if existing_server.as_ref() != Some(&desired_server) {
        mcp.set_server(&desired_server)?;
    }
    if client == Client::Cursor {
        let root = hooks
            .as_object_mut()
            .context("Hooks configuration must be an object")?;
        if root.get("version").is_some_and(|v| v != &json!(1)) {
            bail!("Unsupported Cursor hooks version");
        }
        root.entry("version").or_insert(json!(1));
    }
    let events = object_field(&mut hooks, "hooks")?;
    let entries = events
        .entry(client.event())
        .or_insert(json!([]))
        .as_array_mut()
        .context("Post-tool hooks must be an array")?;
    let mut added_hook = prior.as_ref().is_some_and(|p| p.added_hook);
    if let Some(prior) = &prior {
        if prior.added_hook && prior.hook != desired_hook {
            entries.retain(|entry| entry != &prior.hook);
        }
    }
    if !entries.contains(&desired_hook) {
        entries.push(desired_hook.clone());
        added_hook = true;
    }
    let ownership = Ownership {
        server: desired_server,
        hook: desired_hook,
        added_server,
        added_hook,
        mcp_path: Some(paths.mcp.clone()),
        hooks_path: Some(paths.hooks.clone()),
    };
    // Both configs are validated before writing. Ownership is persisted first,
    // allowing retries/uninstall after an interrupted installation.
    write(&paths.manifest, &serde_json::to_value(ownership)?)?;
    mcp.write(&paths.mcp)?;
    write(&paths.hooks, &hooks)?;
    Ok(())
}

pub fn uninstall_client(paths: &ConfigPaths, client: Client) -> Result<()> {
    if !paths.manifest.exists() {
        return Ok(());
    }
    let ownership: Ownership = serde_json::from_value(read(&paths.manifest)?)?;
    check_paths(&ownership, paths)?;
    let mut mcp = McpConfig::read(&paths.mcp, client)?;
    let mut hooks = read(&paths.hooks)?;
    if ownership.added_server && mcp.server().as_ref() == Some(&ownership.server) {
        mcp.remove_server();
    }
    if ownership.added_hook {
        if let Some(entries) = hooks
            .get_mut("hooks")
            .and_then(|h| h.get_mut(client.event()))
            .and_then(Value::as_array_mut)
        {
            entries.retain(|entry| entry != &ownership.hook);
        }
    }
    if paths.mcp.exists() {
        mcp.write(&paths.mcp)?;
    }
    if paths.hooks.exists() {
        write(&paths.hooks, &hooks)?;
    }
    fs::remove_file(&paths.manifest)?;
    Ok(())
}

// Preserve the original Cursor library entry points.
pub fn install(home: &Path, executable: &Path) -> Result<()> {
    install_client(
        &ConfigPaths::new(home, Client::Cursor, None),
        executable,
        Client::Cursor,
    )
}
pub fn uninstall(home: &Path) -> Result<()> {
    uninstall_client(
        &ConfigPaths::new(home, Client::Cursor, None),
        Client::Cursor,
    )
}
