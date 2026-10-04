use serde_json::{json, Value};
use spill::{
    cursor,
    db::{identifier, Store},
};

#[test]
fn types_persistence_metadata_and_query() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("spill.duckdb");
    let store = Store::new(&path);
    let rows = json!([
        {"id":1,"name":"Alice","ratio":1.5,"active":true,"empty":null,"object":{"k":"v"},"array":[1,2],"mixed":42},
        {"id":2,"name":"Bob","ratio":2.5,"active":false,"empty":null,"object":{},"array":[],"mixed":"forty-two"},
        {"id":3}
    ]);
    let raw = rows.to_string();
    let dataset = store
        .materialize(
            "github.list/issues",
            Some("call-1"),
            rows.as_array().unwrap(),
            &raw,
        )
        .unwrap();
    assert!(dataset.dataset.starts_with("spill_github_list_issues_"));
    assert_eq!(dataset.rows, 3);
    drop(store);
    let store = Store::new(&path);
    let list = store.list().unwrap();
    assert_eq!(list[0]["rows"], 3);
    assert_eq!(list[0]["source_tool"], "github.list/issues");
    let description = store.describe(&dataset.dataset).unwrap();
    for (name, ty) in [
        ("id", "BIGINT"),
        ("ratio", "DOUBLE"),
        ("active", "BOOLEAN"),
        ("name", "VARCHAR"),
        ("object", "VARCHAR"),
        ("array", "VARCHAR"),
    ] {
        assert!(description["columns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == name && c["data_type"] == ty));
    }
    let q = store
        .query(&format!(
            "SELECT id, name, ratio, active, object, \"array\", mixed FROM {} ORDER BY id",
            identifier(&dataset.dataset)
        ))
        .unwrap();
    assert_eq!(
        q["rows"][0],
        json!([1, "Alice", 1.5, true, "{\"k\":\"v\"}", "[1,2]", "42"])
    );
    assert_eq!(q["rows"][2], json!([3, null, null, null, null, null, null]));
    let q = store
        .query("SELECT source_call_id, original_json, original_bytes FROM _spill_datasets")
        .unwrap();
    assert_eq!(q["rows"][0], json!(["call-1", raw, raw.len()]));
    assert_eq!(
        store
            .query(&format!(
                "SELECT count(*) AS n FROM {}",
                identifier(&dataset.dataset)
            ))
            .unwrap()["rows"][0][0],
        3
    );
    assert!(store.describe("missing").is_err());
}

#[test]
fn read_only_and_output_limits() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows = json!([{"id":1}]);
    let ds = store
        .materialize("tool", None, rows.as_array().unwrap(), &rows.to_string())
        .unwrap();
    for sql in [
        format!("DROP TABLE {}", ds.dataset),
        format!("DELETE FROM {}", ds.dataset),
        "SELECT 1; CREATE TABLE x (id INT)".into(),
        "EXPLAIN ANALYZE DELETE FROM _spill_datasets".into(),
        "SELECT * FROM read_csv_auto('/etc/passwd')".into(),
    ] {
        assert!(store.query(&sql).is_err(), "{sql}");
    }
    assert!(store.query("SELECT * FROM range(501)").is_err());
    assert_eq!(
        store.query("SELECT * FROM range(500)").unwrap()["rows"]
            .as_array()
            .unwrap()
            .len(),
        500
    );
    assert!(store.query("SELECT repeat('x', 40000)").is_err());
    let long_alias = "x".repeat(40_000);
    for suffix in ["", " WHERE false"] {
        assert!(store
            .query(&format!("SELECT 1 AS {}{suffix}", identifier(&long_alias)))
            .is_err());
    }
    // Row values alone fit the budget; the column names and result framing do not.
    assert!(store.query("SELECT repeat('x', 32750) AS payload").is_err());
    assert_eq!(
        store.query("SELECT 1 AS n WHERE false").unwrap(),
        json!({"columns":["n"],"rows":[]})
    );
    assert!(store.query("SHOW TABLES").is_ok());
    assert!(store.query(&format!("DESCRIBE {}", ds.dataset)).is_ok());
    assert!(store.query("EXPLAIN SELECT 1").is_ok());
    assert_eq!(
        store.query("SELECT replace('abc', 'b', 'x') AS r").unwrap()["rows"][0],
        json!(["axc"])
    );
    assert_eq!(
        store.query("SELECT 1 AS call, 2 AS set").unwrap()["rows"][0],
        json!([1, 2])
    );
    assert!(store.query("(SELECT 1)").is_ok());
}

