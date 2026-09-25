//! `mssql-dump-config --rows-dir`: the Config table read from a folder of
//! stored rows instead of SQL Server.
//!
//! The folder holds one `<FileName>__part<N>.bin` file per stored part, the
//! BinaryData column exactly as the table keeps it (raw deflate for most
//! rows) -- the format of `F:\ibcmd\lab\rawrows\*\Config` and of the row sets
//! `audit-empty-stage --rows-out` writes. While a folder is active every
//! Config read of the export is answered from it; any other query the export
//! would send to the server is refused, so an offline run never touches SQL.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, anyhow, bail};

use super::config_rows::{BinaryConfigRow, ConfigRowHeader};
use super::dynamic_generation::{is_dynamic_generation_alias, storage_generation_overlay_for};

/// One stored part.
#[derive(Debug, Clone)]
struct StoredPart {
    path: PathBuf,
    bytes: u64,
}

/// The rows of one folder, by stored file name and part number.
#[derive(Debug)]
pub(super) struct OfflineRows {
    rows: BTreeMap<String, BTreeMap<i32, StoredPart>>,
}

static ACTIVE: RwLock<Option<Arc<OfflineRows>>> = RwLock::new(None);

/// Keeps a folder active; the export reads SQL again once it is dropped.
pub(super) struct OfflineRowsGuard {
    _private: (),
}

impl Drop for OfflineRowsGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.write() {
            *active = None;
        }
    }
}

/// Makes `dir` the Config table of this process until the guard drops.
pub(super) fn activate(dir: &Path) -> Result<OfflineRowsGuard> {
    let rows = OfflineRows::load(dir)?;
    let mut active = ACTIVE
        .write()
        .map_err(|_| anyhow!("offline rows lock is poisoned"))?;
    *active = Some(Arc::new(rows));
    Ok(OfflineRowsGuard { _private: () })
}

/// The active folder, if an offline export is running.
pub(super) fn active() -> Option<Arc<OfflineRows>> {
    ACTIVE.read().ok()?.clone()
}

/// The error a query without an offline answer gets.
pub(super) fn refuse(what: &str) -> anyhow::Error {
    anyhow!(
        "offline export (--rows-dir) reads only the Config table from its folder; \
         it has no answer for {what}"
    )
}

/// `<FileName>__part<N>.bin` -> (FileName, N).
fn parse_part_file_name(name: &str) -> Option<(String, i32)> {
    let stem = name.strip_suffix(".bin")?;
    let (file_name, part) = stem.rsplit_once("__part")?;
    if file_name.is_empty() || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((file_name.to_string(), part.parse().ok()?))
}

