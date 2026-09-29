//! The own `config apply`: move what `infobase config import` staged in
//! `ConfigSave` into `Config` the way the platform's exclusive
//! `config apply --dynamic=disable` does, without the platform.
//!
//! What the native apply does, measured on 8.3.27.2214 (see
//! `docs/apply/own-apply.md`): it copies every staged row under a `.new` name
//! in autocommit statements, records a `commit` marker, renames the rows over
//! their names, folds an earlier dynamic generation into the ordinary rows,
//! resets the change registrations of the staged objects, gives
//! `Files.MobileVersions.dat` a new head GUID, rebuilds derived caches and
//! empties `ConfigSave`. Its crash safety is that marker protocol.
//!
//! This module reaches the same end state for a configuration that needs no
//! restructuring in **one serializable transaction** whose data never leaves
//! the server: a crash or a failed postcondition leaves the database as it was
//! (nothing for `config repair` to finish). The parts:
//!
//! - [`plan`]: reads only row metadata and server-computed fingerprints,
//!   refuses what needs a restructuring ([`gate`]), and renders the script
//!   ([`sqlgen`]);
//! - the recovery artifact ([`recovery`]) keeps the rows the apply overwrites;
//! - [`apply_staged_configuration`] runs the script and checks the result.

pub mod check_gate;
pub mod errors;
pub mod gate;
pub mod model;
pub mod objects;
pub mod recovery;
pub mod si;
pub mod sqlgen;
pub mod versions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::mssql_platform_profile::MssqlNativePlatformProfile;
use crate::sql::{ScriptVariables, SqlClient, SqlExec, SqlParam, SqlValue};

use check_gate::ApplyCheckGate;
pub use errors::{
    ExclusiveAccessRefused, ExclusiveAccessUnprovable, NativeCommand, NeedsNativeApply,
};
use gate::{ConservativeGate, GateInput, GateVerdict, StructuralGate};
use model::{RowMeta, hex_lower, quote_ident, quote_string};
use sqlgen::{
    AppendedFile, FilesRewrite, Fingerprint, NewRegistration, NodeLiteral, ScriptInputs,
    fingerprint_select, render_apply_script, replaced_source, special_config_source,
    special_params_source, staged_source,
};

/// How the apply learns that nobody else is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exclusivity {
    /// No other user session on the database, as SQL Server sees it (needs
    /// `VIEW SERVER STATE`). Refuses when a working process still holds a
    /// connection.
    SqlSessions,
    /// The caller has proved it another way (for instance with `rac session
    /// list`); the script does not look.
    Assumed,
}

/// Which overwritten rows the recovery artifact keeps the bytes of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryBlobs {
    /// Rows the apply changes (bytes differ from the staged row).
    Changed,
    /// Only the manifest of hashes.
    None,
}

/// Which structural gate judges the stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateChoice {
    /// The restructure check of track rcheck (`apply_check::check_staged`):
    /// the default. It refuses on a restructuring and on anything it cannot
    /// place.
    ApplyCheck,
    /// The conservative rule of [`gate::ConservativeGate`]: modules, forms,
    /// templates, pictures and help pages, nothing else.
    Conservative,
}

#[derive(Debug, Clone)]
pub struct ConfigApplyOptions {
    pub database: String,
    pub platform_profile: MssqlNativePlatformProfile,
    /// Plan, check and render, but write nothing (not even the recovery
    /// artifact).
    pub dry_run: bool,
    /// Run the whole script and roll it back: proves the SQL and the
    /// postconditions on this very database, changes nothing.
    pub rehearse: bool,
    pub exclusivity: Exclusivity,
    pub recovery_dir: Option<PathBuf>,
    pub recovery_blobs: RecoveryBlobs,
    pub script_output: Option<PathBuf>,
    /// The gate; [`GateChoice::ApplyCheck`] unless asked otherwise.
    pub gate: GateChoice,
    /// See [`ConservativeGate::admit_unverified_roles`]; only with
    /// [`GateChoice::Conservative`].
    pub admit_unverified_roles: bool,
}

/// The XML dialect the restructure check decodes descriptors with.
fn xml_version_of(profile: MssqlNativePlatformProfile) -> Option<&'static str> {
    match profile {
        MssqlNativePlatformProfile::Platform8_3_27_1989
        | MssqlNativePlatformProfile::Platform8_3_27_2214 => Some("2.20"),
        MssqlNativePlatformProfile::Platform8_5_1_1150 => Some("2.21"),
    }
}

/// The ONE place that picks the structural gate. Callers that need another
/// (a test, a caller with its own check) use [`plan_with_gate`] and
/// [`apply_with_gate`].
pub fn structural_gate<'a>(
    sql: &'a SqlExec,
    options: &ConfigApplyOptions,
) -> Box<dyn StructuralGate + 'a> {
    match options.gate {
        GateChoice::ApplyCheck => Box::new(ApplyCheckGate::new(
            sql,
            xml_version_of(options.platform_profile),
        )),
        GateChoice::Conservative => Box::new(options.conservative_gate()),
    }
}

impl ConfigApplyOptions {
    pub fn conservative_gate(&self) -> ConservativeGate {
        ConservativeGate {
            admit_unverified_roles: self.admit_unverified_roles,
        }
    }

