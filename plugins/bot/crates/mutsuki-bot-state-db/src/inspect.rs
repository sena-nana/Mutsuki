//! Read-only catalog inspection: user tables, columns, row counts and rowid-paged rows.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, params};

use crate::codec::sqlite_unsigned;
use crate::error::BotStateDbError;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseSnapshot {
    pub path: String,
    pub journal_mode: String,
    pub size_bytes: Option<u64>,
    pub tables: Vec<BotDatabaseTableInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseTableInfo {
    pub name: String,
    pub row_count: u64,
    pub columns: Vec<BotDatabaseColumnInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseColumnInfo {
    pub name: String,
    pub decl_type: String,
    pub primary_key: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BotDatabaseTablePage {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub next_cursor: Option<String>,
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn user_table_names(connection: &Connection) -> Result<Vec<String>, BotStateDbError> {
    let mut statement = connection.prepare_cached(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(BotStateDbError::from)
}

fn inspect_columns(
    connection: &Connection,
    table: &str,
) -> Result<Vec<BotDatabaseColumnInfo>, BotStateDbError> {
    let mut statement =
        connection.prepare(&format!("PRAGMA table_info({})", quote_ident(table)))?;
    statement
        .query_map([], |row| {
            Ok(BotDatabaseColumnInfo {
                name: row.get(1)?,
                decl_type: row.get::<_, String>(2).unwrap_or_default(),
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(BotStateDbError::from)
}

fn inspect_row_count(connection: &Connection, table: &str) -> Result<u64, BotStateDbError> {
    let count: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM {}", quote_ident(table)),
        [],
        |row| row.get(0),
    )?;
    sqlite_unsigned(count, "row_count")
}

pub(crate) fn inspect_snapshot_tables(
    connection: &Connection,
) -> Result<Vec<BotDatabaseTableInfo>, BotStateDbError> {
    user_table_names(connection)?
        .into_iter()
        .map(|name| {
            Ok(BotDatabaseTableInfo {
                row_count: inspect_row_count(connection, &name)?,
                columns: inspect_columns(connection, &name)?,
                name,
            })
        })
        .collect()
}

fn inspect_value(value: ValueRef<'_>) -> serde_json::Value {
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(value) => serde_json::Value::from(value),
        ValueRef::Real(value) => serde_json::json!(value),
        ValueRef::Text(value) => {
            serde_json::Value::String(String::from_utf8_lossy(value).into_owned())
        }
        ValueRef::Blob(value) => serde_json::json!({
            "$type": "blob",
            "bytes": value.len(),
        }),
    }
}

pub(crate) fn inspect_rows(
    connection: &Connection,
    table: &str,
    after: &str,
    limit: u32,
) -> Result<BotDatabaseTablePage, BotStateDbError> {
    if !user_table_names(connection)?
        .iter()
        .any(|name| name == table)
    {
        return Err(BotStateDbError::InvalidConfiguration(format!(
            "unknown table: {table}"
        )));
    }
    let columns = inspect_columns(connection, table)?;
    let column_names = columns
        .into_iter()
        .map(|column| column.name)
        .collect::<Vec<_>>();
    let select_list = column_names
        .iter()
        .map(|name| quote_ident(name))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT rowid, {select_list} FROM {} WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        quote_ident(table)
    );
    let after_rowid = if after.is_empty() {
        0
    } else {
        after.parse::<i64>().map_err(|_| {
            BotStateDbError::InvalidConfiguration(format!("invalid cursor: {after}"))
        })?
    };
    let mut statement = connection.prepare(&sql)?;
    let mut query = statement.query(params![after_rowid, i64::from(limit)])?;
    let mut rows = Vec::new();
    let mut last_rowid = None;
    while let Some(row) = query.next()? {
        last_rowid = Some(row.get(0)?);
        let mut values = Vec::with_capacity(column_names.len());
        for index in 0..column_names.len() {
            values.push(inspect_value(row.get_ref(index + 1)?));
        }
        rows.push(values);
    }
    let next_cursor = if rows.len() == usize::try_from(limit).unwrap_or(usize::MAX) {
        last_rowid.map(|rowid: i64| rowid.to_string())
    } else {
        None
    };
    Ok(BotDatabaseTablePage {
        table: table.to_owned(),
        columns: column_names,
        rows,
        next_cursor,
    })
}
