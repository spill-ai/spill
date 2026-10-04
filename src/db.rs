use anyhow::{bail, Context, Result};
use duckdb::ffi::ErrorCode;
use duckdb::{
    appender_params_from_iter, params, types::Value as SqlValue, AccessMode, Config, Connection,
    Error as DuckDbError,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const MAX_ROWS: usize = 500;
pub const MAX_BYTES: usize = 32 * 1024;

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    /// True when the column was inferred as VARCHAR due to mixed numeric types
    /// (e.g. integers and floats in the same field). Numeric predicates require CAST.
    pub mixed: bool,
}

#[derive(Debug, Serialize)]
pub struct Dataset {
    pub dataset: String,
    pub rows: i64,
    pub source_tool: String,
    pub created_at: String,
    pub original_bytes: i64,
    pub columns: Vec<Column>,
}

#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Null,
    Bool,
    Int,
    Float,
    String,
    /// Produced by `Kind::of` for objects and arrays.
    Json,
    /// Produced by `merge` when Int and Float appear in the same column.
    /// Stored as VARCHAR (JSON-encoded) to avoid rounding; numeric predicates need CAST.
    MixedNumeric,
}
impl Kind {
    fn of(v: &Value) -> Self {
        match v {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Bool,
            Value::Number(n) if n.as_i64().is_some() => Self::Int,
            Value::Number(n) if n.is_f64() && n.as_f64().is_some() => Self::Float,
            Value::String(_) => Self::String,
            _ => Self::Json,
        }
    }
    fn merge(self, other: Self) -> Self {
        if self == Self::Null {
            return other;
        }
        if other == Self::Null || self == other {
            return self;
        }
        // Int + Float (or MixedNumeric + either) → MixedNumeric (VARCHAR, but worth a CAST warning).
        let is_numeric = |k| matches!(k, Self::Int | Self::Float | Self::MixedNumeric);
        if is_numeric(self) && is_numeric(other) {
            return Self::MixedNumeric;
        }
        // Everything else (e.g. String + Int, Bool + String, Json + anything) → Json.
        Self::Json
    }
    fn sql(self) -> &'static str {
        match self {
            Self::Bool => "BOOLEAN",
            Self::Int => "BIGINT",
            Self::Float => "DOUBLE",
            Self::Null | Self::String | Self::Json | Self::MixedNumeric => "VARCHAR",
        }
    }
    fn value(self, v: &Value) -> SqlValue {
        if v.is_null() {
            return SqlValue::Null;
        }
        match self {
            Self::Bool => SqlValue::Boolean(v.as_bool().unwrap_or(false)),
            Self::Int => SqlValue::BigInt(v.as_i64().unwrap_or(0)),
            Self::Float => SqlValue::Double(v.as_f64().unwrap_or(0.0)),
            Self::String => SqlValue::Text(v.as_str().unwrap_or("").into()),
            _ => SqlValue::Text(v.to_string()),
        }
    }
}

