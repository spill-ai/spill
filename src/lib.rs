pub mod claude;
pub mod client;
pub mod codex;
pub mod cursor;
pub mod db;
pub mod hooks;
pub mod install;
pub mod mcp;
pub mod policy;
mod result;
pub mod sql;

use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn home() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .context("Cannot locate the home directory")
}

pub fn database_path() -> Result<PathBuf> {
    Ok(home()?.join(".spill/spill.duckdb"))
}
