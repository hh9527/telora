use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::types::{Value as SqlValue, ValueRef as SqlValueRef};
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub columns: usize,
    pub rows: usize,
    pub payload_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            columns: 256,
            rows: 10_000,
            payload_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub sql: String,
    pub bindings: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

pub struct Database {
    connection: Connection,
    limits: Limits,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limits(path, Limits::default())
    }

    pub fn open_with_limits(path: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        let path = path.as_ref();
        if limits.columns == 0 || limits.rows == 0 || limits.payload_bytes == 0 {
            bail!("SQLite query limits must be greater than zero");
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("cannot open SQLite database {} read-only", path.display()))?;
        Ok(Self { connection, limits })
    }

    pub fn query(&mut self, request: Query) -> Result<QueryOutput> {
        if request.sql.trim().is_empty() {
            bail!("SQLite query must not be empty");
        }
        let bindings = request
            .bindings
            .iter()
            .enumerate()
            .map(|(index, value)| binding(value, index))
            .collect::<Result<Vec<_>>>()?;
        let mut statement = self
            .connection
            .prepare(&request.sql)
            .context("cannot prepare SQLite query")?;
        if !statement.readonly() {
            bail!("SQLite query must contain one read-only statement");
        }

        let column_count = statement.column_count();
        if column_count > self.limits.columns {
            bail!(
                "SQLite query returned {column_count} columns; limit is {}",
                self.limits.columns
            );
        }
        let columns = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut payload_bytes = columns.iter().map(String::len).sum::<usize>();
        check_payload(payload_bytes, self.limits.payload_bytes)?;

        let mut cursor = statement
            .query(params_from_iter(bindings.iter()))
            .context("cannot execute SQLite query")?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().context("cannot read SQLite query row")? {
            if rows.len() == self.limits.rows {
                bail!("SQLite query returned more than {} rows", self.limits.rows);
            }
            let mut values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                let (value, bytes) = cell(
                    row.get_ref(index)
                        .with_context(|| format!("cannot read SQLite column {index}"))?,
                    index,
                )?;
                payload_bytes = payload_bytes
                    .checked_add(bytes)
                    .ok_or_else(|| anyhow::anyhow!("SQLite query result size overflow"))?;
                check_payload(payload_bytes, self.limits.payload_bytes)?;
                values.push(value);
            }
            rows.push(values);
        }
        Ok(QueryOutput { columns, rows })
    }
}

fn binding(value: &Value, index: usize) -> Result<SqlValue> {
    Ok(match value {
        Value::Null => SqlValue::Null,
        Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                SqlValue::Integer(value)
            } else if let Some(value) = value.as_u64() {
                SqlValue::Integer(i64::try_from(value).map_err(|_| {
                    anyhow::anyhow!("SQLite binding {index} is outside the Int range")
                })?)
            } else {
                let value = value.as_f64().ok_or_else(|| {
                    anyhow::anyhow!("SQLite binding {index} is not a finite number")
                })?;
                if !value.is_finite() {
                    bail!("SQLite binding {index} is not a finite number");
                }
                SqlValue::Real(value)
            }
        }
        Value::String(value) => SqlValue::Text(value.clone()),
        Value::Array(_) | Value::Object(_) => {
            bail!("SQLite binding {index} must be a JSON scalar")
        }
    })
}

fn cell(value: SqlValueRef<'_>, index: usize) -> Result<(Value, usize)> {
    Ok(match value {
        SqlValueRef::Null => (Value::Null, 0),
        SqlValueRef::Integer(value) => (Value::from(value), std::mem::size_of::<i64>()),
        SqlValueRef::Real(value) => {
            let number = serde_json::Number::from_f64(value).ok_or_else(|| {
                anyhow::anyhow!("SQLite column {index} contains a non-finite Float")
            })?;
            (Value::Number(number), std::mem::size_of::<f64>())
        }
        SqlValueRef::Text(value) => {
            let value = std::str::from_utf8(value)
                .with_context(|| format!("SQLite column {index} is not UTF-8 text"))?;
            (Value::String(value.to_owned()), value.len())
        }
        SqlValueRef::Blob(_) => bail!("SQLite column {index} contains an unsupported Blob"),
    })
}

fn check_payload(actual: usize, limit: usize) -> Result<()> {
    if actual > limit {
        bail!("SQLite query result exceeds the {limit}-byte payload limit");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("data.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE item(id INTEGER, name TEXT, score REAL, enabled INTEGER);\n\
                 INSERT INTO item VALUES (1, 'Ada', 1.5, 1), (2, 'Lin', 2.5, 0);",
            )
            .unwrap();
        drop(connection);
        (root, path)
    }

    #[test]
    fn binds_json_scalars_without_sql_interpolation() {
        let (_root, path) = database();
        let mut database = Database::open(path).unwrap();
        let output = database
            .query(Query {
                sql: "SELECT name, score, NULL, ?3 FROM item WHERE id >= ?1 AND enabled = ?2 ORDER BY id".into(),
                bindings: vec![json!(1), json!(true), json!("bound")],
            })
            .unwrap();
        assert_eq!(output.columns, ["name", "score", "NULL", "?3"]);
        assert_eq!(
            output.rows,
            [vec![json!("Ada"), json!(1.5), Value::Null, json!("bound")]]
        );
    }

    #[test]
    fn rejects_writes_multiple_statements_and_structured_bindings() {
        let (_root, path) = database();
        let mut database = Database::open(path).unwrap();
        for sql in ["UPDATE item SET name = 'changed'", "SELECT 1; SELECT 2"] {
            assert!(
                database
                    .query(Query {
                        sql: sql.into(),
                        bindings: vec![]
                    })
                    .is_err()
            );
        }
        let error = database
            .query(Query {
                sql: "SELECT ?1".into(),
                bindings: vec![json!({"not": "scalar"})],
            })
            .unwrap_err();
        assert!(error.to_string().contains("must be a JSON scalar"));
    }

    #[test]
    fn rejects_blobs_and_complete_results_over_limits() {
        let (_root, path) = database();
        let mut database = Database::open(&path).unwrap();
        let error = database
            .query(Query {
                sql: "SELECT x'00'".into(),
                bindings: vec![],
            })
            .unwrap_err();
        assert!(error.to_string().contains("unsupported Blob"));

        let mut database = Database::open_with_limits(
            path,
            Limits {
                columns: 1,
                rows: 1,
                payload_bytes: 1024,
            },
        )
        .unwrap();
        let error = database
            .query(Query {
                sql: "SELECT id FROM item ORDER BY id".into(),
                bindings: vec![],
            })
            .unwrap_err();
        assert!(error.to_string().contains("more than 1 rows"));
    }

    #[test]
    fn open_is_read_only_and_does_not_create_a_database() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing.sqlite");
        assert!(Database::open(&missing).is_err());
        assert!(!missing.exists());

        let (_root, path) = database();
        let mut database = Database::open(&path).unwrap();
        assert!(
            database
                .query(Query {
                    sql: "CREATE TABLE forbidden(value INTEGER)".into(),
                    bindings: vec![],
                })
                .is_err()
        );
        let connection = Connection::open(path).unwrap();
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'forbidden'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
