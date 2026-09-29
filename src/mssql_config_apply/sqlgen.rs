//! The T-SQL of the own apply: the fingerprint queries the plan and the
//! script share, and the one-transaction script itself.
//!
//! Nothing here moves row bytes through the client: rows travel from
//! `ConfigSave` to `Config` inside the server, and the script proves it with
//! aggregate fingerprints computed there.

use std::fmt::Write as _;

use anyhow::{Result, bail};
use uuid::Uuid;

use super::model::{hex_upper, quote_ident, quote_string};

/// Error numbers of the script's `THROW`s (57300..57399).
pub mod code {
    pub const LOCK_BUSY: u32 = 57300;
    pub const NO_VIEW_SERVER_STATE: u32 = 57301;
    pub const OTHER_SESSIONS: u32 = 57302;
    pub const STAGE_DRIFTED: u32 = 57303;
    pub const ACTIVE_DRIFTED: u32 = 57304;
    pub const SPECIAL_DRIFTED: u32 = 57305;
    pub const FILES_DRIFTED: u32 = 57306;
    pub const UNFINISHED_OPERATION: u32 = 57307;
    pub const REPLACE_COUNT: u32 = 57308;
    pub const POSTCONDITION: u32 = 57309;
    pub const CLEANUP: u32 = 57310;
    pub const MARKER_CLEANUP: u32 = 57311;
    pub const ALIAS_LEFT: u32 = 57312;
    pub const CHANGE_REGISTRATION: u32 = 57313;
    pub const FILES_WRITE: u32 = 57314;
    pub const PARAMS_WRITE: u32 = 57315;
    pub const UNFINISHED_SCHEMA: u32 = 57316;
    pub const ALREADY_REGISTERED: u32 = 57317;
    pub const NODES_DRIFTED: u32 = 57318;
    pub const NEW_REGISTRATION: u32 = 57319;
}

/// Names whose presence means an earlier operation did not finish
/// (`ibcmd infobase config repair` territory).
pub const UNFINISHED_NAMES: [&str; 6] = [
    "commit",
    "dynamicCommit",
    "dbStruFinal",
    "convertPhase",
    "erase_save",
    "deleted",
];

/// An order-independent digest of a set of storage rows, computed by the
/// server: the row count, the byte total, and three sums of 32-bit slices of
/// a SHA-256 over each row's name, part, size and content hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct Fingerprint {
    pub rows: i64,
    pub bytes: i64,
    pub h1: i64,
    pub h2: i64,
    pub h3: i64,
}

impl Fingerprint {
    pub fn from_values(values: &[i64]) -> Result<Self> {
        match values {
            [rows, bytes, h1, h2, h3] => Ok(Self {
                rows: *rows,
                bytes: *bytes,
                h1: *h1,
                h2: *h2,
                h3: *h3,
            }),
            _ => bail!(
                "a fingerprint row needs five integers, got {}",
                values.len()
            ),
        }
    }

    pub fn hex(&self) -> String {
        format!(
            "{}:{}:{:x}:{:x}:{:x}",
            self.rows, self.bytes, self.h1, self.h2, self.h3
        )
    }
}

/// `SELECT n, bytes, h1, h2, h3` over the rows `source` yields. `source` is
/// the text after `FROM` and must alias the table as `s`.
pub fn fingerprint_select(source: &str) -> String {
    format!(
        "SELECT COUNT_BIG(*) AS n, ISNULL(SUM(CONVERT(bigint, t.DataSize)), 0) AS bytes, \
         ISNULL(SUM(CONVERT(bigint, CAST(SUBSTRING(t.d, 1, 4) AS int))), 0) AS h1, \
         ISNULL(SUM(CONVERT(bigint, CAST(SUBSTRING(t.d, 5, 4) AS int))), 0) AS h2, \
         ISNULL(SUM(CONVERT(bigint, CAST(SUBSTRING(t.d, 9, 4) AS int))), 0) AS h3 \
         FROM (SELECT s.DataSize AS DataSize, HASHBYTES('SHA2_256', CONCAT(s.FileName, N'|', s.PartNo, N'|', s.DataSize, N'|', \
         CONVERT(nvarchar(64), HASHBYTES('SHA2_256', s.BinaryData), 2))) AS d FROM {source}) t"
    )
}

/// The source of the staged rows.
pub fn staged_source(database: &str) -> Result<String> {
    Ok(format!("{}.dbo.ConfigSave s", quote_ident(database)?))
}

/// The `Config` rows a staged name replaces.
pub fn replaced_source(database: &str) -> Result<String> {
    let db = quote_ident(database)?;
    Ok(format!(
        "{db}.dbo.Config s WHERE EXISTS (SELECT 1 FROM {db}.dbo.ConfigSave x WHERE x.FileName = s.FileName)"
    ))
}

/// `LIKE` pattern of a dynamic alias row, with `!` as the escape character
/// (so that no backslash has to survive any quoting layer).
pub const ALIAS_PATTERN: &str = "N'%!_dynupdate!_%' ESCAPE N'!'";

