//! The recovery artifact: what the apply overwrites, saved before it runs.
//!
//! The apply is one transaction, so a failure leaves the database untouched;
//! the artifact is for taking a *successful* apply back. It holds, in a
//! directory:
//!
//! - `config_replaced.tsv`: one line per `Config` row the staged rows replace
//!   (every part), with its hash and, when its bytes were kept, the file;
//! - `rows/<n>.bin`: the bytes of the rows the apply changes (or of none, with
//!   [`RecoveryBlobs::None`]);
//! - `special_rows.tsv` and their bytes: the dynamic-update markers and alias
//!   rows the apply folds away;
//! - `files_before.tsv` and `change_registrations_before.tsv`: the
//!   `MobileVersions.dat` head and the `_MessageNo` values it resets;
//! - `manifest.json` and `README.txt`.
//!
//! Taking an apply back is: put the kept rows back into `Config` (delete the
//! names in `config_replaced.tsv`, insert the rows), restore the special rows,
//! `MobileVersions.dat` and the `_MessageNo` values, and stage nothing.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::sql::{SqlClient, SqlValue};

use super::RecoveryBlobs;
use super::model::{RowMeta, quote_ident, quote_string};

pub struct RecoveryRequest<'a> {
    pub database: &'a str,
    pub dir: &'a Path,
    pub token: &'a str,
    pub blobs: RecoveryBlobs,
    pub staged: &'a [RowMeta],
    pub replaced: &'a [RowMeta],
    pub mobile_versions_before: Option<&'a [u8]>,
    pub reset_change_registrations: bool,
}

#[derive(Debug, Serialize)]
struct Manifest<'a> {
    schema_version: u32,
    database: &'a str,
    token: &'a str,
    created_unix_seconds: u64,
    staged_rows: usize,
    replaced_rows: usize,
    replaced_rows_saved: usize,
    saved_bytes: u64,
    special_rows: usize,
    change_registrations_reset: usize,
    blobs: &'a str,
}

