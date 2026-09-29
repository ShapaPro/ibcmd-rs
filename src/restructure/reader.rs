//! The database side of the plan's input: `SchemaStorage`, the `DBNames` rows, the file lists and the
//! descriptor rows of `Config` and `ConfigSave`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};

use crate::restructure::plan::{Inputs, StagedImage};
use crate::restructure::storage::SchemaStorageRow;
use crate::sql::SqlRow;
use crate::sql::mssql::{TdsConnection, sql_row};

fn rows(
    connection: &mut TdsConnection,
    query: &str,
    mut each: impl FnMut(SqlRow) -> Result<()>,
) -> Result<()> {
    connection.query_each(query, &[], |row| each(sql_row(row)))
}

/// Reads everything a plan needs, and the `SchemaStorage` rows.
pub fn read_inputs(connection: &mut TdsConnection) -> Result<(Inputs, Vec<SchemaStorageRow>)> {
    let mut storage = Vec::new();
    rows(
        connection,
        "SELECT SchemaID, Status, CurrentSchema, NewGenCreated, NewGenDropped FROM dbo.SchemaStorage ORDER BY SchemaID",
        |mut row| {
            storage.push(SchemaStorageRow {
                schema_id: row.i64(0)?,
                status: row.i64(1)?,
                current_schema: row.take_binary(2)?,
                new_gen_created: row.take_binary(3)?,
                new_gen_dropped: row.take_binary(4)?,
            });
            Ok(())
        },
    )
    .context("SchemaStorage")?;
    let main = storage
        .iter()
        .find(|row| row.schema_id == 0)
        .context("SchemaStorage has no row for the main configuration")?;

    let mut inputs = Inputs {
        schema: main.current_schema.clone(),
        ..Inputs::default()
    };
    let mut have_main_names = false;
    rows(
        connection,
        "SELECT FileName, BinaryData FROM dbo.Params WHERE PartNo = 0 AND (FileName = N'DBNames' OR FileName LIKE N'DBNames-Ext-%')",
        |mut row| {
            let name = row.take_text(0)?;
            let data = row.take_binary(1)?;
            if name == "DBNames" {
                inputs.main_names = data;
                have_main_names = true;
            } else {
                inputs.extension_names.push((name, data));
            }
            Ok(())
        },
    )
    .context("Params DBNames")?;
    if !have_main_names {
        bail!("Params has no DBNames row");
    }
    rows(
        connection,
        "SELECT t.name FROM sys.tables t JOIN sys.partitions p ON p.object_id = t.object_id AND p.index_id IN (0, 1)          WHERE t.name LIKE N'[_]RefSInf%' GROUP BY t.name HAVING SUM(p.rows) > 0",
        |mut row| {
            let name = row.take_text(0)?;
            inputs
                .predefined_tables
                .insert(name.trim_start_matches('_').to_owned());
            Ok(())
        },
    )
    .context("RefSInf tables")?;
    rows(
        connection,
        "SELECT FileName, BinaryData FROM dbo.Params WHERE PartNo = 0 AND FileName LIKE N'%.si'",
        |mut row| {
            let name = row.take_text(0)?;
            inputs.cache_rows.push((name, row.take_binary(1)?));
            Ok(())
        },
    )
    .context("Params *.si")?;
    inputs.staged = read_staged(connection)?;
    Ok((inputs, storage))
}

fn read_staged(connection: &mut TdsConnection) -> Result<StagedImage> {
    let mut image = StagedImage::default();
    for (table, files, descriptors) in [
        ("Config", &mut image.old_files, &mut image.old_descriptors),
        (
            "ConfigSave",
            &mut image.new_files,
            &mut image.new_descriptors,
        ),
    ] {
        collect_files(connection, table, files)?;
        collect_descriptors(connection, table, descriptors)?;
    }
    rows(
        connection,
        "SELECT BinaryData FROM dbo.ConfigSave WHERE FileName = N'deleted' AND PartNo = 0",
        |mut row| {
            image.deleted = Some(row.take_binary(0)?);
            Ok(())
        },
    )
    .context("ConfigSave deleted")?;
    Ok(image)
}

fn collect_files(
    connection: &mut TdsConnection,
    table: &str,
    files: &mut BTreeSet<String>,
) -> Result<()> {
    rows(
        connection,
        &format!("SELECT FileName FROM dbo.{table}"),
        |mut row| {
            files.insert(row.take_text(0)?);
            Ok(())
        },
    )
    .with_context(|| format!("{table} file names"))
}

/// The rows named by a bare uuid: the object descriptors.
fn collect_descriptors(
    connection: &mut TdsConnection,
    table: &str,
    descriptors: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    rows(
        connection,
        &format!(
            "SELECT FileName, BinaryData FROM dbo.{table} WHERE PartNo = 0 AND LEN(FileName) = 36 AND FileName NOT LIKE N'%.%'"
        ),
        |mut row| {
            let name = row.take_text(0)?;
            descriptors.insert(name, row.take_binary(1)?);
            Ok(())
        },
    )
    .with_context(|| format!("{table} descriptors"))
}

/// Other user sessions in the current database (an exclusive restructure wants none).
pub fn other_sessions(connection: &mut TdsConnection) -> Result<i64> {
    let mut count = 0;
    rows(
        connection,
        "SELECT COUNT(*) FROM sys.dm_exec_sessions WHERE database_id = DB_ID() AND session_id <> @@SPID AND is_user_process = 1",
        |row| {
            count = row.i64(0)?;
            Ok(())
        },
    )
    .context("cannot count the sessions of the database (VIEW SERVER STATE?)")?;
    Ok(count)
}

/// The current database's name.
pub fn database_name(connection: &mut TdsConnection) -> Result<String> {
    let mut name = String::new();
    rows(connection, "SELECT DB_NAME()", |mut row| {
        name = row.take_text(0)?;
        Ok(())
    })?;
    Ok(name)
}
