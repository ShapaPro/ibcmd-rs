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

/// A `Params` row the script rewrites (the `.ui` hook).
#[derive(Debug, Clone)]
pub struct ParamsRewrite {
    pub file_name: String,
    pub old_data_size: i64,
    pub old_sha256_hex: String,
    pub new_bytes: Vec<u8>,
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
    /// Dynamic generations to fold into the ordinary rows, oldest first.
    pub generations: Vec<Uuid>,
    /// `_ConfigChngR` exists: reset the change registrations of the staged
    /// descriptors.
    pub reset_change_registrations: bool,
    pub files_rewrites: Vec<FilesRewrite>,
    pub params_rewrites: Vec<ParamsRewrite>,
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
    if input.special_config.rows > 0 || input.special_params.rows > 0 {
        writeln!(
            sql,
            "DELETE FROM dbo.Config WHERE FileName = N'DynamicallyUpdated';"
        )
        .unwrap();
        writeln!(
            sql,
            "DELETE FROM dbo.Params WHERE FileName = N'DynamicallyUpdated';"
        )
        .unwrap();
        throw(
            &mut sql,
            "EXISTS (SELECT 1 FROM dbo.Config WHERE FileName = N'DynamicallyUpdated') OR EXISTS (SELECT 1 FROM dbo.Params WHERE FileName = N'DynamicallyUpdated')",
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

    // Change registrations: every staged descriptor's object is changed for
    // every node again.
    if input.reset_change_registrations {
        writeln!(
            sql,
            "UPDATE dbo._ConfigChngR SET _MessageNo = NULL WHERE _MessageNo IS NOT NULL AND _MDObjID IN (SELECT CAST(TRY_CAST(FileName AS uniqueidentifier) AS binary(16)) FROM dbo.ConfigSave WHERE PartNo = 0 AND LEN(FileName) = 36 AND TRY_CAST(FileName AS uniqueidentifier) IS NOT NULL);"
        )
        .unwrap();
        throw(
            &mut sql,
            "EXISTS (SELECT 1 FROM dbo._ConfigChngR WHERE _MessageNo IS NOT NULL AND _MDObjID IN (SELECT CAST(TRY_CAST(FileName AS uniqueidentifier) AS binary(16)) FROM dbo.ConfigSave WHERE PartNo = 0 AND LEN(FileName) = 36 AND TRY_CAST(FileName AS uniqueidentifier) IS NOT NULL))",
            code::CHANGE_REGISTRATION,
            "a change registration of a staged object was not reset",
        );
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
        writeln!(
            sql,
            "UPDATE dbo.Params SET Modified = @now, DataSize = {}, BinaryData = 0x{} WHERE FileName = N'{name}' AND PartNo = 0;",
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
            generations: Vec::new(),
            reset_change_registrations: true,
            files_rewrites: Vec::new(),
            params_rewrites: Vec::new(),
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
        assert!(sql.contains("UPDATE dbo._ConfigChngR SET _MessageNo = NULL"));
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