#[test]
fn cursor_hook_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows: Vec<Value> = (0..2000)
        .map(|id| json!({"id":id,"state":if id%2==0 {"OPEN"} else {"CLOSED"}}))
        .collect();
    let raw = json!({"content":[{"type":"text","text":serde_json::to_string(&rows).unwrap()}]})
        .to_string();
    let event = json!({"hook_event_name":"postToolUse","tool_name":"MCP:github_list_issues","tool_use_id":"test-call","tool_output":raw});
    let out = cursor::hook(&event.to_string(), &store);
    assert!(out.to_string().len() < 8192);
    assert!(out["updated_mcp_tool_output"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Rows: 2000"));
    let datasets = store.list().unwrap();
    let name = datasets[0]["dataset"].as_str().unwrap();
    assert_eq!(
        store
            .query(&format!("SELECT count(*) FROM {name} WHERE state = 'OPEN'"))
            .unwrap()["rows"][0][0],
        1000
    );
    assert_eq!(
        cursor::hook(
            &json!({"tool_name":"MCP:small","tool_output":"[{\"a\":1}]"}).to_string(),
            &store
        ),
        json!({})
    );
    assert_eq!(store.list().unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn lossless_fallback_and_unusual_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows: Value = serde_json::from_str(r#"[{"a\"b":18446744073709551615,"number":9007199254740993},{"a\"b":{"k":1},"number":1.5}]"#).unwrap();
    let ds = store
        .materialize("!", None, rows.as_array().unwrap(), &rows.to_string())
        .unwrap();
    let out = store
        .query(&format!("SELECT * FROM {}", ds.dataset))
        .unwrap();
    assert_eq!(
        out["rows"][0],
        json!(["18446744073709551615", "9007199254740993"])
    );
    let invalid = json!([{"a":1,"A":2}]);
    assert!(store
        .materialize(
            "bad",
            None,
            invalid.as_array().unwrap(),
            &invalid.to_string()
        )
        .is_err());
    assert_eq!(store.list().unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn database_enforces_read_only_beyond_keyword_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows = json!([{"id": 1}]);
    let dataset = store
        .materialize("tool", None, rows.as_array().unwrap(), &rows.to_string())
        .unwrap();
    let sql = format!(
        "SELECT * FROM query('DELETE FROM {} RETURNING *')",
        dataset.dataset
    );
    // The mutation is inside a SQL string, so the keyword policy permits it.
    // DuckDB's read-only connection must still prevent it.
    assert!(spill::sql::validate(&sql).is_ok());
    assert!(store.query(&sql).is_err());
    assert_eq!(
        store
            .query(&format!("SELECT count(*) FROM {}", dataset.dataset))
            .unwrap()["rows"][0][0],
        1
    );
}

#[test]
fn failed_metadata_insert_rolls_back_dataset_table() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("spill.duckdb");
    {
        let connection = duckdb::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE _spill_datasets (dataset_name VARCHAR CHECK (false), source_tool VARCHAR, source_call_id VARCHAR, created_at TIMESTAMP, row_count BIGINT, original_bytes BIGINT, original_json VARCHAR)").unwrap();
    }
    let store = Store::new(&path);
    assert!(store
        .materialize("failed", None, &[json!({"id": 1})], "[{\"id\":1}]")
        .is_err());
    assert_eq!(store.list().unwrap(), json!([]));
    assert_eq!(
        store.query("SHOW TABLES").unwrap()["rows"],
        json!([["_spill_datasets"]])
    );
}

#[test]
fn large_payload_preserves_original_json() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let large_str = "x".repeat(1_200_000);
    let rows = json!([{"data": large_str}, {"data": null}, {}]);
    let raw = json!({"structuredContent": rows, "nextCursor": "next-page"}).to_string();
    let ds = store
        .materialize("tool", Some("call-big"), rows.as_array().unwrap(), &raw)
        .unwrap();
    assert_eq!(ds.original_bytes, raw.len() as i64);

    let conn = duckdb::Connection::open(store.path()).unwrap();
    let (stored, bytes): (String, i64) = conn
        .query_row(
            "SELECT original_json, original_bytes FROM _spill_datasets WHERE dataset_name = ?",
            [&ds.dataset],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(bytes, raw.len() as i64);
    assert_eq!(stored, raw);
}

#[test]
fn uninitialized_database_handles_query_and_describe_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("sub/spill.duckdb"));
    assert_eq!(store.list().unwrap(), json!([]));
    assert!(store.describe("nonexistent").is_err());
    let res = store.query("SELECT 42 AS answer").unwrap();
    assert_eq!(res["rows"], json!([[42]]));
    assert!(store.path().exists());
}

#[test]
fn large_numbers_fallback_to_json_safely() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows: Value = serde_json::from_str(r#"[{"huge": 1e999}]"#).unwrap();
    let ds = store
        .materialize("calc", None, rows.as_array().unwrap(), &rows.to_string())
        .unwrap();
    assert_eq!(ds.columns[0].data_type, "VARCHAR");
    assert_eq!(
        store
            .query(&format!("SELECT huge FROM {}", ds.dataset))
            .unwrap()["rows"],
        json!([["1e+999"]])
    );
}

#[test]
fn unsigned_integers_above_bigint_round_trip_exactly() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows: Value =
        serde_json::from_str(r#"[{"n":9223372036854775809},{"n":18446744073709551615}]"#).unwrap();
    let ds = store
        .materialize("numbers", None, rows.as_array().unwrap(), &rows.to_string())
        .unwrap();
    assert_eq!(ds.columns[0].data_type, "VARCHAR");
    assert_eq!(
        store
            .query(&format!("SELECT n FROM {} ORDER BY n", ds.dataset))
            .unwrap()["rows"],
        json!([["18446744073709551615"], ["9223372036854775809"]])
    );
}

#[test]
fn mixed_numeric_metadata_preserves_unusual_column_names_and_legacy_records() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows =
        json!([{"a,b": 1, "normal": 1}, {"a,b": 1.5, "normal": 1.5}, {"a,b": 2, "normal": 2}]);
    let ds = store
        .materialize("mixed", None, rows.as_array().unwrap(), &rows.to_string())
        .unwrap();
    assert!(ds.columns.iter().all(|c| c.mixed));
    let description = store.describe(&ds.dataset).unwrap();
    assert!(description["columns"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["mixed"] == true));
    {
        let conn = duckdb::Connection::open(store.path()).unwrap();
        conn.execute(
            "UPDATE _spill_datasets SET mixed_columns = ? WHERE dataset_name = ?",
            ["normal", &ds.dataset],
        )
        .unwrap();
    }
    let description = store.describe(&ds.dataset).unwrap();
    let columns = description["columns"].as_array().unwrap();
    assert!(columns
        .iter()
        .any(|c| c["name"] == "normal" && c["mixed"] == true));
    assert!(columns
        .iter()
        .any(|c| c["name"] == "a,b" && c["mixed"] == false));
}

#[test]
fn dataset_list_includes_metadata_in_byte_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("spill.duckdb"));
    let rows = json!([{"id": 1}]);
    store
        .materialize(
            &"x".repeat(40_000),
            None,
            rows.as_array().unwrap(),
            &rows.to_string(),
        )
        .unwrap();
    assert!(store.list().is_err());
}