impl OfflineRows {
    fn load(dir: &Path) -> Result<Self> {
        let mut rows = BTreeMap::<String, BTreeMap<i32, StoredPart>>::new();
        let entries =
            fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some((file_name, part_no)) = name.to_str().and_then(parse_part_file_name) else {
                continue;
            };
            let bytes = entry.metadata()?.len();
            rows.entry(file_name).or_default().insert(
                part_no,
                StoredPart {
                    path: entry.path(),
                    bytes,
                },
            );
        }
        if rows.is_empty() {
            bail!(
                "{} holds no <FileName>__part<N>.bin rows",
                dir.display()
            );
        }
        for (file_name, parts) in &rows {
            for (expected, part_no) in parts.keys().enumerate() {
                if *part_no != expected as i32 {
                    bail!(
                        "{}: row {file_name} has part {part_no} where part {expected} was expected",
                        dir.display()
                    );
                }
            }
        }
        Ok(Self { rows })
    }

    fn check_table(&self, table: &str) -> Result<()> {
        if table.trim_matches(['[', ']']).eq_ignore_ascii_case("Config") {
            Ok(())
        } else {
            Err(refuse(&format!("the {table} table")))
        }
    }

    /// The rows the table publishes, by published name: the stored names,
    /// or -- when an active dynamic generation's overlay is installed -- its
    /// aliases under their published names, without the rows they replace
    /// and without any other generation's aliases, as the SQL view reads it.
    fn published(&self, table: &str) -> BTreeMap<String, &BTreeMap<i32, StoredPart>> {
        let overlay = storage_generation_overlay_for(table);
        let mut out = BTreeMap::new();
        for (file_name, parts) in &self.rows {
            match &overlay {
                None => {
                    out.insert(file_name.clone(), parts);
                }
                Some(overlay) => {
                    if let Some(published) = overlay.published_name(file_name) {
                        out.insert(published.to_string(), parts);
                    } else if !is_dynamic_generation_alias(file_name)
                        && !overlay.hides(file_name)
                    {
                        out.insert(file_name.clone(), parts);
                    }
                }
            }
        }
        out
    }

    /// `SELECT FileName, PartNo, DataSize ... ORDER BY FileName, PartNo`.
    pub(super) fn headers(
        &self,
        table: &str,
        selected: &BTreeSet<String>,
    ) -> Result<Vec<ConfigRowHeader>> {
        self.check_table(table)?;
        let mut headers = Vec::new();
        for (file_name, parts) in self.published(table) {
            if !selected.is_empty() && !selected.contains(&file_name) {
                continue;
            }
            let data_size = parts.values().map(|part| part.bytes).sum::<u64>() as i64;
            for part_no in parts.keys() {
                headers.push(ConfigRowHeader {
                    file_name: file_name.clone(),
                    part_no: *part_no,
                    data_size,
                });
            }
        }
        Ok(headers)
    }

    /// The assembled rows (parts concatenated) whose published name passes
    /// `keep`, in file name order.
    pub(super) fn rows(
        &self,
        table: &str,
        keep: impl Fn(&str) -> bool,
    ) -> Result<Vec<BinaryConfigRow>> {
        self.check_table(table)?;
        let mut rows = Vec::new();
        for (file_name, parts) in self.published(table) {
            if !keep(&file_name) {
                continue;
            }
            let mut binary = Vec::with_capacity(parts.values().map(|part| part.bytes).sum::<u64>() as usize);
            for part in parts.values() {
                binary.extend(
                    fs::read(&part.path)
                        .with_context(|| format!("failed to read {}", part.path.display()))?,
                );
            }
            rows.push(BinaryConfigRow {
                file_name,
                part_no: 0,
                data_size: binary.len() as i64,
                binary,
            });
        }
        Ok(rows)
    }

    /// Rows by exact name (all rows when `names` is empty); a name the folder
    /// lacks is skipped, as `WHERE FileName IN (...)` skips it.
    pub(super) fn rows_named(
        &self,
        table: &str,
        names: &BTreeSet<String>,
    ) -> Result<Vec<BinaryConfigRow>> {
        if names.is_empty() {
            return self.rows(table, |_| true);
        }
        self.rows(table, |file_name| names.contains(file_name))
    }

    /// Part 0 of every row, as stored (`fetch_config_part0_rows_bcp`).
    pub(super) fn part0_rows(&self) -> Result<std::collections::HashMap<String, Vec<u8>>> {
        let mut out = std::collections::HashMap::new();
        for (file_name, parts) in &self.rows {
            if let Some(part) = parts.get(&0) {
                out.insert(
                    file_name.clone(),
                    fs::read(&part.path)
                        .with_context(|| format!("failed to read {}", part.path.display()))?,
                );
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_part_file_name;

    #[test]
    fn part_file_names_split_at_the_last_part_marker() {
        assert_eq!(
            parse_part_file_name("0a1b.0__part0.bin"),
            Some(("0a1b.0".to_string(), 0))
        );
        assert_eq!(
            parse_part_file_name("x__party__part12.bin"),
            Some(("x__party".to_string(), 12))
        );
        assert_eq!(parse_part_file_name("versions.bin"), None);
        assert_eq!(parse_part_file_name("root__part0.txt"), None);
    }
}