/// The dynamic-update leftovers of `Config`: the marker and every alias.
pub fn special_config_source(database: &str) -> Result<String> {
    let db = quote_ident(database)?;
    Ok(format!(
        "{db}.dbo.Config s WHERE s.FileName = N'DynamicallyUpdated' OR s.FileName LIKE {ALIAS_PATTERN}"
    ))
}

/// The `Params` marker row, when present.
pub fn special_params_source(database: &str) -> Result<String> {
    let db = quote_ident(database)?;
    Ok(format!(
        "{db}.dbo.Params s WHERE s.FileName = N'DynamicallyUpdated'"
    ))
}

/// A `Files` row the script rewrites, with the digest the plan saw.
#[derive(Debug, Clone)]
pub struct FilesRewrite {
    pub file_name: String,
    pub old_data_size: i64,
    pub old_sha256_hex: String,
    pub new_bytes: Vec<u8>,
}

/// A `Params` row the script rewrites (the search information of new objects).
#[derive(Debug, Clone)]
pub struct ParamsRewrite {
    pub file_name: String,
    pub old_data_size: i64,
    pub old_sha256_hex: String,
    pub new_bytes: Vec<u8>,
    /// The platform writes the search-information rows anew (`Creation` moves
    /// too); `siVersions` only changes `Modified`.
    pub set_creation: bool,
}

/// A new object to register for every node, with the files it owns in the
/// order they are listed.
#[derive(Debug, Clone)]
pub struct NewRegistration {
    /// `_MDObjID`: the uuid in the platform's byte order, 32 hex digits.
    pub object_hex: String,
    pub files: Vec<String>,
}

/// A file an existing, registered object gains.
#[derive(Debug, Clone)]
pub struct AppendedFile {
    pub object_hex: String,
    pub file_name: String,
}

/// An exchange-plan node new objects are registered for.
#[derive(Debug, Clone)]
pub struct NodeLiteral {
    /// `_NodeTRef`, 8 hex digits.
    pub type_hex: String,
    /// `_NodeRRef`, 32 hex digits.
    pub reference_hex: String,
}

/// Everything the script is rendered from.
#[derive(Debug, Clone)]
pub struct ScriptInputs {
    pub database: String,
    /// The client process: its own sessions do not count as "other".
    pub client_pid: u32,
    /// End with `ROLLBACK` instead of `COMMIT` (a rehearsal).
    pub rehearse: bool,
    /// Require that no other user session is connected to the database.
    pub require_exclusive: bool,
    pub staged: Fingerprint,
    pub replaced: Fingerprint,
    pub special_config: Fingerprint,
    pub special_params: Fingerprint,
    /// Delete the `Params` marker too. The native apply collects the service
    /// information (`.si`) and clears that marker only when the stage carries a
    /// descriptor row; a stage of body rows alone leaves it (measured, S5).
    pub clear_params_marker: bool,
    /// Dynamic generations to fold into the ordinary rows, oldest first.
    pub generations: Vec<Uuid>,
    /// `_ConfigChngR` exists: reset the change registrations of the staged
    /// descriptors.
    pub reset_change_registrations: bool,
    pub files_rewrites: Vec<FilesRewrite>,
    pub params_rewrites: Vec<ParamsRewrite>,
    /// New objects to register (needs `reset_change_registrations`).
    pub new_registrations: Vec<NewRegistration>,
    /// The nodes new objects are registered for.
    pub nodes: Vec<NodeLiteral>,
    /// How many distinct nodes `_ConfigChngR` holds now (the plan's view of
    /// the nodes, asserted again under the lock).
    pub nodes_seen: usize,
    pub appended_files: Vec<AppendedFile>,
}

/// The `_ConfigChngR` rows (alias `r`) of every object that owns a staged
/// row: the object whose uuid a staged name starts with, and the object whose
/// file list names a staged row. `prefix` qualifies the tables (`dbo.` inside
/// the script, `[db].dbo.` outside it).
pub fn staged_objects_predicate(prefix: &str) -> String {
    format!(
        "(r._MDObjID IN (SELECT CAST(TRY_CAST(LEFT(s.FileName, 36) AS uniqueidentifier) AS binary(16)) FROM {prefix}ConfigSave s WHERE s.PartNo = 0 AND TRY_CAST(LEFT(s.FileName, 36) AS uniqueidentifier) IS NOT NULL AND (LEN(s.FileName) = 36 OR SUBSTRING(s.FileName, 37, 1) = N'.'))          OR EXISTS (SELECT 1 FROM {prefix}_ConfigChngR_ExtProps e JOIN {prefix}ConfigSave s ON s.FileName = e._FileName WHERE e._ConfigChngR_IDRRef = r._IDRRef))"
    )
}

fn throw(out: &mut String, condition: &str, code: u32, message: &str) {
    writeln!(
        out,
        "IF {condition} THROW {code}, N'{}', 1;",
        quote_string(message)
    )
    .unwrap();
}

fn assert_fingerprint(
    out: &mut String,
    label: &str,
    source: &str,
    want: &Fingerprint,
    code: u32,
    message: &str,
) {
    writeln!(
        out,
        "SELECT @n = q.n, @b = q.bytes, @h1 = q.h1, @h2 = q.h2, @h3 = q.h3 FROM ({}) q;",
        fingerprint_select(source)
    )
    .unwrap();
    throw(
        out,
        &format!(
            "@n <> {} OR @b <> {} OR @h1 <> {} OR @h2 <> {} OR @h3 <> {}",
            want.rows, want.bytes, want.h1, want.h2, want.h3
        ),
        code,
        &format!("{message} ({label})"),
    );
}