    pub fn new(database: impl Into<String>, platform_profile: MssqlNativePlatformProfile) -> Self {
        Self {
            database: database.into(),
            platform_profile,
            dry_run: false,
            rehearse: false,
            exclusivity: Exclusivity::SqlSessions,
            recovery_dir: None,
            recovery_blobs: RecoveryBlobs::Changed,
            script_output: None,
            gate: GateChoice::ApplyCheck,
            admit_unverified_roles: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyTimings {
    pub storage_check_ms: u128,
    pub inventory_ms: u128,
    pub gate_ms: u128,
    pub fingerprints_ms: u128,
    pub recovery_ms: u128,
    pub sql_ms: u128,
    pub total_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageSummary {
    pub rows: usize,
    pub bytes: i64,
    pub descriptors: usize,
    pub bodies: usize,
    pub service_rows: usize,
    /// Staged rows with no `Config` row of the same name.
    pub new_rows: usize,
    /// Staged rows whose bytes equal the active row's.
    pub identical_rows: usize,
    pub replaced_rows: usize,
    pub replaced_parts_dropped: usize,
    /// Staged rows consumed without being moved into `Config` (an empty or
    /// dynamic-only `deleted` list).
    pub consumed_rows: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DynamicSummary {
    pub generations: Vec<String>,
    pub alias_rows: usize,
}

/// What the staged new rows (forms, templates, body rows) add to the apply.
#[derive(Debug, Clone, Serialize)]
pub struct NewObjectsSummary {
    pub objects: Vec<objects::NewObject>,
    pub appended_bodies: Vec<objects::NewBody>,
    /// Nodes each new object is registered for.
    pub registration_nodes: usize,
    pub search_info_records: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigApplyReport {
    pub schema_version: u32,
    pub database: String,
    pub platform_profile: String,
    pub storage_schema_sha256: String,
    pub dry_run: bool,
    pub rehearsal: bool,
    /// The transaction committed.
    pub executed: bool,
    /// Nothing was staged: nothing to apply.
    pub nothing_to_apply: bool,
    pub exclusivity: Exclusivity,
    pub active_generation: Option<String>,
    pub new_generation: Option<String>,
    pub stage: Option<StageSummary>,
    pub dynamic: Option<DynamicSummary>,
    pub gate: Option<GateVerdict>,
    pub new_objects: Option<NewObjectsSummary>,
    pub tables_touched: Vec<String>,
    /// Derived state the native apply also rewrites and this one does not
    /// (or only in part): the honest gaps.
    pub not_written: Vec<String>,
    pub warnings: Vec<String>,
    pub script_sha256: Option<String>,
    pub script_path: Option<PathBuf>,
    pub recovery_dir: Option<PathBuf>,
    pub recovery_token: Option<String>,
    pub timings: ApplyTimings,
}

/// Why the own apply stopped before writing, for callers that map it to an
/// exit code.
#[derive(Debug)]
pub struct StructuralRefusal {
    pub verdict: GateVerdict,
}

impl std::fmt::Display for StructuralRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // a gate with words of its own (the restructure check) speaks first
        if let Some(refusal) = &self.verdict.refusal {
            return write!(formatter, "{refusal}");
        }
        write!(
            formatter,
            "the staged configuration needs the platform's own config apply (a restructuring or a change this apply does not do): {} blocker(s)",
            self.verdict.blockers.len() + self.verdict.blockers_omitted
        )?;
        for blocker in self.verdict.blockers.iter().take(5) {
            write!(formatter, "\n  {}: {}", blocker.row, blocker.reason)?;
        }
        Ok(())
    }
}

impl std::error::Error for StructuralRefusal {}

fn ms(since: Instant) -> u128 {
    since.elapsed().as_millis()
}

fn require_client(sql: &SqlExec) -> Result<&dyn SqlClient> {
    sql.client().ok_or_else(|| {
        anyhow!("the own config apply runs on the built-in SQL client; drop --sqlcmd")
    })
}

const ROW_COLUMNS: &str = "FileName, PartNo, CONVERT(bigint, DataSize), DATALENGTH(BinaryData), CONVERT(int, Attributes), \
     CONVERT(varchar(27), Creation, 121), CONVERT(varchar(27), Modified, 121), CONVERT(varchar(64), HASHBYTES('SHA2_256', BinaryData), 2)";

fn read_row_metas(client: &dyn SqlClient, query: &str) -> Result<Vec<RowMeta>> {
    let mut rows = Vec::new();
    client.read_rows(query, &[], &mut |row| {
        rows.push(RowMeta {
            name: row.text(0)?.to_owned(),
            part: i32::try_from(row.i64(1)?).context("PartNo")?,
            data_size: row.i64(2)?,
            byte_len: row.i64(3)?,
            attributes: i16::try_from(row.i64(4)?).context("Attributes")?,
            creation: row.text(5)?.to_owned(),
            modified: row.text(6)?.to_owned(),
            sha256: row.text(7)?.to_ascii_lowercase(),
        });
        Ok(())
    })?;
    Ok(rows)
}

fn read_fingerprint(client: &dyn SqlClient, source: &str) -> Result<Fingerprint> {
    let rows = client.query_rows(&fingerprint_select(source), &[])?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow!("a fingerprint query returned no row"))?;
    let values = (0..5)
        .map(|index| row.i64(index))
        .collect::<Result<Vec<_>>>()?;
    Fingerprint::from_values(&values)
}

fn read_blob(
    client: &dyn SqlClient,
    database: &str,
    table: &str,
    name: &str,
) -> Result<Option<Vec<u8>>> {
    let db = quote_ident(database)?;
    let value = client.query_scalar(
        &format!("SELECT BinaryData FROM {db}.dbo.{table} WHERE FileName = @P1 AND PartNo = 0"),
        &[SqlParam::Text(name)],
    )?;
    match value {
        None => Ok(None),
        Some(SqlValue::Binary(bytes)) => Ok(Some(bytes)),
        Some(other) => bail!("{table}.{name} is not binary: {other:?}"),
    }
}

/// The list in a stage's `deleted` row: `<BOM><count>,"<row name>",<flag>,...`
/// (`0` when nothing is removed), as (name, flag) pairs. `None` when the text
/// is no such list.
fn parse_removals(plain: &[u8]) -> Option<Vec<(String, String)>> {
    let text = String::from_utf8_lossy(versions::strip_bom(plain)).into_owned();
    let mut tokens = text.trim().split(',').map(str::trim);
    let count: usize = tokens.next()?.parse().ok()?;
    let rest: Vec<&str> = tokens.collect();
    if rest.len() != count * 2 {
        return None;
    }
    let mut entries = Vec::with_capacity(count);
    for pair in rest.chunks(2) {
        let name = pair[0].strip_prefix('"')?.strip_suffix('"')?;
        entries.push((name.to_owned(), pair[1].to_owned()));
    }
    Some(entries)
}

/// A row of an online (dynamic) update: an alias, `versions_dynupdate_<g>`,
/// or the marker.
fn is_dynamic_update_row(name: &str) -> bool {
    name == "DynamicallyUpdated" || name.contains("_dynupdate_")
}

/// Whether a `deleted` list asks for nothing this apply does not do already:
/// no entry, or entries that all name (flag 0) a row of a dynamic update that
/// `Config` carries now (`overlay_rows`, lower-cased). The fold removes those
/// rows whether or not the stage lists them.
fn removals_ask_for_nothing(
    entries: &[(String, String)],
    overlay_rows: &std::collections::HashSet<String>,
) -> bool {
    entries.iter().all(|(name, flag)| {
        flag == "0"
            && is_dynamic_update_row(name)
            && !name.to_ascii_lowercase().starts_with("deleted_dynupdate_")
            && overlay_rows.contains(&name.to_ascii_lowercase())
    })
}

/// What a stage's `deleted` row lists, for the message that names it.
fn describe_removals(plain: &[u8]) -> String {
    match parse_removals(plain) {
        None => "a list this apply cannot read".to_owned(),
        Some(entries) if entries.is_empty() => {
            "it is empty (the platform's own import writes one to every stage)".to_owned()
        }
        Some(entries) => {
            let count = entries.len();
            let overlay = entries
                .iter()
                .filter(|(name, _)| is_dynamic_update_row(name))
                .count();
            let first = entries
                .iter()
                .find(|(name, _)| !is_dynamic_update_row(name))
                .map(|(name, _)| format!(", the first of them {name}"))
                .unwrap_or_default();
            format!(
                "{count} row name(s), {overlay} of them rows of a dynamic update and {} others{first}",
                count - overlay
            )
        }
    }
}

fn scalar_i64(client: &dyn SqlClient, query: &str) -> Result<i64> {
    match client.query_scalar(query, &[])? {
        Some(SqlValue::Int(value)) => Ok(value),
        other => bail!("expected an integer from {query}, got {other:?}"),
    }
}

/// A session other than ours on the database, for the message that refuses
/// the apply.
#[derive(Debug, Clone, Serialize)]
pub struct OtherSession {
    pub session_id: i64,
    pub login: String,
    pub host: String,
    pub program: String,
    pub status: String,
    pub last_request_end: String,
}

pub fn other_sessions(client: &dyn SqlClient, database: &str) -> Result<Vec<OtherSession>> {
    let permitted = scalar_i64(
        client,
        "SELECT CONVERT(bigint, HAS_PERMS_BY_NAME(NULL, NULL, N'VIEW SERVER STATE'))",
    )?;
    if permitted != 1 {
        return Err(ExclusiveAccessUnprovable {
            reason: "exclusive access cannot be proven: the login lacks VIEW SERVER STATE, so other sessions are invisible".to_owned(),
        }
        .into());
    }
    let pid = i64::from(std::process::id());
    let mut sessions = Vec::new();
    client.read_rows(
        "SELECT session_id, ISNULL(login_name, N''), ISNULL(host_name, N''), ISNULL(program_name, N''), status, ISNULL(CONVERT(varchar(27), last_request_end_time, 121), N'') \
         FROM sys.dm_exec_sessions WHERE is_user_process = 1 AND database_id = DB_ID(@P1) AND ISNULL(host_process_id, -1) <> @P2 ORDER BY session_id",
        &[SqlParam::Text(database), SqlParam::I64(pid)],
        &mut |row| {
            sessions.push(OtherSession {
                session_id: row.i64(0)?,
                login: row.text(1)?.to_owned(),
                host: row.text(2)?.to_owned(),
                program: row.text(3)?.to_owned(),
                status: row.text(4)?.to_owned(),
                last_request_end: row.text(5)?.to_owned(),
            });
            Ok(())
        },
    )?;
    Ok(sessions)
}

/// The plan and the script, built from a read-only look at the database.
pub struct ConfigApplyPlan {
    pub report: ConfigApplyReport,
    pub script: Option<String>,
    inputs: Option<ScriptInputs>,
    staged: Vec<RowMeta>,
    replaced: Vec<RowMeta>,
    mobile_versions_before: Option<Vec<u8>>,
    new: objects::NewObjects,
}

pub fn plan(sql: &SqlExec, options: &ConfigApplyOptions) -> Result<ConfigApplyPlan> {
    let gate = structural_gate(sql, options);
    plan_with_gate(sql, options, gate.as_ref())
}

/// [`plan`] with another structural gate.
pub fn plan_with_gate(
    sql: &SqlExec,
    options: &ConfigApplyOptions,
    structural_gate: &dyn StructuralGate,
) -> Result<ConfigApplyPlan> {
    let total = Instant::now();
    if options.platform_profile == MssqlNativePlatformProfile::Platform8_5_1_1150 {
        return Err(NeedsNativeApply::apply(
            "the own config apply is measured on 8.3.27 only; the 8.5 storage is not verified for it yet: run the native `ibcmd infobase config apply`",
        )
        .into());
    }
    let client = require_client(sql)?;
    let database = options.database.as_str();
    let db = quote_ident(database)?;
    let mut timings = ApplyTimings::default();

    let started = Instant::now();
    let storage = crate::mssql_platform_profile::verify_mssql_storage_profile(
        options.platform_profile,
        client,
        database,
    )?;
    timings.storage_check_ms = ms(started);

    let started = Instant::now();
    let staged = read_row_metas(
        client,
        &format!("SELECT {ROW_COLUMNS} FROM {db}.dbo.ConfigSave ORDER BY FileName, PartNo"),
    )?;
    let mut report = ConfigApplyReport {
        schema_version: 1,
        database: options.database.clone(),
        platform_profile: storage.claimed_platform_profile.clone(),
        storage_schema_sha256: storage.storage_schema_sha256.clone(),
        dry_run: options.dry_run,
        rehearsal: options.rehearse,
        executed: false,
        nothing_to_apply: false,
        exclusivity: options.exclusivity,
        active_generation: None,
        new_generation: None,
        stage: None,
        dynamic: None,
        gate: None,
        new_objects: None,
        tables_touched: Vec::new(),
        not_written: Vec::new(),
        warnings: Vec::new(),
        script_sha256: None,
        script_path: None,
        recovery_dir: None,
        recovery_token: None,
        timings: ApplyTimings::default(),
    };
    if staged.is_empty() {
        report.nothing_to_apply = true;
        timings.inventory_ms = ms(started);
        timings.total_ms = ms(total);
        report.timings = timings;
        return Ok(ConfigApplyPlan {
            report,
            script: None,
            inputs: None,
            staged,
            replaced: Vec::new(),
            mobile_versions_before: None,
            new: objects::NewObjects::default(),
        });
    }
    let replaced = read_row_metas(
        client,
        &format!(
            "SELECT {ROW_COLUMNS} FROM {db}.dbo.Config s WHERE EXISTS (SELECT 1 FROM {db}.dbo.ConfigSave x WHERE x.FileName = s.FileName) ORDER BY FileName, PartNo"
        ),
    )?;
    let special = read_row_metas(
        client,
        &format!(
            "SELECT {ROW_COLUMNS} FROM {db}.dbo.Config WHERE FileName = N'DynamicallyUpdated' OR FileName LIKE N'%\\_dynupdate\\_%' ESCAPE N'\\' ORDER BY FileName, PartNo"
        ),
    )?;
    let unfinished = sqlgen::UNFINISHED_NAMES
        .iter()
        .map(|name| format!("N'{}'", quote_string(name)))
        .collect::<Vec<_>>()
        .join(", ");
    // In `ConfigSave` a row `deleted` is not a marker of an unfinished
    // operation but the stage's own list of removals (the platform's import
    // writes one to every stage): it is judged below.
    let unfinished_in_save = sqlgen::UNFINISHED_NAMES
        .iter()
        .filter(|name| **name != "deleted")
        .map(|name| format!("N'{}'", quote_string(name)))
        .collect::<Vec<_>>()
        .join(", ");
    let left_over = scalar_i64(
        client,
        &format!(
            "SELECT (SELECT COUNT_BIG(*) FROM {db}.dbo.Config WHERE FileName IN ({unfinished}) OR FileName LIKE N'%.new') + (SELECT COUNT_BIG(*) FROM {db}.dbo.ConfigSave WHERE FileName IN ({unfinished_in_save}) OR FileName LIKE N'%.new')"
        ),
    )?;
    if left_over != 0 {
        return Err(NeedsNativeApply::repair(format!(
            "{left_over} row(s) of an unfinished operation (commit / dynamicCommit / dbStruFinal / convertPhase / erase_save / deleted / *.new) are recorded in Config or ConfigSave; run the native `ibcmd infobase config repair` first"
        ))
        .into());
    }
    // The stage's list of removals (`deleted`). This apply deletes no row that a
    // staged row does not replace, as the native apply does not. A list that
    // asks for nothing more -- empty, or naming only rows of a dynamic update
    // that `Config` carries, which the fold removes anyway -- is consumed the
    // way the native apply does it: not moved into `Config`, dropped with the
    // rest of `ConfigSave`. Any other list is a removal and is refused
    // (docs/apply/own-apply.md, "Removals").
    let mut consumed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut consumed_note = None;
    // the dynamic-update rows the list names: dropped, not folded
    let mut dropped_rows: Vec<String> = Vec::new();
    if staged
        .iter()
        .any(|row| row.name.eq_ignore_ascii_case("deleted"))
    {
        let several_parts = staged
            .iter()
            .any(|row| row.name.eq_ignore_ascii_case("deleted") && row.part != 0);
        let plain = if several_parts {
            None
        } else {
            read_blob(client, database, "ConfigSave", "deleted")?
                .and_then(|bytes| versions::inflate_row(&bytes).ok())
        };
        let overlay_rows: std::collections::HashSet<String> = special
            .iter()
            .map(|row| row.name.to_ascii_lowercase())
            .collect();
        let entries = plain.as_deref().and_then(parse_removals);
        match entries {
            Some(entries) if removals_ask_for_nothing(&entries, &overlay_rows) => {
                // A list that names dynamic-update rows must name every one that
                // `Config` carries: the native apply then deletes them without
                // folding (measured on E3 of docs/apply/own-apply.md), and what a
                // list that names only some of them does is not measured.
                let named: std::collections::HashSet<String> = entries
                    .iter()
                    .map(|(name, _)| name.to_ascii_lowercase())
                    .collect();
                if !entries.is_empty() && named != overlay_rows {
                    return Err(NeedsNativeApply::apply(format!(
                        "the stage's `deleted` list names {} of the {} rows of the dynamic update that Config carries; the native apply's answer to a partial list is not measured: run the native `ibcmd infobase config apply`",
                        named.len(),
                        overlay_rows.len()
                    ))
                    .into());
                }
                dropped_rows = entries.iter().map(|(name, _)| name.clone()).collect();
                let mut note = describe_removals(plain.as_deref().unwrap_or_default());
                if !dropped_rows.is_empty() {
                    note.push_str("; the dynamic-update rows it names are deleted, not folded");
                }
                consumed_note = Some(note);
                consumed.insert("deleted".to_owned());
            }
            _ => {
                let description = plain
                    .as_deref()
                    .map(describe_removals)
                    .unwrap_or_else(|| "a list this apply cannot read".to_owned());
                return Err(NeedsNativeApply::apply(format!(
                    "the stage carries a `deleted` row, the list of removals: {description}. This apply consumes an empty list and a list of the rows of a dynamic update, takes the stage of this repository's `infobase config import`, and removes nothing that a staged row does not replace; any other removal, and the stage of the platform's own `config import`, need the native `ibcmd infobase config apply`"
                ))
                .into());
            }
        }
    }
    // An overlay of a dynamic update in Params (a `.si` row under an alias name)
    // is folded by the native apply's `.si` promotion; this apply only folds
    // `Config`, so it leaves such a database to the native one.
    let params_overlays = scalar_i64(
        client,
        &format!(
            "SELECT COUNT_BIG(*) FROM {db}.dbo.Params WHERE FileName LIKE {}",
            sqlgen::ALIAS_PATTERN
        ),
    )?;
    if params_overlays != 0 {
        return Err(NeedsNativeApply::apply(format!(
            "Params holds {params_overlays} dynamic-update overlay row(s) (names with _dynupdate_): run the native `ibcmd infobase config apply`"
        ))
        .into());
    }
    // The schema storage of a settled infobase is at Status 100; the native apply
    // walks it through 200, 400 and 500 and back, so any other value is an
    // interrupted operation.
    let unsettled = scalar_i64(
        client,
        &format!("SELECT COUNT_BIG(*) FROM {db}.dbo.SchemaStorage WHERE Status <> 100"),
    )?;
    if unsettled != 0 {
        return Err(NeedsNativeApply::repair(format!(
            "SchemaStorage is not settled ({unsettled} row(s) with Status other than 100): an interrupted restructuring or apply; run the native `ibcmd infobase config repair` first"
        ))
        .into());
    }
    timings.inventory_ms = ms(started);

    let active: HashMap<(String, i32), RowMeta> = replaced
        .iter()
        .cloned()
        .map(|row| (row.key(), row))
        .collect();
    let staged_bytes: i64 = staged.iter().map(|row| row.byte_len).sum();
    let mut stage = StageSummary {
        rows: staged.len(),
        bytes: staged_bytes,
        descriptors: 0,
        bodies: 0,
        service_rows: 0,
        new_rows: 0,
        identical_rows: 0,
        replaced_rows: replaced.len(),
        replaced_parts_dropped: 0,
        consumed_rows: 0,
    };
    for row in &staged {
        match model::classify_name(&row.name) {
            model::RowName::Service(_) => stage.service_rows += 1,
            model::RowName::Descriptor(_) => stage.descriptors += 1,
            model::RowName::Body { .. } => stage.bodies += 1,
            model::RowName::Other => {}
        }
        if consumed.contains(&row.name.to_ascii_lowercase()) {
            stage.consumed_rows += 1;
            continue;
        }
        match active.get(&row.key()) {
            None => stage.new_rows += 1,
            Some(active_row) if active_row.sha256 == row.sha256 => stage.identical_rows += 1,
            Some(_) => {}
        }
    }
    let staged_keys: std::collections::HashSet<_> = staged.iter().map(RowMeta::key).collect();
    stage.replaced_parts_dropped = replaced
        .iter()
        .filter(|row| !staged_keys.contains(&row.key()))
        .count();

    // versions: the staged row replaces the ordinary one and names a new
    // generation.
    let staged_versions = read_blob(client, database, "ConfigSave", "versions")?
        .ok_or_else(|| anyhow!("ConfigSave holds no versions row: not a complete stage"))?;
    let staged_versions = versions::parse_versions(&staged_versions)?;
    // A delta stage (the rows of a few objects and `versions`) is a stage too:
    // the native apply takes it and keeps the active `root` and `version`.
    for service in ["root", "version"] {
        if !staged.iter().any(|row| row.name == service) {
            report.warnings.push(format!(
                "the stage has no {service} row: a delta stage, the active {service} stays"
            ));
        }
    }
    let active_versions = read_blob(client, database, "Config", "versions")?
        .ok_or_else(|| anyhow!("Config holds no versions row"))?;
    let active_versions = versions::parse_versions(&active_versions)?;
    let config_marker = read_blob(client, database, "Config", "DynamicallyUpdated")?;
    let params_marker = read_blob(client, database, "Params", "DynamicallyUpdated")?;
    let history =
        versions::parse_dynamic_history(config_marker.as_deref(), params_marker.as_deref())?;
    let mut known = vec![active_versions.generation];
    known.extend(history.generations.iter().copied());
    if known.contains(&staged_versions.generation) {
        bail!(
            "the staged versions row reuses generation {}: the stage was made against another state",
            staged_versions.generation
        );
    }
    let listed: std::collections::HashSet<String> = staged_versions
        .entries
        .iter()
        .map(|(name, _)| name.to_lowercase())
        .collect();
    let unlisted = staged
        .iter()
        .filter(|row| {
            row.part == 0
                && !listed.contains(&row.name.to_lowercase())
                && !consumed.contains(&row.name.to_ascii_lowercase())
        })
        .count();
    if unlisted > 0 {
        report.warnings.push(format!(
            "{unlisted} staged row(s) are not listed in the staged versions row"
        ));
    }
    if special.iter().any(|row| {
        row.name
            .to_ascii_lowercase()
            .starts_with("deleted_dynupdate_")
    }) {
        return Err(NeedsNativeApply::apply(
            "a deleted_dynupdate_* row records deleted objects of a dynamic generation; the own apply cannot fold deletions: run the native `ibcmd infobase config apply`",
        )
        .into());
    }
    let alias_rows = special
        .iter()
        .filter(|row| {
            row.name.contains("_dynupdate_") && !row.name.starts_with("versions_dynupdate_")
        })
        .count();
    let handled = |name: &str| {
        history
            .generations
            .iter()
            .any(|generation| name.contains(&generation.hyphenated().to_string()))
    };
    let orphan_aliases = special
        .iter()
        .filter(|row| row.name.contains("_dynupdate_") && !handled(&row.name))
        .count();
    if orphan_aliases > 0 {
        report.warnings.push(format!(
            "{orphan_aliases} dynamic alias row(s) belong to no generation of the DynamicallyUpdated markers; left untouched"
        ));
    }
    report.active_generation = Some(
        history
            .generations
            .last()
            .copied()
            .unwrap_or(active_versions.generation)
            .hyphenated()
            .to_string(),
    );
    report.new_generation = Some(staged_versions.generation.hyphenated().to_string());
    report.dynamic = Some(DynamicSummary {
        generations: history
            .generations
            .iter()
            .map(|generation| generation.hyphenated().to_string())
            .collect(),
        alias_rows,
    });
    if let Some(description) = consumed_note {
        report.warnings.push(format!(
            "the stage's `deleted` list asks for nothing this apply does not do ({description}); the row is consumed, not moved into Config, as the native apply does"
        ));
    }
    report.stage = Some(stage);

    // The change registrations exist with their file lists or not at all.
    let has_change_registrations = scalar_i64(
        client,
        &format!(
            "SELECT CASE WHEN OBJECT_ID(N'{db}.dbo._ConfigChngR', N'U') IS NULL OR OBJECT_ID(N'{db}.dbo._ConfigChngR_ExtProps', N'U') IS NULL THEN 0 ELSE 1 END"
        ),
    )? == 1;

    // New rows: a form or template an existing object gains, or a body row.
    let started = Instant::now();
    let analysis = objects::analyze(&objects::AnalysisInput {
        client,
        database,
        staged: &staged,
        active: &active,
        has_change_registrations,
    })?;
    let new = analysis.new;
    let mut analysis_blockers = analysis.blockers;
    // A new object on a change register that has no rows at all (the ERP УХ
    // corpus): the native apply writes nothing there for existing objects and
    // what it does for a new one is not measured, so the apply does not guess.
    if !new.is_empty() && has_change_registrations {
        let register_has_rows = scalar_i64(
            client,
            &format!(
                "SELECT CASE WHEN EXISTS (SELECT 1 FROM {db}.dbo._ConfigChngR) THEN 1 ELSE 0 END"
            ),
        )? == 1;
        if !register_has_rows {
            let first = new
                .objects
                .first()
                .map(|object| object.uuid.clone())
                .or_else(|| new.bodies.first().map(|body| body.file_name.clone()))
                .unwrap_or_default();
            analysis_blockers.push(gate::GateBlocker {
                row: first,
                reason: "the change register has no rows: how the native apply registers a new form, template or body there is not measured (docs/apply/own-apply.md)".to_owned(),
            });
        }
    }

    // The structural gate.
    let mut verdict = structural_gate.check(&GateInput {
        client,
        database,
        staged: &staged,
        active: &active,
        accepted_new_rows: &new.rows,
        accepted_owner_descriptors: &new.owners,
        new_object_kinds: &new.kinds,
        consumed_rows: &consumed,
    })?;
    timings.gate_ms = ms(started);
    for blocker in analysis_blockers {
        verdict.block(&blocker.row, blocker.reason);
    }
    if verdict.restructuring_required {
        report.gate = Some(verdict.clone());
        timings.total_ms = ms(total);
        report.timings = timings;
        return Err(anyhow::Error::new(StructuralRefusal { verdict }));
    }
    report.gate = Some(verdict);

    // Fingerprints the script asserts.
    let started = Instant::now();
    let staged_fp = read_fingerprint(client, &staged_source(database)?)?;
    let replaced_fp = read_fingerprint(client, &replaced_source(database)?)?;
    let special_config_fp = read_fingerprint(client, &special_config_source(database)?)?;
    let special_params_fp = read_fingerprint(client, &special_params_source(database)?)?;
    timings.fingerprints_ms = ms(started);
    if staged_fp.rows != staged.len() as i64 {
        bail!("ConfigSave changed while the plan was being made");
    }

    let has_year_offset = scalar_i64(
        client,
        &format!(
            "SELECT CASE WHEN OBJECT_ID(N'{db}.dbo._YearOffset', N'U') IS NULL THEN 0 ELSE 1 END"
        ),
    )? == 1;
    if !has_year_offset {
        report.warnings.push(
            "the database has no _YearOffset table; Files rows get unshifted timestamps".to_owned(),
        );
    }

    // Files.MobileVersions.dat: a fresh GUID at the head.
    let mut files_rewrites = Vec::new();
    let mut mobile_before = None;
    let mobile_metas = read_row_metas(
        client,
        &format!(
            "SELECT {ROW_COLUMNS} FROM {db}.dbo.Files WHERE FileName = N'MobileVersions.dat' ORDER BY PartNo"
        ),
    )?;
    match mobile_metas.as_slice() {
        [] => report.warnings.push(
            "Files.MobileVersions.dat is absent; mobile clients get no new version".to_owned(),
        ),
        [meta] if meta.part == 0 => {
            let current = read_blob(client, database, "Files", "MobileVersions.dat")?
                .ok_or_else(|| anyhow!("Files.MobileVersions.dat vanished"))?;
            let next = versions::mobile_versions_prepend(&current, Uuid::new_v4())?;
            files_rewrites.push(FilesRewrite {
                file_name: "MobileVersions.dat".to_owned(),
                old_data_size: meta.data_size,
                old_sha256_hex: meta.sha256.to_ascii_uppercase(),
                new_bytes: next,
            });
            mobile_before = Some(current);
        }
        _ => bail!("Files.MobileVersions.dat has several parts"),
    }

    // The `Params` rows to rewrite: the search information of new objects.
    // The `.ui` rows (the platform's configuration-licensing records) are never
    // written.
    let params_rewrites = new.search_info.clone();

    let mut touched = vec!["Config", "ConfigSave"];
    if config_marker.is_some() || params_marker.is_some() || !params_rewrites.is_empty() {
        touched.push("Params");
    }
    if has_change_registrations {
        touched.push("_ConfigChngR");
        if !new.is_empty() {
            touched.push("_ConfigChngR_ExtProps");
        }
    }
    if !files_rewrites.is_empty() {
        touched.push("Files");
    }
    report.tables_touched = touched.into_iter().map(str::to_owned).collect();
    let nodes_seen = if new.is_empty() || !has_change_registrations {
        0
    } else {
        scalar_i64(
            client,
            &format!(
                "SELECT COUNT_BIG(*) FROM (SELECT DISTINCT _NodeTRef, _NodeRRef FROM {db}.dbo._ConfigChngR) d"
            ),
        )? as usize
    };
    report.new_objects = (!new.is_empty()).then(|| NewObjectsSummary {
        objects: new.objects.clone(),
        appended_bodies: new.bodies.clone(),
        registration_nodes: new.nodes.len(),
        search_info_records: new.search_info_records,
    });
    let object_hex = |uuid: &str| -> Result<String> {
        Ok(model::hex_upper(
            &Uuid::parse_str(uuid)
                .with_context(|| format!("{uuid} is not a uuid"))?
                .to_bytes_le(),
        ))
    };
    let new_registrations = new
        .objects
        .iter()
        .map(|object| {
            Ok(NewRegistration {
                object_hex: object_hex(&object.uuid)?,
                files: object.bodies.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let appended_files = new
        .bodies
        .iter()
        .map(|body| {
            Ok(AppendedFile {
                object_hex: object_hex(&body.object)?,
                file_name: body.file_name.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let nodes = new
        .nodes
        .iter()
        .map(|node| NodeLiteral {
            type_hex: node.type_ref.clone(),
            reference_hex: node.reference.clone(),
        })
        .collect::<Vec<_>>();
    report.not_written = vec![
        "Params .ui rows (the platform's configuration-licensing records, track ui #340): never written; the native apply re-encrypts two of them on every apply".to_owned(),
    ];
    report.not_written.extend([
        "Params .si service-information rows and siVersions, except the main row and its version when a new form or template adds records (the native apply re-encodes every .si row with a new version; the content is unchanged otherwise)".to_owned(),
        "the help/search index in Files (userDocs_ru*, userPostings_ru*, userVocabulary_ru*)".to_owned(),
        "the extension CAS garbage collection (ConfigCAS, Files CAS_GC_Info, extd_props_cached/gc.mrk)".to_owned(),
        "scratch rows of the extension restructure (_ExtensionsRestructNGS)".to_owned(),
    ]);

    let consumed_row_count = staged
        .iter()
        .filter(|row| consumed.contains(&row.name.to_ascii_lowercase()))
        .count() as i64;
    let mut consumed_names = consumed.iter().cloned().collect::<Vec<_>>();
    consumed_names.sort();
    let inputs = ScriptInputs {
        database: options.database.clone(),
        client_pid: std::process::id(),
        rehearse: options.rehearse,
        require_exclusive: options.exclusivity == Exclusivity::SqlSessions,
        staged: staged_fp,
        replaced: replaced_fp,
        special_config: special_config_fp,
        special_params: special_params_fp,
        clear_params_marker: report
            .stage
            .as_ref()
            .is_some_and(|stage| stage.descriptors > 0),
        generations: history.generations.clone(),
        reset_change_registrations: has_change_registrations,
        files_rewrites,
        params_rewrites,
        new_registrations,
        nodes,
        nodes_seen,
        appended_files,
        consumed_names,
        consumed_row_count,
        dropped_rows,
    };
    let script = render_apply_script(&inputs)?;
    let script_sha = hex_lower(&Sha256::digest(script.as_bytes()));
    report.script_sha256 = Some(script_sha.clone());
    report.recovery_token = Some(script_sha[..16].to_owned());
    timings.total_ms = ms(total);
    report.timings = timings;
    Ok(ConfigApplyPlan {
        report,
        script: Some(script),
        inputs: Some(inputs),
        staged,
        replaced,
        mobile_versions_before: mobile_before,
        new,
    })
}

/// Plans and, unless `dry_run`, applies the staged configuration.
pub fn apply_staged_configuration(
    sql: &SqlExec,
    options: &ConfigApplyOptions,
) -> Result<ConfigApplyReport> {
    let gate = structural_gate(sql, options);
    apply_with_gate(sql, options, gate.as_ref())
}

pub fn apply_with_gate(
    sql: &SqlExec,
    options: &ConfigApplyOptions,
    structural_gate: &dyn StructuralGate,
) -> Result<ConfigApplyReport> {
    let total = Instant::now();
    let client = require_client(sql)?;
    let mut plan = plan_with_gate(sql, options, structural_gate)?;
    if plan.report.nothing_to_apply {
        return Ok(plan.report);
    }
    let script = plan
        .script
        .clone()
        .ok_or_else(|| anyhow!("the plan has no script"))?;
    if let Some(path) = &options.script_output {
        write_artifact(path, script.as_bytes())?;
        plan.report.script_path = Some(path.clone());
    }
    if options.exclusivity == Exclusivity::SqlSessions {
        let sessions = other_sessions(client, &options.database)?;
        if !sessions.is_empty() {
            return Err(ExclusiveAccessRefused {
                database: options.database.clone(),
                sessions,
                in_transaction: false,
            }
            .into());
        }
    }
    if options.dry_run {
        plan.report.timings.total_ms = ms(total);
        return Ok(plan.report);
    }

    // The recovery artifact first: the pre-image of what the script overwrites.
    let started = Instant::now();
    let token = plan
        .report
        .recovery_token
        .clone()
        .ok_or_else(|| anyhow!("the plan has no recovery token"))?;
    let dir = options.recovery_dir.clone().unwrap_or_else(|| {
        std::env::temp_dir()
            .join("ibcmd-rs")
            .join("config-apply-recovery")
            .join(format!("{}-{token}", safe_stem(&options.database)))
    });
    recovery::write_recovery(
        client,
        &recovery::RecoveryRequest {
            database: &options.database,
            dir: &dir,
            token: &token,
            blobs: options.recovery_blobs,
            staged: &plan.staged,
            replaced: &plan.replaced,
            mobile_versions_before: plan.mobile_versions_before.as_deref(),
            reset_change_registrations: plan
                .inputs
                .as_ref()
                .is_some_and(|inputs| inputs.reset_change_registrations),
            new_objects: &plan.new,
            params_rewrites: plan
                .inputs
                .as_ref()
                .map_or(&[][..], |inputs| inputs.params_rewrites.as_slice()),
        },
    )?;
    plan.report.recovery_dir = Some(dir);
    plan.report.timings.recovery_ms = ms(started);

    let started = Instant::now();
    if let Err(error) = client.run_script(&script, ScriptVariables::Refuse) {
        // the refusals of the transaction's own checks are typed; a drifted
        // fingerprint or a failed postcondition stays an ordinary failure
        if let Some((number, message)) = errors::server_error_of(&error)
            && let Some(typed) =
                errors::from_transaction_code(number, &message, &options.database, || {
                    other_sessions(client, &options.database).unwrap_or_default()
                })
        {
            return Err(typed);
        }
        return Err(error.context("the config apply transaction failed; the database is unchanged"));
    }
    plan.report.timings.sql_ms = ms(started);
    plan.report.executed = !options.rehearse;

    // A last look, outside the transaction.
    if !options.rehearse {
        let db = quote_ident(&options.database)?;
        let left = scalar_i64(
            client,
            &format!("SELECT COUNT_BIG(*) FROM {db}.dbo.ConfigSave"),
        )?;
        if left != 0 {
            bail!("ConfigSave still holds {left} row(s) after the apply");
        }
    }
    plan.report.timings.total_ms = ms(total);
    Ok(plan.report)
}

/// `ibcmd-rs mssql-config-apply`: build the SQL handle, run, print the report.
pub fn run_command(args: &crate::cli::MssqlConfigApplyArgs) -> Result<()> {
    use crate::cli::{MssqlConfigApplyExclusivityArg, MssqlConfigApplyRecoveryArg};
    use crate::sql::SqlOptions;

    if !args.dry_run && !args.allow_non_lab {
        bail!(
            "--allow-non-lab acknowledgement is required for a database write (or use --dry-run)"
        );
    }
    let password = args.sql_user.as_deref().and_then(|_| {
        args.sql_pwd
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var(&args.sql_pwd_env).ok())
    });
    let sql = SqlExec::from_options(SqlOptions {
        sqlcmd: None,
        bcp: None,
        server: &args.server,
        user: args.sql_user.as_deref(),
        password: password.as_deref(),
        password_env: &args.sql_pwd_env,
        trust_server_certificate: true,
    })?;
    let mut options = ConfigApplyOptions::new(args.database.clone(), args.platform_profile);
    options.dry_run = args.dry_run;
    options.rehearse = args.rehearse;
    options.exclusivity = match args.exclusivity {
        MssqlConfigApplyExclusivityArg::Sql => Exclusivity::SqlSessions,
        MssqlConfigApplyExclusivityArg::Assumed => Exclusivity::Assumed,
    };
    options.recovery_dir = args.recovery_dir.clone();
    options.recovery_blobs = match args.recovery_blobs {
        MssqlConfigApplyRecoveryArg::Changed => RecoveryBlobs::Changed,
        MssqlConfigApplyRecoveryArg::None => RecoveryBlobs::None,
    };
    options.script_output = args.script_output.clone();
    options.gate = match args.gate {
        crate::cli::MssqlConfigApplyGateArg::ApplyCheck => GateChoice::ApplyCheck,
        crate::cli::MssqlConfigApplyGateArg::Conservative => GateChoice::Conservative,
    };
    if args.admit_unverified_roles && options.gate != GateChoice::Conservative {
        bail!(
            "--admit-unverified-roles is a switch of the conservative gate: add --gate conservative"
        );
    }
    options.admit_unverified_roles = args.admit_unverified_roles;
    match apply_staged_configuration(&sql, &options) {
        Ok(report) => {
            let json = serde_json::to_string_pretty(&report)?;
            if let Some(path) = &args.report {
                std::fs::write(path, &json)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
            println!("{json}");
            Ok(())
        }
        Err(error) => {
            if let Some(refusal) = refusal_report(&error, &args.database) {
                let json = serde_json::to_string_pretty(&refusal)?;
                if let Some(path) = &args.report {
                    std::fs::write(path, &json)
                        .with_context(|| format!("failed to write {}", path.display()))?;
                }
                println!("{json}");
            }
            Err(error)
        }
    }
}

/// What the command prints when the apply refuses by type: the same fields
/// whatever the refusal, `refused` naming the kind.
fn refusal_report(error: &anyhow::Error, database: &str) -> Option<serde_json::Value> {
    if let Some(refusal) = error.downcast_ref::<StructuralRefusal>() {
        return Some(serde_json::json!({
            "refused": "needs_native_apply",
            "database": database,
            "gate": refusal.verdict,
        }));
    }
    if let Some(refusal) = error.downcast_ref::<NeedsNativeApply>() {
        return Some(serde_json::json!({
            "refused": "needs_native_apply",
            "database": database,
            "native_command": match refusal.command {
                NativeCommand::Apply => "apply",
                NativeCommand::Repair => "repair",
            },
            "reason": refusal.reason,
        }));
    }
    if let Some(refusal) = error.downcast_ref::<ExclusiveAccessRefused>() {
        return Some(serde_json::json!({
            "refused": "exclusive_access",
            "database": database,
            "in_transaction": refusal.in_transaction,
            "sessions": refusal.sessions,
        }));
    }
    if let Some(refusal) = error.downcast_ref::<ExclusiveAccessUnprovable>() {
        return Some(serde_json::json!({
            "refused": "exclusive_access_unprovable",
            "database": database,
            "reason": refusal.reason,
        }));
    }
    None
}

fn safe_stem(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn write_artifact(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => Ok(()),
        Ok(_) => bail!("refusing to overwrite existing artifact {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::write(path, bytes)
            .with_context(|| format!("failed to write {}", path.display())),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_8_5_profile_is_refused_before_anything_is_read() {
        let sql = SqlExec::detached("no server in a unit test");
        let options = ConfigApplyOptions::new("db", MssqlNativePlatformProfile::Platform8_5_1_1150);
        let error = plan(&sql, &options).err().expect("8.5 is refused");
        assert!(
            error.to_string().contains("measured on 8.3.27 only"),
            "{error}"
        );
    }

    #[test]
    fn a_platform_that_is_measured_still_needs_a_server() {
        let sql = SqlExec::detached("no server in a unit test");
        let options =
            ConfigApplyOptions::new("db", MssqlNativePlatformProfile::Platform8_3_27_2214);
        let error = plan(&sql, &options).err().expect("a detached handle fails");
        assert!(!error.to_string().contains("8.5"), "{error}");
    }

    #[test]
    fn safe_stems_keep_only_file_name_characters() {
        assert_eq!(safe_stem("a b/c:d-e_f"), "a_b_c_d-e_f");
    }

    #[test]
    fn a_deleted_row_is_described_from_its_list() {
        // the shapes measured in the lab: the empty list, one row, the rows of a
        // dynamic update
        assert!(describe_removals("\u{feff}0".as_bytes()).starts_with("it is empty"));
        let one = "\u{feff}1,\"5ff28850-03db-4a0f-b95e-c2ea4d8c516d\",0";
        assert_eq!(
            describe_removals(one.as_bytes()),
            "1 row name(s), 0 of them rows of a dynamic update and 1 others, the first of them 5ff28850-03db-4a0f-b95e-c2ea4d8c516d"
        );
        let overlay = "\u{feff}3,\"ab132638-5188-470d-9432-de85f2b2c7d8_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b.0\",0,\"DynamicallyUpdated\",0,\"versions_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b\",0";
        assert_eq!(
            describe_removals(overlay.as_bytes()),
            "3 row name(s), 3 of them rows of a dynamic update and 0 others"
        );
        // a count that disagrees with the names, and text that is no list
        assert!(describe_removals(b"2,\"a\",0").contains("cannot read"));
        assert!(describe_removals(b"{1,2}").contains("cannot read"));
    }

    #[test]
    fn a_deleted_list_asks_for_nothing_when_it_is_empty_or_only_a_dynamic_update() {
        let alias =
            "ab132638-5188-470d-9432-de85f2b2c7d8_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b";
        let versions = "versions_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b";
        let carried: std::collections::HashSet<String> = [alias, versions, "DynamicallyUpdated"]
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect();
        let ask = |text: &str| {
            parse_removals(text.as_bytes())
                .map(|entries| removals_ask_for_nothing(&entries, &carried))
        };
        // empty
        assert_eq!(ask("\u{feff}0"), Some(true));
        // the rows of a dynamic update that Config carries
        let overlay = format!("\u{feff}3,\"{alias}\",0,\"DynamicallyUpdated\",0,\"{versions}\",0");
        assert_eq!(ask(&overlay), Some(true));
        // a dynamic-update row Config does not carry now
        let other = "\u{feff}1,\"a627e390-8fad-4a95-afe6-674f54813188_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b\",0";
        assert_eq!(ask(other), Some(false));
        // an ordinary row: a removal
        assert_eq!(
            ask("\u{feff}1,\"5ff28850-03db-4a0f-b95e-c2ea4d8c516d\",0"),
            Some(false)
        );
        // a name with flag 1 (a nested object, not a row)
        let flagged = format!("\u{feff}1,\"{alias}\",1");
        assert_eq!(ask(&flagged), Some(false));
        // the removal list of a dynamic update itself is never folded
        let mut with_removals = carried.clone();
        with_removals.insert("deleted_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b".to_owned());
        let entries =
            parse_removals(b"1,\"deleted_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b\",0")
                .unwrap();
        assert!(!removals_ask_for_nothing(&entries, &with_removals));
        // text that is no list
        assert_eq!(ask("{1,2}"), None);
        assert_eq!(ask("2,\"a\",0"), None);
    }

    #[test]
    fn a_refusal_is_reported_by_its_type() {
        let structural = anyhow::Error::new(StructuralRefusal {
            verdict: GateVerdict::default(),
        });
        assert_eq!(
            refusal_report(&structural, "db").unwrap()["refused"],
            "needs_native_apply"
        );
        let native = anyhow::Error::new(NeedsNativeApply::repair("unfinished"));
        let report = refusal_report(&native, "db").unwrap();
        assert_eq!(report["native_command"], "repair");
        assert_eq!(report["reason"], "unfinished");
        let sessions = anyhow::Error::new(ExclusiveAccessRefused {
            database: "db".to_owned(),
            sessions: Vec::new(),
            in_transaction: true,
        });
        let report = refusal_report(&sessions, "db").unwrap();
        assert_eq!(report["refused"], "exclusive_access");
        assert_eq!(report["in_transaction"], true);
        let blind = anyhow::Error::new(ExclusiveAccessUnprovable {
            reason: "cannot prove".to_owned(),
        });
        assert_eq!(
            refusal_report(&blind, "db").unwrap()["refused"],
            "exclusive_access_unprovable"
        );
        assert!(refusal_report(&anyhow::anyhow!("plain"), "db").is_none());
    }
}
