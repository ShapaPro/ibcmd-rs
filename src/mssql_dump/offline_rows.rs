//! `mssql-dump-config --rows-dir`: the Config table read from a folder of
//! stored rows instead of SQL Server.
//!
//! The folder holds one `<FileName>__part<N>.bin` file per stored part, the
//! BinaryData column exactly as the table keeps it (raw deflate for most
//! rows) -- the format of `F:\ibcmd\lab\rawrows\*\Config` and of the row sets
//! `audit-empty-stage --rows-out` writes. While a folder is active every
//! Config read of the export is answered from it; any other query the export
//! would send to the server is refused, so an offline run never touches SQL.
//!
//! The folder is listed once. A query looks its names up (or walks the rows
//! once when it filters by shape), and the selected parts are read in
//! parallel, in the order the query returns them.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;

use super::config_rows::{BinaryConfigRow, ConfigRowHeader};
use super::dynamic_generation::{
    StorageGenerationOverlay, is_dynamic_generation_alias, storage_generation_overlay_for,
};

/// One stored part.
#[derive(Debug, Clone)]
struct StoredPart {
    path: PathBuf,
    bytes: u64,
}

/// The parts of one stored row, in part order (0, 1, ...).
type Parts = Vec<StoredPart>;

/// The rows of one folder, by stored file name.
#[derive(Debug)]
pub(super) struct OfflineRows {
    rows: BTreeMap<String, Parts>,
    /// The published view under the last overlay a query saw.
    overlay_view: Mutex<Option<Arc<OverlayView>>>,
}

/// The names a table publishes while a dynamic generation's overlay is
/// installed: its aliases under their published names, without the rows they
/// replace and without any other generation's aliases, as the SQL view reads
/// it. Published name -> stored name.
#[derive(Debug)]
struct OverlayView {
    table: String,
    overlay: StorageGenerationOverlay,
    published: BTreeMap<String, String>,
}

/// How a query sees the table.
enum View {
    /// No overlay: the stored names are the published names.
    Plain,
    Overlay(Arc<OverlayView>),
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

/// The stored bytes of one row: its parts concatenated in order.
fn read_parts(parts: &[StoredPart]) -> Result<Vec<u8>> {
    if let [part] = parts {
        return fs::read(&part.path)
            .with_context(|| format!("failed to read {}", part.path.display()));
    }
    let mut binary = Vec::with_capacity(parts.iter().map(|part| part.bytes).sum::<u64>() as usize);
    for part in parts {
        binary.extend(
            fs::read(&part.path)
                .with_context(|| format!("failed to read {}", part.path.display()))?,
        );
    }
    Ok(binary)
}

impl OfflineRows {
    fn load(dir: &Path) -> Result<Self> {
        let mut found = BTreeMap::<String, BTreeMap<i32, StoredPart>>::new();
        let entries =
            fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some((file_name, part_no)) = name.to_str().and_then(parse_part_file_name) else {
                continue;
            };
            let bytes = entry.metadata()?.len();
            found.entry(file_name).or_default().insert(
                part_no,
                StoredPart {
                    path: entry.path(),
                    bytes,
                },
            );
        }
        if found.is_empty() {
            bail!("{} holds no <FileName>__part<N>.bin rows", dir.display());
        }
        let mut rows = BTreeMap::new();
        for (file_name, parts) in found {
            for (expected, part_no) in parts.keys().enumerate() {
                if *part_no != expected as i32 {
                    bail!(
                        "{}: row {file_name} has part {part_no} where part {expected} was expected",
                        dir.display()
                    );
                }
            }
            rows.insert(file_name, parts.into_values().collect());
        }
        Ok(Self {
            rows,
            overlay_view: Mutex::new(None),
        })
    }

    fn check_table(&self, table: &str) -> Result<()> {
        if table.trim_matches(['[', ']']).eq_ignore_ascii_case("Config") {
            Ok(())
        } else {
            Err(refuse(&format!("the {table} table")))
        }
    }