/// The whole apply as one T-SQL batch: one transaction that either leaves the
/// database exactly as it was or moves every staged row into `Config`.
pub fn render_apply_script(input: &ScriptInputs) -> Result<String> {
    let db = quote_ident(&input.database)?;
    let mut sql = String::new();
    writeln!(sql, "SET NOCOUNT ON;").unwrap();
    writeln!(sql, "SET XACT_ABORT ON;").unwrap();
    writeln!(sql, "SET LOCK_TIMEOUT 30000;").unwrap();
    writeln!(sql, "USE {db};").unwrap();
    writeln!(sql, "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE;").unwrap();
    writeln!(sql, "BEGIN TRY").unwrap();
    writeln!(sql, "BEGIN TRANSACTION;").unwrap();
    writeln!(sql, "DECLARE @r int, @touch nvarchar(256), @n bigint, @b bigint, @h1 bigint, @h2 bigint, @h3 bigint;").unwrap();
    writeln!(
        sql,
        "EXEC @r = sys.sp_getapplock @Resource = N'ibcmd-rs:config-apply', @LockMode = 'Exclusive', @LockOwner = 'Transaction', @LockTimeout = 0;"
    )
    .unwrap();
    throw(
        &mut sql,
        "@r < 0",
        code::LOCK_BUSY,
        "the config apply application lock is busy",
    );

    // Exclusive table locks first, so the checks below see a frozen state.
    for table in ["Config", "ConfigSave", "Params", "Files"] {
        writeln!(
            sql,
            "SELECT TOP (1) @touch = FileName FROM dbo.{table} WITH (TABLOCKX, HOLDLOCK) ORDER BY FileName;"
        )
        .unwrap();
    }
    if input.reset_change_registrations {
        writeln!(
            sql,
            "SELECT TOP (1) @touch = NULL FROM dbo._ConfigChngR WITH (TABLOCKX, HOLDLOCK);"
        )
        .unwrap();
    }

    // Exclusive access: no other user session on the database.
    if input.require_exclusive {
        throw(
            &mut sql,
            "HAS_PERMS_BY_NAME(NULL, NULL, N'VIEW SERVER STATE') <> 1",
            code::NO_VIEW_SERVER_STATE,
            "exclusive access cannot be proven without VIEW SERVER STATE",
        );
        throw(
            &mut sql,
            &format!(
                "EXISTS (SELECT 1 FROM sys.dm_exec_sessions WHERE is_user_process = 1 AND session_id <> @@SPID AND database_id = DB_ID() AND ISNULL(host_process_id, -1) <> {})",
                input.client_pid
            ),
            code::OTHER_SESSIONS,
            "another session is connected to the database; the apply needs exclusive access",
        );
    }

    // The unfinished-operation markers must be absent.
    let unfinished = UNFINISHED_NAMES
        .iter()
        .map(|name| format!("N'{}'", quote_string(name)))
        .collect::<Vec<_>>()
        .join(", ");
    throw(
        &mut sql,
        &format!(
            "EXISTS (SELECT 1 FROM dbo.Config WHERE FileName IN ({unfinished}) OR FileName LIKE N'%.new') OR EXISTS (SELECT 1 FROM dbo.ConfigSave WHERE FileName IN ({unfinished}) OR FileName LIKE N'%.new')"
        ),
        code::UNFINISHED_OPERATION,
        "an unfinished operation is recorded in Config or ConfigSave; run the native config repair first",
    );
    // A restructuring in flight leaves the schema storage in another state than 100.
    throw(
        &mut sql,
        "EXISTS (SELECT 1 FROM dbo.SchemaStorage WHERE Status <> 100)",
        code::UNFINISHED_SCHEMA,
        "SchemaStorage is not in the settled state (Status 100): an interrupted restructuring; run the native config repair first",
    );

    // What the plan saw must still be there.
    assert_fingerprint(
        &mut sql,
        "ConfigSave",
        &staged_source_local(),
        &input.staged,
        code::STAGE_DRIFTED,
        "ConfigSave changed since the plan was made",
    );
    assert_fingerprint(
        &mut sql,
        "Config",
        &replaced_source_local(),
        &input.replaced,
        code::ACTIVE_DRIFTED,
        "the Config rows to replace changed since the plan was made",
    );
    assert_fingerprint(
        &mut sql,
        "Config markers",
        &special_config_local(),
        &input.special_config,
        code::SPECIAL_DRIFTED,
        "the dynamic-update rows of Config changed since the plan was made",
    );
    assert_fingerprint(
        &mut sql,
        "Params marker",
        &special_params_local(),
        &input.special_params,
        code::SPECIAL_DRIFTED,
        "Params.DynamicallyUpdated changed since the plan was made",
    );

    // Timestamps as the platform writes them: local time, shifted by the
    // infobase's year offset.
    writeln!(
        sql,
        "DECLARE @offset int = ISNULL((SELECT TOP (1) Offset FROM dbo._YearOffset), 0);"
    )
    .unwrap();
    writeln!(
        sql,
        "DECLARE @now datetime2(6) = DATEADD(year, @offset, CONVERT(datetime2(6), SYSDATETIME()));"
    )
    .unwrap();

    // Fold the dynamic generations into the ordinary rows, oldest first: the
    // alias row of an object replaces its ordinary row.
    if !input.generations.is_empty() {
        for generation in &input.generations {
            let g = generation.hyphenated().to_string();
            // `<x>_dynupdate_<g>[.<suffix>]` -> `<x>[.<suffix>]`: cut the marker
            // (11 characters) and the generation (36).
            let target = "LEFT(FileName, CHARINDEX(N'_dynupdate_', FileName) - 1) + SUBSTRING(FileName, CHARINDEX(N'_dynupdate_', FileName) + 47, 4000)";
            let filter = format!(
                "FileName LIKE N'%!_dynupdate!_{g}%' ESCAPE N'!' AND FileName NOT LIKE N'versions!_dynupdate!_%' ESCAPE N'!' AND FileName NOT LIKE N'deleted!_dynupdate!_%' ESCAPE N'!'"
            );
            writeln!(sql, "DELETE FROM dbo.Config WHERE FileName IN (SELECT {target} FROM dbo.Config WHERE {filter});").unwrap();
            writeln!(
                sql,
                "UPDATE dbo.Config SET FileName = {target} WHERE {filter};"
            )
            .unwrap();
            writeln!(
                sql,
                "DELETE FROM dbo.Config WHERE FileName = N'versions_dynupdate_{g}';"
            )
            .unwrap();
        }
        throw(
            &mut sql,
            &format!(
                "EXISTS (SELECT 1 FROM dbo.Config WHERE FileName LIKE {ALIAS_PATTERN} AND {})",
                generation_filter(&input.generations)
            ),
            code::ALIAS_LEFT,
            "a dynamic alias row is left after the fold",
        );
    }
    let clear_params = input.clear_params_marker && input.special_params.rows > 0;
    if input.special_config.rows > 0 || clear_params {
        writeln!(
            sql,
            "DELETE FROM dbo.Config WHERE FileName = N'DynamicallyUpdated';"
        )
        .unwrap();
        let mut left =
            "EXISTS (SELECT 1 FROM dbo.Config WHERE FileName = N'DynamicallyUpdated')".to_owned();
        if clear_params {
            writeln!(
                sql,
                "DELETE FROM dbo.Params WHERE FileName = N'DynamicallyUpdated';"
            )
            .unwrap();
            left.push_str(
                " OR EXISTS (SELECT 1 FROM dbo.Params WHERE FileName = N'DynamicallyUpdated')",
            );
        }
        throw(
            &mut sql,
            &left,
            code::MARKER_CLEANUP,
            "DynamicallyUpdated is left after the cleanup",
        );
    }

    // The move: the staged rows replace every part of the rows they name.
    writeln!(
        sql,
        "DELETE FROM dbo.Config WHERE FileName IN (SELECT FileName FROM dbo.ConfigSave);"
    )
    .unwrap();
    writeln!(
        sql,
        "INSERT dbo.Config (FileName, Creation, Modified, Attributes, DataSize, BinaryData, PartNo) SELECT FileName, Creation, Modified, Attributes, DataSize, BinaryData, PartNo FROM dbo.ConfigSave;"
    )
    .unwrap();
    throw(
        &mut sql,
        &format!("@@ROWCOUNT <> {}", input.staged.rows),
        code::REPLACE_COUNT,
        "the number of rows moved into Config differs from the staged count",
    );

    // Change registrations: every object that owns a staged row is changed for
    // every node again, and new objects are registered.
    if input.reset_change_registrations {
        let predicate = staged_objects_predicate("dbo.");
        writeln!(
            sql,
            "UPDATE r SET _MessageNo = NULL FROM dbo._ConfigChngR r WHERE r._MessageNo IS NOT NULL AND {predicate};"
        )
        .unwrap();
        throw(
            &mut sql,
            &format!(
                "EXISTS (SELECT 1 FROM dbo._ConfigChngR r WHERE r._MessageNo IS NOT NULL AND {predicate})"
            ),
            code::CHANGE_REGISTRATION,
            "a change registration of a staged object was not reset",
        );
        render_new_registrations(&mut sql, input);
    }

    for rewrite in &input.files_rewrites {
        let name = quote_string(&rewrite.file_name);
        throw(
            &mut sql,
            &format!(
                "(SELECT COUNT_BIG(*) FROM dbo.Files WHERE FileName = N'{name}' AND PartNo = 0 AND CONVERT(bigint, DataSize) = {} AND HASHBYTES('SHA2_256', BinaryData) = 0x{}) <> 1",
                rewrite.old_data_size, rewrite.old_sha256_hex
            ),
            code::FILES_DRIFTED,
            &format!(
                "Files.{} changed since the plan was made",
                rewrite.file_name
            ),
        );
        writeln!(
            sql,
            "DELETE FROM dbo.Files WHERE FileName = N'{name}' AND PartNo <> 0;"
        )
        .unwrap();
        writeln!(
            sql,
            "UPDATE dbo.Files SET Creation = @now, Modified = @now, DataSize = {}, BinaryData = 0x{} WHERE FileName = N'{name}' AND PartNo = 0;",
            rewrite.new_bytes.len(),
            hex_upper(&rewrite.new_bytes)
        )
        .unwrap();
        throw(
            &mut sql,
            "@@ROWCOUNT <> 1",
            code::FILES_WRITE,
            &format!("Files.{} was not rewritten", rewrite.file_name),
        );
    }
    for rewrite in &input.params_rewrites {
        let name = quote_string(&rewrite.file_name);
        throw(
            &mut sql,
            &format!(
                "(SELECT COUNT_BIG(*) FROM dbo.Params WHERE FileName = N'{name}' AND PartNo = 0 AND CONVERT(bigint, DataSize) = {} AND HASHBYTES('SHA2_256', BinaryData) = 0x{}) <> 1",
                rewrite.old_data_size, rewrite.old_sha256_hex
            ),
            code::FILES_DRIFTED,
            &format!(
                "Params.{} changed since the plan was made",
                rewrite.file_name
            ),
        );
        writeln!(
            sql,
            "DELETE FROM dbo.Params WHERE FileName = N'{name}' AND PartNo <> 0;"
        )
        .unwrap();
        let creation = if rewrite.set_creation {
            "Creation = @now, "
        } else {
            ""
        };
        writeln!(
            sql,
            "UPDATE dbo.Params SET {creation}Modified = @now, DataSize = {}, BinaryData = 0x{} WHERE FileName = N'{name}' AND PartNo = 0;",
            rewrite.new_bytes.len(),
            hex_upper(&rewrite.new_bytes)
        )
        .unwrap();
        throw(
            &mut sql,
            "@@ROWCOUNT <> 1",
            code::PARAMS_WRITE,
            &format!("Params.{} was not rewritten", rewrite.file_name),
        );
    }

    // Postcondition: every staged row is in Config, byte for byte, and no
    // other part of it is.
    throw(
        &mut sql,
        "EXISTS (SELECT 1 FROM dbo.ConfigSave s WHERE NOT EXISTS (SELECT 1 FROM dbo.Config c WHERE c.FileName = s.FileName AND c.PartNo = s.PartNo AND c.DataSize = s.DataSize AND c.Creation = s.Creation AND c.Modified = s.Modified AND c.Attributes = s.Attributes AND c.BinaryData = s.BinaryData))",
        code::POSTCONDITION,
        "a staged row is missing or differs in Config after the move",
    );
    throw(
        &mut sql,
        "EXISTS (SELECT 1 FROM dbo.Config c WHERE EXISTS (SELECT 1 FROM dbo.ConfigSave s WHERE s.FileName = c.FileName) AND NOT EXISTS (SELECT 1 FROM dbo.ConfigSave s WHERE s.FileName = c.FileName AND s.PartNo = c.PartNo))",
        code::POSTCONDITION,
        "Config keeps a part the staged row does not have",
    );

    // ConfigSave is consumed.
    writeln!(sql, "DELETE FROM dbo.ConfigSave;").unwrap();
    throw(
        &mut sql,
        &format!("@@ROWCOUNT <> {}", input.staged.rows),
        code::CLEANUP,
        "ConfigSave cleanup removed an unexpected number of rows",
    );
    throw(
        &mut sql,
        "EXISTS (SELECT 1 FROM dbo.ConfigSave)",
        code::CLEANUP,
        "ConfigSave is not empty after the cleanup",
    );

    if input.rehearse {
        writeln!(sql, "ROLLBACK TRANSACTION;").unwrap();
    } else {
        writeln!(sql, "COMMIT TRANSACTION;").unwrap();
    }
    writeln!(sql, "END TRY").unwrap();
    writeln!(sql, "BEGIN CATCH").unwrap();
    writeln!(sql, "IF XACT_STATE() <> 0 ROLLBACK TRANSACTION;").unwrap();
    writeln!(sql, "THROW;").unwrap();
    writeln!(sql, "END CATCH;").unwrap();
    Ok(sql)
}

