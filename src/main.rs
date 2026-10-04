use anyhow::Result;
use clap::{Parser, Subcommand};
use spill::{
    client::Client,
    db::Store,
    hooks,
    install::{self, ConfigPaths},
    mcp,
};
use std::io::{self, Read};

#[derive(Parser)]
#[command(
    name = "spill",
    version,
    about = "Keep large MCP tool results out of the context window"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Configure a client’s global MCP server and post-tool hook.
    Install { client: Client },
    /// Remove only the configuration added by Spill. Keep stored datasets.
    Uninstall { client: Client },
    /// Process one client hook event on stdin, failing open.
    Hook { client: Client },
    /// Run the local stdio MCP server.
    Mcp,
    /// List stored datasets.
    List,
    /// Inspect a dataset's schema and metadata.
    Describe { dataset: String },
    /// Run bounded, read-only SQL against stored datasets.
    Sql { query: String },
}
#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("spill: {error:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    // Even home-directory or stdin errors must leave the original MCP output intact.
    if let Command::Hook { client } = cli.command {
        let result = (|| -> Result<_> {
            let store = Store::new(spill::database_path()?);
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            Ok(hooks::hook(client, &input, &store))
        })();
        let output = result.unwrap_or_else(|_| {
            eprintln!("spill: hook unavailable; preserving original output");
            serde_json::json!({})
        });
        println!("{output}");
        return Ok(());
    }
    let store = Store::new(spill::database_path()?);
    let value = match cli.command {
        Command::Install { client } => {
            store.init()?;
            install::install_client(&config_paths(client)?, &stable_executable_path()?, client)?;
            eprintln!(
                "spill: {} configured; restart the client to load MCP and hook settings",
                client.name()
            );
            return Ok(());
        }
        Command::Uninstall { client } => {
            install::uninstall_client(&config_paths(client)?, client)?;
            eprintln!("spill: Spill-owned {} configuration removed", client.name());
            return Ok(());
        }
        Command::Mcp => return mcp::serve(store).await,
        Command::List => store.list()?,
        Command::Describe { dataset } => store.describe(&dataset)?,
        Command::Sql { query } => store.query(&query)?,
        Command::Hook { .. } => unreachable!(),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn stable_executable_path() -> Result<std::path::PathBuf> {
    // Walk PATH to find a symlink whose canonical target matches the running
    // binary. This keeps Homebrew installs stable across `brew upgrade` by
    // writing `/opt/homebrew/bin/spill` into client configs rather than the
    // versioned Cellar path it symlinks to.
    //
    // Limitation: exec-based version manager wrappers (asdf, mise, etc.) are
    // shell scripts that exec the real binary, so `canonicalize` resolves to
    // the wrapper itself rather than the underlying binary. In that case the
    // function falls back to `current_exe()`, which is the versioned binary
    // path. Users on such setups should ensure their version manager's shim
    // directory is stable across upgrades, or pass an explicit path via the
    // SPILL_BIN environment variable in a future release.
    let current = std::env::current_exe()?;
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("spill");
            if candidate.is_file() {
                if let (Ok(canon_candidate), Ok(canon_current)) =
                    (candidate.canonicalize(), current.canonicalize())
                {
                    if canon_candidate == canon_current {
                        return Ok(candidate);
                    }
                }
            }
        }
    }
    Ok(current)
}

fn config_paths(client: Client) -> Result<ConfigPaths> {
    let variable = match client {
        Client::Cursor => None,
        Client::Codex => Some("CODEX_HOME"),
        Client::Claude => Some("CLAUDE_CONFIG_DIR"),
    };
    let root = variable
        .and_then(std::env::var_os)
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);
    if root.as_ref().is_some_and(|p| !p.is_absolute()) {
        anyhow::bail!("Client configuration directory must be an absolute path");
    }
    Ok(ConfigPaths::new(&spill::home()?, client, root.as_deref()))
}