    /// The view a query on `table` reads: the stored rows as they are, or the
    /// published view of the overlay installed for the table, built once per
    /// overlay and kept until another one is installed.
    fn view(&self, table: &str) -> View {
        let Some(overlay) = storage_generation_overlay_for(table) else {
            return View::Plain;
        };
        let mut cached = self
            .overlay_view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(view) = cached.as_ref()
            && view.table == table
            && view.overlay == overlay
        {
            return View::Overlay(view.clone());
        }
        let mut published = BTreeMap::new();
        for file_name in self.rows.keys() {
            if let Some(name) = overlay.published_name(file_name) {
                published.insert(name.to_string(), file_name.clone());
            } else if !is_dynamic_generation_alias(file_name) && !overlay.hides(file_name) {
                published.insert(file_name.clone(), file_name.clone());
            }
        }
        let view = Arc::new(OverlayView {
            table: table.to_string(),
            overlay,
            published,
        });
        *cached = Some(view.clone());
        View::Overlay(view)
    }

    /// The rows of a view whose published name passes `keep`, in published
    /// name order.
    fn select<'a>(&'a self, view: &'a View, keep: impl Fn(&str) -> bool) -> Vec<(&'a str, &'a Parts)> {
        match view {
            View::Plain => self
                .rows
                .iter()
                .filter(|(name, _)| keep(name))
                .map(|(name, parts)| (name.as_str(), parts))
                .collect(),
            View::Overlay(view) => view
                .published
                .iter()
                .filter(|(name, _)| keep(name))
                .filter_map(|(name, stored)| Some((name.as_str(), self.rows.get(stored)?)))
                .collect(),
        }
    }

    /// The rows of a view with these published names, looked up, in name
    /// order; a name the view lacks is skipped, as `WHERE FileName IN (...)`
    /// skips it.
    fn lookup<'a>(&'a self, view: &'a View, names: &'a BTreeSet<String>) -> Vec<(&'a str, &'a Parts)> {
        names
            .iter()
            .filter_map(|name| {
                let parts = match view {
                    View::Plain => self.rows.get(name)?,
                    View::Overlay(view) => self.rows.get(view.published.get(name)?)?,
                };
                Some((name.as_str(), parts))
            })
            .collect()
    }

    /// Reads and assembles the selected rows in parallel, keeping their order.
    fn read(selected: Vec<(&str, &Parts)>) -> Result<Vec<BinaryConfigRow>> {
        crate::parallel::install(|| {
            selected
                .par_iter()
                .map(|(file_name, parts)| {
                    let binary = read_parts(parts)?;
                    Ok(BinaryConfigRow {
                        file_name: file_name.to_string(),
                        part_no: 0,
                        data_size: binary.len() as i64,
                        binary,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })?
    }

    /// `SELECT FileName, PartNo, DataSize ... ORDER BY FileName, PartNo`.
    pub(super) fn headers(
        &self,
        table: &str,
        selected: &BTreeSet<String>,
    ) -> Result<Vec<ConfigRowHeader>> {
        self.check_table(table)?;
        let view = self.view(table);
        let rows = if selected.is_empty() {
            self.select(&view, |_| true)
        } else {
            self.lookup(&view, selected)
        };
        let mut headers = Vec::with_capacity(rows.len());
        for (file_name, parts) in rows {
            let data_size = parts.iter().map(|part| part.bytes).sum::<u64>() as i64;
            for part_no in 0..parts.len() {
                headers.push(ConfigRowHeader {
                    file_name: file_name.to_string(),
                    part_no: part_no as i32,
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
        let view = self.view(table);
        Self::read(self.select(&view, keep))
    }

    /// Rows by exact name (all rows when `names` is empty); a name the folder
    /// lacks is skipped, as `WHERE FileName IN (...)` skips it.
    pub(super) fn rows_named(
        &self,
        table: &str,
        names: &BTreeSet<String>,
    ) -> Result<Vec<BinaryConfigRow>> {
        self.check_table(table)?;
        let view = self.view(table);
        if names.is_empty() {
            return Self::read(self.select(&view, |_| true));
        }
        Self::read(self.lookup(&view, names))
    }

    /// Part 0 of every row, as stored (`fetch_config_part0_rows_bcp`).
    pub(super) fn part0_rows(&self) -> Result<HashMap<String, Vec<u8>>> {
        let parts = self
            .rows
            .iter()
            .filter_map(|(file_name, parts)| Some((file_name, parts.first()?)))
            .collect::<Vec<_>>();
        let read = crate::parallel::install(|| {
            parts
                .par_iter()
                .map(|(file_name, part)| {
                    let bytes = fs::read(&part.path)
                        .with_context(|| format!("failed to read {}", part.path.display()))?;
                    Ok(((*file_name).clone(), bytes))
                })
                .collect::<Result<Vec<_>>>()
        })??;
        Ok(read.into_iter().collect())
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
