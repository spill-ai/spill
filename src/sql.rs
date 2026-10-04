use anyhow::{bail, Result};
use sqlparser::{
    dialect::DuckDbDialect,
    tokenizer::{Token, Tokenizer},
};

/// This filter is policy, not the security boundary. DuckDB also runs in
/// READ_ONLY mode with external access and extension loading disabled.
pub fn validate(sql: &str) -> Result<()> {
    let tokens = Tokenizer::new(&DuckDbDialect {}, sql).tokenize()?;
    let tokens: Vec<_> = tokens
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let first = tokens
        .iter()
        .find(|t| !matches!(t, Token::LParen))
        .and_then(|t| match t {
            Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
            _ => None,
        });
    let Some(first) = first else {
        bail!("Expected SELECT, WITH, SHOW, DESCRIBE, or EXPLAIN");
    };
    if !["SELECT", "WITH", "SHOW", "DESCRIBE", "EXPLAIN"].contains(&first.as_str()) {
        bail!("Only SELECT, WITH, SHOW, DESCRIBE, and EXPLAIN are allowed");
    }
    for (i, token) in tokens.iter().enumerate() {
        if matches!(token, Token::SemiColon) && i + 1 != tokens.len() {
            bail!("Only one SQL statement is allowed");
        }
        if let Token::Word(word) = token {
            if word.quote_style.is_none() {
                let upper = word.value.to_ascii_uppercase();
                let prev = if i > 0 { tokens.get(i - 1) } else { None };
                let is_alias_or_field = prev.is_some_and(|p| match p {
                    Token::Period => true,
                    Token::Word(w) if w.quote_style.is_none() => w.value.eq_ignore_ascii_case("AS"),
                    _ => false,
                });
                if is_alias_or_field {
                    continue;
                }
                let next_is_paren = tokens
                    .get(i + 1)
                    .is_some_and(|next| matches!(next, Token::LParen));
                // A scalar function call like `replace(...)` is read-only in DuckDB.
                if !next_is_paren && upper == "REPLACE" {
                    bail!("Mutating SQL is not allowed");
                }
                let is_stmt_start = prev.is_none()
                    || prev.is_some_and(|p| match p {
                        Token::SemiColon | Token::LParen => true,
                        Token::Word(w) if w.quote_style.is_none() => {
                            let v = w.value.to_ascii_uppercase();
                            v == "EXPLAIN" || v == "ANALYZE"
                        }
                        _ => false,
                    });
                let next_word = tokens.get(i + 1).and_then(|t| match t {
                    Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
                    _ => None,
                });
                let is_command = match upper.as_str() {
                    "INSERT" => is_stmt_start || next_word.as_deref() == Some("INTO"),
                    "DELETE" => is_stmt_start && next_word.as_deref() == Some("FROM"),
                    "UPDATE" => {
                        let is_after_update_clause = prev.is_some_and(|p| match p {
                            Token::Word(w) if w.quote_style.is_none() => {
                                let v = w.value.to_ascii_uppercase();
                                v == "UPDATE" || v == "TABLE"
                            }
                            _ => false,
                        });
                        is_stmt_start && !is_after_update_clause
                    }
                    "DROP" => {
                        is_stmt_start
                            && matches!(
                                next_word.as_deref(),
                                Some(
                                    "TABLE"
                                        | "VIEW"
                                        | "SCHEMA"
                                        | "MACRO"
                                        | "INDEX"
                                        | "SEQUENCE"
                                        | "TYPE"
                                        | "DATABASE"
                                )
                            )
                    }
                    "ALTER" => {
                        is_stmt_start
                            && matches!(next_word.as_deref(), Some("TABLE" | "VIEW" | "SEQUENCE"))
                    }
                    "CREATE" => {
                        is_stmt_start
                            && matches!(
                                next_word.as_deref(),
                                Some(
                                    "TABLE"
                                        | "VIEW"
                                        | "SCHEMA"
                                        | "INDEX"
                                        | "MACRO"
                                        | "FUNCTION"
                                        | "SEQUENCE"
                                        | "TYPE"
                                        | "TEMP"
                                        | "TEMPORARY"
                                        | "OR"
                                        | "UNIQUE"
                                )
                            )
                    }
                    "TRUNCATE" => is_stmt_start,
                    "MERGE" => is_stmt_start && next_word.as_deref() == Some("INTO"),
                    "ATTACH" | "DETACH" | "INSTALL" | "LOAD" | "PRAGMA" | "RESET" | "EXPORT"
                    | "IMPORT" | "VACUUM" | "CHECKPOINT" => is_stmt_start,
                    "COPY" => is_stmt_start && next_word.is_some(),
                    "SET" | "CALL" => {
                        let is_update_set = prev.is_some_and(|p| match p {
                            Token::Word(w) => {
                                let val = w.value.to_ascii_uppercase();
                                val == "UPDATE" || val == "TABLE"
                            }
                            _ => false,
                        });
                        is_stmt_start || is_update_set
                    }
                    _ => false,
                };
                if is_command {
                    bail!("Mutating SQL is not allowed");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restrictions() {
        for sql in [
            "SELECT * FROM foo",
            "(SELECT * FROM foo)",
            "((SELECT 1))",
            "SELECT replace('hello world', 'world', 'there') AS greeting",
            "SELECT id, call, set FROM foo",
            "with x as (select 1) select * from x;",
            "SHOW TABLES",
            "DESCRIBE foo",
            "EXPLAIN SELECT 1",
            "-- hello\n SELECT 'DROP; DELETE' AS \"UPDATE\";",
            "SELECT id, load, copy, export, import FROM foo",
            "SELECT count(*) AS copy FROM foo",
            "SELECT t.delete, t.create FROM foo t",
            "SELECT * FROM foo ORDER BY load DESC",
            // Reserved keywords used as bare column names in WHERE / GROUP BY / HAVING
            // must not be rejected — they are column references, not commands.
            "SELECT * FROM foo WHERE update > 0",
            "SELECT * FROM foo WHERE delete IS NOT NULL",
            "SELECT * FROM foo WHERE insert = 'x'",
            "SELECT insert, update FROM foo WHERE drop > 1",
            "SELECT load, COUNT(*) FROM foo GROUP BY load",
            "SELECT copy, COUNT(*) FROM foo GROUP BY copy HAVING COUNT(*) > 1",
            "SELECT * FROM foo WHERE load = 1 AND copy = 2",
        ] {
            assert!(validate(sql).is_ok(), "{sql}");
        }
        for sql in [
            "DROP TABLE foo",
            "DELETE FROM foo",
            "select 1; drop table foo",
            "WITH x AS (DELETE FROM foo RETURNING *) SELECT * FROM x",
            "EXPLAIN ANALYZE INSERT INTO foo VALUES (1)",
            "SELECT 1; SELECT 2",
            "REPLACE INTO foo VALUES (1)",
            "SELECT 1; SET x = 1",
            "CALL my_procedure()",
            "",
            "SELECT 'unclosed",
            "select 1;;",
        ] {
            assert!(validate(sql).is_err(), "{sql}");
        }
    }
}
