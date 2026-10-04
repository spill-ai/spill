use serde_json::{json, Value};
use spill::install::{install, uninstall};
use std::{fs, path::Path};

fn read(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn merge_idempotence_backups_and_uninstall() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    fs::create_dir(home.join(".cursor")).unwrap();
    let mcp_path = home.join(".cursor/mcp.json");
    let hooks_path = home.join(".cursor/hooks.json");
    let mcp = json!({"mcpServers":{"github":{"command":"github-mcp","env":{"USER_OPTION":"kept"}}},"other":true});
    let hooks = json!({"version":1,"hooks":{"postToolUse":[{"command":"audit.sh"}],"stop":[{"command":"stop.sh"}]}});
    fs::write(&mcp_path, mcp.to_string()).unwrap();
    fs::write(&hooks_path, hooks.to_string()).unwrap();
    install(home, Path::new("/path with space/it's spill")).unwrap();
    let first_mcp = fs::read(&mcp_path).unwrap();
    let first_hooks = fs::read(&hooks_path).unwrap();
    let count = fs::read_dir(home.join(".cursor")).unwrap().count();
    assert_eq!(count, 4); // Two configs and two backups.
    install(home, Path::new("/path with space/it's spill")).unwrap();
    assert_eq!(fs::read(&mcp_path).unwrap(), first_mcp);
    assert_eq!(fs::read(&hooks_path).unwrap(), first_hooks);
    assert_eq!(fs::read_dir(home.join(".cursor")).unwrap().count(), count);
    let config = read(&hooks_path);
    assert_eq!(config["hooks"]["postToolUse"].as_array().unwrap().len(), 2);
    assert_eq!(
        config["hooks"]["postToolUse"][1]["command"],
        "'/path with space/it'\\''s spill' hook cursor"
    );
    uninstall(home).unwrap();
    assert_eq!(read(&mcp_path), mcp);
    assert_eq!(read(&hooks_path), hooks);
    uninstall(home).unwrap();
}

#[test]
fn preserve_user_changes_and_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    install(home, Path::new("/bin/spill")).unwrap();
    let mcp_path = home.join(".cursor/mcp.json");
    let mut mcp = read(&mcp_path);
    mcp["mcpServers"]["spill"]["args"] = json!(["custom"]);
    fs::write(&mcp_path, mcp.to_string()).unwrap();
    assert!(install(home, Path::new("/bin/spill")).is_err());
    uninstall(home).unwrap();
    assert_eq!(read(&mcp_path), mcp);
    assert!(install(home, Path::new("/bin/spill")).is_err());
}

#[test]
fn validate_both_files_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join(".cursor")).unwrap();
    fs::write(tmp.path().join(".cursor/hooks.json"), "not json").unwrap();
    assert!(install(tmp.path(), Path::new("/bin/spill")).is_err());
    assert!(!tmp.path().join(".cursor/mcp.json").exists());
    assert!(!tmp.path().join(".spill/cursor-install.json").exists());
}

#[test]
fn preexisting_identical_entries_are_not_owned() {
    let tmp = tempfile::tempdir().unwrap();
    install(tmp.path(), Path::new("/bin/spill")).unwrap();
    fs::remove_file(tmp.path().join(".spill/cursor-install.json")).unwrap();
    let before_mcp = read(tmp.path().join(".cursor/mcp.json"));
    let before_hooks = read(tmp.path().join(".cursor/hooks.json"));
    install(tmp.path(), Path::new("/bin/spill")).unwrap();
    uninstall(tmp.path()).unwrap();
    assert_eq!(read(tmp.path().join(".cursor/mcp.json")), before_mcp);
    assert_eq!(read(tmp.path().join(".cursor/hooks.json")), before_hooks);
}

#[test]
fn executable_migration_replaces_only_owned_entries() {
    let tmp = tempfile::tempdir().unwrap();
    install(tmp.path(), Path::new("/old/spill")).unwrap();
    install(tmp.path(), Path::new("/new/spill")).unwrap();
    let config = read(tmp.path().join(".cursor/hooks.json"));
    assert_eq!(config["hooks"]["postToolUse"].as_array().unwrap().len(), 1);
    assert_eq!(
        config["hooks"]["postToolUse"][0]["command"],
        "'/new/spill' hook cursor"
    );
    uninstall(tmp.path()).unwrap();
    assert!(read(tmp.path().join(".cursor/mcp.json"))["mcpServers"]
        .as_object()
        .unwrap()
        .is_empty());
}