/// New objects registered for every node, their files listed, and files an
/// existing object gains appended to its list.
fn render_new_registrations(sql: &mut String, input: &ScriptInputs) {
    if input.new_registrations.is_empty() && input.appended_files.is_empty() {
        return;
    }
    throw(
        sql,
        &format!(
            "(SELECT COUNT_BIG(*) FROM (SELECT DISTINCT _NodeTRef, _NodeRRef FROM dbo._ConfigChngR) d) <> {}",
            input.nodes_seen
        ),
        code::NODES_DRIFTED,
        "the exchange-plan nodes changed since the plan was made",
    );
    if !input.new_registrations.is_empty() && !input.nodes.is_empty() {
        let objects = input
            .new_registrations
            .iter()
            .map(|object| format!("0x{}", object.object_hex))
            .collect::<Vec<_>>()
            .join(", ");
        throw(
            sql,
            &format!("EXISTS (SELECT 1 FROM dbo._ConfigChngR WHERE _MDObjID IN ({objects}))"),
            code::ALREADY_REGISTERED,
            "a new object has change registrations already",
        );
        // Ids continue the sequence of the table: the greatest one plus one.
        writeln!(
            sql,
            "DECLARE @max binary(16) = (SELECT MAX(_IDRRef) FROM dbo._ConfigChngR);"
        )
        .unwrap();
        writeln!(
            sql,
            "DECLARE @head binary(8) = SUBSTRING(@max, 1, 8), @tail bigint = CAST(SUBSTRING(@max, 9, 8) AS bigint);"
        )
        .unwrap();
        throw(
            sql,
            "@max IS NULL OR @tail > 9000000000000000000",
            code::NEW_REGISTRATION,
            "no room for new change-registration ids",
        );
        let mut values = Vec::new();
        let mut sequence = 0usize;
        for node in &input.nodes {
            for object in &input.new_registrations {
                sequence += 1;
                values.push(format!(
                    "(0x{}, 0x{}, 0x{}, {sequence})",
                    node.type_hex, node.reference_hex, object.object_hex
                ));
            }
        }
        writeln!(
            sql,
            "INSERT dbo._ConfigChngR (_NodeTRef, _NodeRRef, _MessageNo, _MDObjID, _IDRRef) SELECT CAST(v.t AS binary(4)), CAST(v.n AS binary(16)), NULL, CAST(v.o AS binary(16)), CAST(@head + CAST(@tail + v.k AS binary(8)) AS binary(16)) FROM (VALUES {}) AS v(t, n, o, k);",
            values.join(", ")
        )
        .unwrap();
        throw(
            sql,
            &format!("@@ROWCOUNT <> {sequence}"),
            code::NEW_REGISTRATION,
            "the new objects were not registered for every node",
        );
        let mut files = Vec::new();
        for object in &input.new_registrations {
            for (index, file) in object.files.iter().enumerate() {
                files.push(format!(
                    "(0x{}, {index}, N'{}')",
                    object.object_hex,
                    quote_string(file)
                ));
            }
        }
        if !files.is_empty() {
            writeln!(
                sql,
                "INSERT dbo._ConfigChngR_ExtProps (_ConfigChngR_IDRRef, _KeyField, _FileName) SELECT r._IDRRef, CAST(v.k AS binary(4)), v.f FROM (VALUES {}) AS v(o, k, f) JOIN dbo._ConfigChngR r ON r._MDObjID = CAST(v.o AS binary(16));",
                files.join(", ")
            )
            .unwrap();
            throw(
                sql,
                &format!("@@ROWCOUNT <> {}", files.len() * input.nodes.len()),
                code::NEW_REGISTRATION,
                "the files of the new objects were not listed for every node",
            );
        }
    }
    if !input.appended_files.is_empty() {
        let values = input
            .appended_files
            .iter()
            .map(|file| {
                format!(
                    "(0x{}, N'{}')",
                    file.object_hex,
                    quote_string(&file.file_name)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            sql,
            "INSERT dbo._ConfigChngR_ExtProps (_ConfigChngR_IDRRef, _KeyField, _FileName) SELECT r._IDRRef, CAST(ISNULL((SELECT MAX(CAST(e._KeyField AS int)) FROM dbo._ConfigChngR_ExtProps e WHERE e._ConfigChngR_IDRRef = r._IDRRef), -1) + 1 AS binary(4)), v.f FROM (VALUES {values}) AS v(o, f) JOIN dbo._ConfigChngR r ON r._MDObjID = CAST(v.o AS binary(16)) WHERE NOT EXISTS (SELECT 1 FROM dbo._ConfigChngR_ExtProps x WHERE x._ConfigChngR_IDRRef = r._IDRRef AND x._FileName = v.f);"
        )
        .unwrap();
        for file in &input.appended_files {
            throw(
                sql,
                &format!(
                    "EXISTS (SELECT 1 FROM dbo._ConfigChngR r WHERE r._MDObjID = 0x{} AND NOT EXISTS (SELECT 1 FROM dbo._ConfigChngR_ExtProps e WHERE e._ConfigChngR_IDRRef = r._IDRRef AND e._FileName = N'{}'))",
                    file.object_hex,
                    quote_string(&file.file_name)
                ),
                code::NEW_REGISTRATION,
                &format!("{} is not listed for every node", file.file_name),
            );
        }
    }
}

fn staged_source_local() -> String {
    "dbo.ConfigSave s".to_owned()
}

fn replaced_source_local() -> String {
    "dbo.Config s WHERE EXISTS (SELECT 1 FROM dbo.ConfigSave x WHERE x.FileName = s.FileName)"
        .to_owned()
}

fn special_config_local() -> String {
    format!(
        "dbo.Config s WHERE s.FileName = N'DynamicallyUpdated' OR s.FileName LIKE {ALIAS_PATTERN}"
    )
}

fn special_params_local() -> String {
    "dbo.Params s WHERE s.FileName = N'DynamicallyUpdated'".to_owned()
}

fn generation_filter(generations: &[Uuid]) -> String {
    let parts = generations
        .iter()
        .map(|generation| {
            format!(
                "FileName LIKE N'%!_dynupdate!_{}%' ESCAPE N'!'",
                generation.hyphenated()
            )
        })
        .collect::<Vec<_>>();
    format!("({})", parts.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> ScriptInputs {
        ScriptInputs {
            database: "db]x".to_owned(),
            client_pid: 4242,
            rehearse: false,
            require_exclusive: true,
            staged: Fingerprint {
                rows: 3,
                bytes: 300,
                h1: 1,
                h2: 2,
                h3: 3,
            },
            replaced: Fingerprint {
                rows: 2,
                bytes: 200,
                h1: 4,
                h2: 5,
                h3: 6,
            },
            special_config: Fingerprint::default(),
            special_params: Fingerprint::default(),
            clear_params_marker: true,
            generations: Vec::new(),
            reset_change_registrations: true,
            files_rewrites: Vec::new(),
            params_rewrites: Vec::new(),
            new_registrations: Vec::new(),
            nodes: Vec::new(),
            nodes_seen: 0,
            appended_files: Vec::new(),
        }
    }

    #[test]
    fn the_script_is_one_guarded_transaction() {
        let sql = render_apply_script(&inputs()).unwrap();
        assert!(sql.contains("USE [db]]x];"));
        let begin = sql.find("BEGIN TRANSACTION;").unwrap();
        let commit = sql.find("COMMIT TRANSACTION;").unwrap();
        let mv = sql.find("INSERT dbo.Config").unwrap();
        let cleanup = sql.find("DELETE FROM dbo.ConfigSave;").unwrap();
        assert!(begin < mv && mv < cleanup && cleanup < commit);
        assert!(sql.contains("THROW 57302"), "exclusive access is asserted");
        assert!(sql.contains("ISNULL(host_process_id, -1) <> 4242"));
        assert!(sql.contains("@n <> 3 OR @b <> 300 OR @h1 <> 1 OR @h2 <> 2 OR @h3 <> 3"));
        assert!(sql.contains("UPDATE r SET _MessageNo = NULL FROM dbo._ConfigChngR r WHERE r._MessageNo IS NOT NULL AND (r._MDObjID IN"));
        assert!(
            sql.contains(
                "_ConfigChngR_ExtProps e JOIN dbo.ConfigSave s ON s.FileName = e._FileName"
            ),
            "a staged file resets the object that lists it"
        );
        assert!(
            !sql.contains("DECLARE @alias"),
            "no dynamic history, no fold"
        );
        assert!(sql.contains("BEGIN CATCH"));
    }

    #[test]
    fn a_rehearsal_ends_with_a_rollback() {
        let mut input = inputs();
        input.rehearse = true;
        let sql = render_apply_script(&input).unwrap();
        assert!(sql.contains("ROLLBACK TRANSACTION;\nEND TRY"));
        assert!(!sql.contains("COMMIT TRANSACTION;"));
    }

    #[test]
    fn dynamic_generations_are_folded_oldest_first_before_the_move() {
        let mut input = inputs();
        input.generations = vec![
            Uuid::parse_str("719baa18-69ed-439a-8962-1de53d98e05e").unwrap(),
            Uuid::parse_str("8c2ac6ff-7309-4025-93f6-264cbb068d62").unwrap(),
        ];
        input.special_config = Fingerprint {
            rows: 5,
            bytes: 1,
            h1: 1,
            h2: 1,
            h3: 1,
        };
        input.special_params = Fingerprint {
            rows: 1,
            bytes: 1,
            h1: 1,
            h2: 1,
            h3: 1,
        };
        let sql = render_apply_script(&input).unwrap();
        let first = sql.find("719baa18-69ed-439a-8962-1de53d98e05e").unwrap();
        let second = sql.find("8c2ac6ff-7309-4025-93f6-264cbb068d62").unwrap();
        let mv = sql.find("INSERT dbo.Config").unwrap();
        assert!(first < second && second < mv);
        assert!(sql.contains("DELETE FROM dbo.Config WHERE FileName = N'versions_dynupdate_719baa18-69ed-439a-8962-1de53d98e05e';"));
        assert!(sql.contains("DELETE FROM dbo.Params WHERE FileName = N'DynamicallyUpdated';"));
    }

    #[test]
    fn a_stage_of_body_rows_alone_leaves_the_params_marker() {
        let mut input = inputs();
        input.special_config = Fingerprint {
            rows: 1,
            bytes: 1,
            h1: 1,
            h2: 1,
            h3: 1,
        };
        input.special_params = input.special_config;
        input.clear_params_marker = false;
        let sql = render_apply_script(&input).unwrap();
        assert!(sql.contains("DELETE FROM dbo.Config WHERE FileName = N'DynamicallyUpdated';"));
        assert!(!sql.contains("DELETE FROM dbo.Params WHERE FileName = N'DynamicallyUpdated';"));
        input.clear_params_marker = true;
        let sql = render_apply_script(&input).unwrap();
        assert!(sql.contains("DELETE FROM dbo.Params WHERE FileName = N'DynamicallyUpdated';"));
    }

    #[test]
    fn rewrites_are_guarded_by_the_digest_the_plan_saw() {
        let mut input = inputs();
        input.files_rewrites.push(FilesRewrite {
            file_name: "MobileVersions.dat".to_owned(),
            old_data_size: 10,
            old_sha256_hex: "AB".to_owned(),
            new_bytes: vec![1, 2, 3],
        });
        let sql = render_apply_script(&input).unwrap();
        assert!(sql.contains("FileName = N'MobileVersions.dat' AND PartNo = 0 AND CONVERT(bigint, DataSize) = 10 AND HASHBYTES('SHA2_256', BinaryData) = 0xAB"));
        assert!(sql.contains("DataSize = 3, BinaryData = 0x010203"));
    }

    #[test]
    fn a_params_rewrite_moves_creation_only_when_asked_to() {
        let mut input = inputs();
        input.params_rewrites.push(ParamsRewrite {
            file_name: "1a621f0f-5568-4183-bd9f-f6ef670e7090.si".to_owned(),
            old_data_size: 7,
            old_sha256_hex: "CD".to_owned(),
            new_bytes: vec![9, 9],
            set_creation: true,
        });
        input.params_rewrites.push(ParamsRewrite {
            file_name: "siVersions".to_owned(),
            old_data_size: 5,
            old_sha256_hex: "EF".to_owned(),
            new_bytes: vec![8],
            set_creation: false,
        });
        let sql = render_apply_script(&input).unwrap();
        assert!(sql.contains("UPDATE dbo.Params SET Creation = @now, Modified = @now, DataSize = 2, BinaryData = 0x0909 WHERE FileName = N'1a621f0f-5568-4183-bd9f-f6ef670e7090.si'"));
        assert!(sql.contains("UPDATE dbo.Params SET Modified = @now, DataSize = 1, BinaryData = 0x08 WHERE FileName = N'siVersions'"));
    }

    #[test]
    fn new_objects_are_registered_for_every_node_with_their_files() {
        let mut input = inputs();
        input.nodes_seen = 5;
        input.nodes = vec![
            NodeLiteral {
                type_hex: "000003DD".to_owned(),
                reference_hex: "AA".repeat(16),
            },
            NodeLiteral {
                type_hex: "000003DD".to_owned(),
                reference_hex: "BB".repeat(16),
            },
        ];
        input.new_registrations = vec![
            NewRegistration {
                object_hex: "11".repeat(16),
                files: vec!["one.0".to_owned(), "one.1".to_owned()],
            },
            NewRegistration {
                object_hex: "22".repeat(16),
                files: vec!["two.0".to_owned()],
            },
        ];
        let sql = render_apply_script(&input).unwrap();
        let reset = sql.find("UPDATE r SET _MessageNo = NULL").unwrap();
        let insert = sql.find("INSERT dbo._ConfigChngR (").unwrap();
        let listing = sql.find("INSERT dbo._ConfigChngR_ExtProps").unwrap();
        assert!(reset < insert && insert < listing);
        // the ids continue the table's sequence
        assert!(sql.contains("CAST(@head + CAST(@tail + v.k AS binary(8)) AS binary(16))"));
        // two nodes x two objects, numbered 1..4, nodes outermost
        assert!(sql.contains(&format!(
            "(0x000003DD, 0x{}, 0x{}, 1), (0x000003DD, 0x{}, 0x{}, 2), (0x000003DD, 0x{}, 0x{}, 3), (0x000003DD, 0x{}, 0x{}, 4)",
            "AA".repeat(16),
            "11".repeat(16),
            "AA".repeat(16),
            "22".repeat(16),
            "BB".repeat(16),
            "11".repeat(16),
            "BB".repeat(16),
            "22".repeat(16)
        )));
        assert!(sql.contains("@@ROWCOUNT <> 4"));
        // three files, each listed for both nodes
        assert!(sql.contains(&format!(
            "(0x{}, 0, N'one.0'), (0x{}, 1, N'one.1'), (0x{}, 0, N'two.0')",
            "11".repeat(16),
            "11".repeat(16),
            "22".repeat(16)
        )));
        assert!(sql.contains("@@ROWCOUNT <> 6"));
        assert!(sql.contains("<> 5"), "the node count is asserted");
        assert!(
            sql.contains("THROW 57317"),
            "an already registered object is refused"
        );
    }

    #[test]
    fn a_file_an_object_gains_is_appended_after_its_last_key() {
        let mut input = inputs();
        input.nodes_seen = 3;
        input.appended_files = vec![AppendedFile {
            object_hex: "33".repeat(16),
            file_name: "three.1".to_owned(),
        }];
        let sql = render_apply_script(&input).unwrap();
        assert!(sql.contains("MAX(CAST(e._KeyField AS int)) FROM dbo._ConfigChngR_ExtProps e WHERE e._ConfigChngR_IDRRef = r._IDRRef), -1) + 1 AS binary(4))"));
        assert!(sql.contains(&format!("(0x{}, N'three.1')", "33".repeat(16))));
        assert!(
            !sql.contains("INSERT dbo._ConfigChngR ("),
            "no new object, no new registration row"
        );
    }

    #[test]
    fn fingerprints_round_trip_their_five_numbers() {
        let fp = Fingerprint::from_values(&[3, 300, -1, 2, 255]).unwrap();
        assert_eq!(
            fp.hex(),
            "3:300:-1:2:ff".replace("-1", &format!("{:x}", -1i64))
        );
        assert!(Fingerprint::from_values(&[1, 2]).is_err());
        assert!(fingerprint_select("dbo.Config s").contains("FROM dbo.Config s) t"));
    }
}