pub fn identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn init(&self) -> Result<()> {
        let _conn = self.open(false)?;
        Ok(())
    }

    fn open(&self, read_only: bool) -> Result<Connection> {
        if !read_only {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }
        }
        let make_config = || -> Result<Config> {
            Ok(Config::default()
                .access_mode(if read_only {
                    AccessMode::ReadOnly
                } else {
                    AccessMode::ReadWrite
                })?
                .enable_external_access(false)?
                .enable_autoload_extension(false)?
                .with("autoinstall_known_extensions", "false")?)
        };
        let mut attempts = 0;
        let conn = loop {
            let config = make_config()?;
            match Connection::open_with_flags(&self.path, config) {
                Ok(conn) => break conn,
                Err(DuckDbError::DuckDBFailure(ref ffi_err, _))
                    if matches!(
                        ffi_err.code,
                        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked | ErrorCode::CannotOpen
                    ) && attempts < 5 =>
                {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(25 * attempts));
                    continue;
                }
                Err(e) => return Err(e).context("Cannot open Spill database"),
            }
        };
        if !read_only {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS _spill_datasets (
                dataset_name VARCHAR PRIMARY KEY, source_tool VARCHAR, source_call_id VARCHAR,
                created_at TIMESTAMP, row_count BIGINT, original_bytes BIGINT,
                original_json VARCHAR, mixed_columns VARCHAR
            );
            ALTER TABLE _spill_datasets ADD COLUMN IF NOT EXISTS original_json VARCHAR;
            ALTER TABLE _spill_datasets ADD COLUMN IF NOT EXISTS mixed_columns VARCHAR;",
            )?;
        }
        Ok(conn)
    }

    pub fn materialize(
        &self,
        tool: &str,
        call_id: Option<&str>,
        rows: &[Value],
        original: &str,
    ) -> Result<Dataset> {
        if rows.is_empty() || !rows.iter().all(Value::is_object) {
            bail!("Expected a nonempty array of objects");
        }
        let mut kinds = BTreeMap::<String, Kind>::new();
        for row in rows {
            for (key, value) in row.as_object().unwrap() {
                let kind = kinds.entry(key.clone()).or_insert(Kind::Null);
                *kind = kind.merge(Kind::of(value));
            }
        }
        // DuckDB identifiers are case-insensitive. Preserve all source keys in
        // original_json and fail open rather than silently renaming or losing data.
        let mut keys = HashSet::new();
        for key in kinds.keys() {
            if key.is_empty() || key.contains('\0') || !keys.insert(key.to_ascii_lowercase()) {
                bail!("Dataset has empty, NUL, or case-colliding column names");
            }
        }
        if kinds.is_empty() {
            kinds.insert("_spill_empty_object".into(), Kind::Null);
        }
        let stem: String = tool
            .chars()
            .take(48)
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let name = format!(
            "spill_{}_{}",
            if stem.is_empty() { "tool" } else { &stem },
            &Uuid::new_v4().simple().to_string()[..12]
        );
        let columns: Vec<Column> = kinds
            .iter()
            .map(|(key, kind)| Column {
                name: key.clone(),
                data_type: kind.sql().into(),
                mixed: *kind == Kind::MixedNumeric,
            })
            .collect();
        let mut conn = self.open(false)?;
        let tx = conn.transaction()?;
        let schema = columns
            .iter()
            .map(|c| format!("{} {}", identifier(&c.name), c.data_type))
            .collect::<Vec<_>>()
            .join(", ");
        tx.execute_batch(&format!("CREATE TABLE {} ({schema})", identifier(&name)))?;
        {
            let mut appender = tx.appender(&name)?;
            for row in rows {
                let values: Vec<_> = kinds
                    .iter()
                    .map(|(key, kind)| kind.value(row.get(key).unwrap_or(&Value::Null)))
                    .collect();
                appender.append_row(appender_params_from_iter(values))?;
            }
            appender.flush()?;
        }
        let mixed_columns = serde_json::to_string(
            &columns
                .iter()
                .filter(|c| c.mixed)
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
        )?;
        tx.execute(
            "INSERT INTO _spill_datasets (
                dataset_name, source_tool, source_call_id, created_at,
                row_count, original_bytes, original_json, mixed_columns
            ) VALUES (?, ?, ?, make_timestamp(epoch_us(current_timestamp)), ?, ?, ?, ?)",
            params![
                name,
                tool,
                call_id,
                rows.len() as i64,
                original.len() as i64,
                original,
                mixed_columns
            ],
        )?;
        tx.commit()?;
        let created_at = conn.query_row("SELECT strftime(created_at, '%Y-%m-%dT%H:%M:%SZ') FROM _spill_datasets WHERE dataset_name = ?", [&name], |r| r.get(0))?;
        Ok(Dataset {
            dataset: name,
            rows: rows.len() as i64,
            source_tool: tool.into(),
            created_at,
            original_bytes: original.len() as i64,
            columns,
        })
    }

    pub fn list(&self) -> Result<Value> {
        if !self.path.exists() {
            return Ok(json!([]));
        }
        let conn = self.open(true)?;
        let mut statement = conn.prepare("SELECT dataset_name, row_count, source_tool, strftime(created_at, '%Y-%m-%dT%H:%M:%SZ') FROM _spill_datasets ORDER BY created_at DESC, dataset_name")?;
        let mut cursor = statement.query([])?;
        let mut result = Vec::new();
        while let Some(row) = cursor.next()? {
            if result.len() >= MAX_ROWS {
                bail!("Too many datasets. Use query to filter _spill_datasets.");
            }
            result.push(json!({"dataset": row.get::<_, String>(0)?, "rows": row.get::<_, i64>(1)?, "source_tool": row.get::<_, String>(2)?, "created_at": row.get::<_, String>(3)?}));
        }
        bounded(json!(result))
    }

    pub fn describe(&self, name: &str) -> Result<Value> {
        let conn = self.open(true).context("Unknown Spill dataset")?;
        let (rows, source_tool, created_at, original_bytes, mixed_columns): (i64, String, String, i64, String) = conn.query_row(
            "SELECT row_count, source_tool, strftime(created_at, '%Y-%m-%dT%H:%M:%SZ'), original_bytes, COALESCE(mixed_columns, '') FROM _spill_datasets WHERE dataset_name = ?",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).context("Unknown Spill dataset")?;
        // Older databases stored comma-separated names. Continue to read those
        // records while new records encode arbitrary column names as JSON.
        let mixed_set: HashSet<String> = serde_json::from_str::<Vec<String>>(&mixed_columns)
            .unwrap_or_else(|_| {
                mixed_columns
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .into_iter()
            .collect();
        let mut statement = conn.prepare("SELECT column_name, data_type FROM information_schema.columns WHERE table_schema = 'main' AND table_name = ? ORDER BY ordinal_position")?;
        let columns = statement
            .query_map([name], |r| {
                let col_name: String = r.get(0)?;
                let data_type: String = r.get(1)?;
                Ok((col_name, data_type))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?
            .into_iter()
            .map(|(col_name, data_type)| Column {
                mixed: mixed_set.contains(col_name.as_str()),
                name: col_name,
                data_type,
            })
            .collect();
        bounded(serde_json::to_value(Dataset {
            dataset: name.into(),
            rows,
            source_tool,
            created_at,
            original_bytes,
            columns,
        })?)
    }

    pub fn query(&self, sql: &str) -> Result<Value> {
        crate::sql::validate(sql)?;
        if !self.path.exists() {
            self.init()?;
        }
        let conn = self.open(true)?;
        let mut statement = conn.prepare(sql)?;
        let mut cursor = statement.query([])?;
        let mut result = QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
        };
        let mut bytes = 0;
        while let Some(row) = cursor.next()? {
            if result.rows.len() == MAX_ROWS {
                bail!("Query returned too many rows. Add LIMIT or aggregate the result.");
            }
            if result.columns.is_empty() {
                result.columns = row.as_ref().column_names();
            }
            let values = (0..result.columns.len())
                .map(|i| row.get::<_, SqlValue>(i).map(to_json))
                .collect::<duckdb::Result<Vec<_>>>()?;
            bytes += serde_json::to_vec(&values)?.len();
            if bytes > MAX_BYTES {
                bail!("Query returned too many bytes. Select fewer columns, add LIMIT, or aggregate the result.");
            }
            result.rows.push(values);
        }
        drop(cursor);
        if result.columns.is_empty() {
            result.columns = statement.column_names();
        }
        bounded(serde_json::to_value(result)?)
    }
}

fn to_json(v: SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::Null,
        SqlValue::Boolean(v) => json!(v),
        SqlValue::TinyInt(v) => json!(v),
        SqlValue::SmallInt(v) => json!(v),
        SqlValue::Int(v) => json!(v),
        SqlValue::BigInt(v) => json!(v),
        SqlValue::UTinyInt(v) => json!(v),
        SqlValue::USmallInt(v) => json!(v),
        SqlValue::UInt(v) => json!(v),
        SqlValue::UBigInt(v) => json!(v),
        SqlValue::HugeInt(v) => json!(v),
        SqlValue::Float(v) => json!(v),
        SqlValue::Double(v) => json!(v),
        SqlValue::Text(v) => json!(v),
        SqlValue::List(values) => Value::Array(values.into_iter().map(to_json).collect()),
        other => json!(format!("{other:?}")),
    }
}

pub fn bounded(value: Value) -> Result<Value> {
    if serde_json::to_vec(&value)?.len() > MAX_BYTES {
        bail!("Result exceeds 32 KiB. Use query to select fewer columns or aggregate the result.");
    }
    Ok(value)
}