fn tsv(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

pub fn write_recovery(client: &dyn SqlClient, request: &RecoveryRequest<'_>) -> Result<()> {
    let db = quote_ident(request.database)?;
    fs::create_dir_all(request.dir.join("rows"))
        .with_context(|| format!("failed to create {}", request.dir.display()))?;
    let mut saved: std::collections::HashMap<(String, i32), String> =
        std::collections::HashMap::new();
    let mut saved_bytes = 0u64;
    let mut sequence = 0usize;

    // Bytes of the rows the apply changes (the server compares, so unchanged
    // rows never travel).
    if request.blobs == RecoveryBlobs::Changed {
        let query = format!(
            "SELECT c.FileName, c.PartNo, c.BinaryData FROM {db}.dbo.Config c WHERE EXISTS (SELECT 1 FROM {db}.dbo.ConfigSave s WHERE s.FileName = c.FileName) \
             AND NOT EXISTS (SELECT 1 FROM {db}.dbo.ConfigSave s WHERE s.FileName = c.FileName AND s.PartNo = c.PartNo AND s.DataSize = c.DataSize AND s.BinaryData = c.BinaryData) ORDER BY c.FileName, c.PartNo"
        );
        client.read_rows(&query, &[], &mut |mut row| {
            let name = row.take_text(0)?;
            let part = i32::try_from(row.i64(1)?)?;
            let bytes = row.take_binary(2)?;
            sequence += 1;
            let file = format!("rows/{sequence:06}.bin");
            fs::write(request.dir.join(&file), &bytes)
                .with_context(|| format!("failed to write {file}"))?;
            saved_bytes += bytes.len() as u64;
            saved.insert((name.to_lowercase(), part), file);
            Ok(())
        })?;
    }

    let mut replaced_file = BufWriter::new(
        fs::File::create(request.dir.join("config_replaced.tsv")).context("config_replaced.tsv")?,
    );
    writeln!(
        replaced_file,
        "name\tpart\tdata_size\tbyte_len\tattributes\tcreation\tmodified\tsha256\tfile"
    )?;
    for row in request.replaced {
        writeln!(
            replaced_file,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            tsv(&row.name),
            row.part,
            row.data_size,
            row.byte_len,
            row.attributes,
            row.creation,
            row.modified,
            row.sha256,
            saved.get(&row.key()).map_or("-", String::as_str)
        )?;
    }
    replaced_file.flush()?;

    // The dynamic-update leftovers, always with their bytes (they are small).
    let mut special_file = BufWriter::new(
        fs::File::create(request.dir.join("special_rows.tsv")).context("special_rows.tsv")?,
    );
    writeln!(
        special_file,
        "table\tname\tpart\tattributes\tcreation\tmodified\tfile"
    )?;
    let mut special_count = 0usize;
    for (table, filter) in [
        (
            "Config",
            "FileName = N'DynamicallyUpdated' OR FileName LIKE N'%\\_dynupdate\\_%' ESCAPE N'\\'"
                .to_owned(),
        ),
        ("Params", "FileName = N'DynamicallyUpdated'".to_owned()),
    ] {
        let query = format!(
            "SELECT FileName, PartNo, CONVERT(int, Attributes), CONVERT(varchar(27), Creation, 121), CONVERT(varchar(27), Modified, 121), BinaryData FROM {db}.dbo.{table} WHERE {filter} ORDER BY FileName, PartNo"
        );
        client.read_rows(&query, &[], &mut |mut row| {
            let name = row.take_text(0)?;
            let part = row.i64(1)?;
            let attributes = row.i64(2)?;
            let creation = row.text(3)?.to_owned();
            let modified = row.text(4)?.to_owned();
            let bytes = row.take_binary(5)?;
            sequence += 1;
            let file = format!("rows/{sequence:06}.bin");
            fs::write(request.dir.join(&file), &bytes)?;
            saved_bytes += bytes.len() as u64;
            special_count += 1;
            writeln!(
                special_file,
                "{table}\t{}\t{part}\t{attributes}\t{creation}\t{modified}\t{file}",
                tsv(&name)
            )?;
            Ok(())
        })?;
    }
    special_file.flush()?;

    if let Some(bytes) = request.mobile_versions_before {
        fs::write(request.dir.join("MobileVersions.dat.before"), bytes)?;
        saved_bytes += bytes.len() as u64;
    }

    // The `_MessageNo` values the reset overwrites.
    let mut reset_count = 0usize;
    if request.reset_change_registrations {
        let mut file = BufWriter::new(
            fs::File::create(request.dir.join("change_registrations_before.tsv"))
                .context("change_registrations_before.tsv")?,
        );
        writeln!(file, "node_type_ref\tnode_ref\tobject_id\tmessage_no")?;
        let query = format!(
            "SELECT CONVERT(varchar(16), _NodeTRef, 2), CONVERT(varchar(64), _NodeRRef, 2), CONVERT(varchar(64), _MDObjID, 2), CONVERT(bigint, _MessageNo) FROM {db}.dbo._ConfigChngR \
             WHERE _MessageNo IS NOT NULL AND _MDObjID IN (SELECT CAST(TRY_CAST(FileName AS uniqueidentifier) AS binary(16)) FROM {db}.dbo.ConfigSave WHERE PartNo = 0 AND LEN(FileName) = 36 AND TRY_CAST(FileName AS uniqueidentifier) IS NOT NULL) ORDER BY 1, 2, 3"
        );
        client.read_rows(&query, &[], &mut |row| {
            reset_count += 1;
            let message = match row.value(3)? {
                SqlValue::Int(value) => value.to_string(),
                other => other.to_text(),
            };
            writeln!(
                file,
                "{}\t{}\t{}\t{}",
                row.text(0)?,
                row.text(1)?,
                row.text(2)?,
                message
            )?;
            Ok(())
        })?;
        file.flush()?;
    }

    let manifest = Manifest {
        schema_version: 1,
        database: request.database,
        token: request.token,
        created_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
        staged_rows: request.staged.len(),
        replaced_rows: request.replaced.len(),
        replaced_rows_saved: saved.len(),
        saved_bytes,
        special_rows: special_count,
        change_registrations_reset: reset_count,
        blobs: match request.blobs {
            RecoveryBlobs::Changed => "changed",
            RecoveryBlobs::None => "none",
        },
    };
    fs::write(
        request.dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(
        request.dir.join("README.txt"),
        format!(
            "ibcmd-rs config apply recovery artifact for database {db}\n\
             token {token}\n\n\
             The apply ran as one transaction: a failed run changes nothing. This directory\n\
             lets a successful run be taken back.\n\n\
             config_replaced.tsv   every Config row (all parts) the staged rows replaced; `file` is its saved bytes\n\
             special_rows.tsv      the DynamicallyUpdated markers and _dynupdate_ alias rows that were folded away\n\
             MobileVersions.dat.before   Files.MobileVersions.dat before the new head GUID\n\
             change_registrations_before.tsv   _ConfigChngR rows whose _MessageNo was reset to NULL\n\n\
             To take the apply back: stage the saved rows in ConfigSave (name, part, sizes, attributes,\n\
             creation, modified, bytes), then run `ibcmd-rs mssql-config-apply` (or the native apply)\n\
             on the stopped database; restore special_rows.tsv, MobileVersions.dat and the message\n\
             numbers with plain INSERT/UPDATE statements. The `sha256` column proves each row.\n\
             Names are quoted: {quoted}\n",
            db = request.database,
            token = request.token,
            quoted = quote_string(request.database)
        ),
    )?;
    Ok(())
}
