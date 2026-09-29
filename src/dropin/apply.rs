//! `ibcmd infobase config apply` of the drop-in command line: the platform's
//! syntax (`--force`, `--dynamic`, `--session-terminate`, ...) served by the
//! own exclusive apply (`crate::mssql_config_apply`), and the platform's words
//! and exit codes for what comes back.
//!
//! What is served, and why (measured on 8.3.27.2214; the evidence and the
//! whole table are in `docs/apply/dropin-apply.md`):
//!
//! - the platform's default, `--dynamic=auto`, applies exactly like
//!   `--dynamic=disable` when the exclusive lock can be taken: the staged rows
//!   replace the active ones in place, no `_dynupdate_` rows are written. So
//!   `auto`, `disable` and `prompt` are served by the exclusive apply;
//!   `--dynamic=force` (dynamic update only) is refused by name at parsing;
//! - the platform run against a database (`--dbms=...`) does not look for other
//!   sessions at all. This apply does: with sessions connected it refuses, in
//!   the platform's words for a lock it cannot take. `--session-terminate=force`
//!   and `prompt` name what this version cannot do (end sessions), so they are
//!   accepted while nobody is connected and refused when somebody is;
//! - a stage that needs a restructuring, or anything else the own apply does
//!   not do, is refused with `требуется штатный config apply: ...` (exit 1).
//!
//! THE SEAM is [`call_apply`]: the one place the own apply is called.

use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;
use uuid::Uuid;

use crate::infobase::{ConnectionRequest, PlatformNeed, ensure_mssql, resolve_connection};
use crate::mssql_config_apply::gate::GateVerdict;
use crate::mssql_config_apply::{
    ConfigApplyOptions, ConfigApplyReport, Exclusivity, OtherSession, StructuralRefusal,
    other_sessions,
};
use crate::mssql_platform_profile::MssqlNativePlatformProfile;
use crate::platform::PlatformSpec;
use crate::settings::{DatabaseTarget, PlatformHint, Settings};
use crate::sql::{SqlExec, SqlOptions};

use super::parse::{ApplyRequest, Common, ExclusivityMode, SessionTerminate};
use super::{APPLY, read_requested_password, sql_server_name, write_json};

/// What one `config apply` came to, before it is told to the user.
#[derive(Debug)]
pub enum Outcome {
    /// The staged configuration was applied.
    Applied(Box<ConfigApplyReport>),
    /// `ConfigSave` held nothing.
    NothingToApply(Box<ConfigApplyReport>),
    /// Not carried out because this version does not serve what the stage
    /// needs (exit 1): the reason, in words.
    Refused(String),
    /// The operation failed (exit -1): the message.
    Failed(String),
}

/// The `--report` file of an apply.
#[derive(Serialize)]
struct ApplyReportFile<'a> {
    operation: &'static str,
    ok: bool,
    nothing_to_apply: bool,
    apply: &'a ConfigApplyReport,
}

/// The sessions the messages list before "and N more".
const SESSIONS_SHOWN: usize = 10;
/// What the own apply's error says when it finds other sessions.
const SESSIONS_MARKER: &str = "exclusive access is not established";
/// What it says when it cannot look for them.
const SESSIONS_BLIND_MARKER: &str = "exclusive access cannot be proven";

/// `ibcmd infobase config apply ...`: the exit code.
pub fn run(mut request: ApplyRequest) -> i32 {
    let report = request.common.report.clone();
    APPLY.start();
    if let Err(message) = read_requested_password(&mut request.common) {
        return APPLY.fail_with(&message, report.as_deref());
    }
    tell(execute(&request), report.as_deref())
}

/// Says what happened, in the platform's words, and gives the exit code.
fn tell(outcome: Outcome, report: Option<&Path>) -> i32 {
    match outcome {
        Outcome::Applied(applied) => {
            if let Some(generation) = applied
                .new_generation
                .as_deref()
                .and_then(native_generation)
            {
                println!("[INFO] Создано поколение конфигурации: {generation}");
            }
            let file = report_file(&applied, true, false);
            APPLY.succeed(report, &file)
        }
        Outcome::NothingToApply(applied) => {
            let file = report_file(&applied, true, true);
            if let Some(path) = report
                && let Err(error) = write_json(path, &file)
            {
                return APPLY.fail_with(&format!("{error:#}"), None);
            }
            println!("[INFO] Обновление конфигурации базы данных не требуется");
            0
        }
        Outcome::Refused(message) => APPLY.refuse_with(&message, report),
        Outcome::Failed(message) => APPLY.fail_with(&message, report),
    }
}

fn report_file(
    report: &ConfigApplyReport,
    ok: bool,
    nothing_to_apply: bool,
) -> ApplyReportFile<'_> {
    ApplyReportFile {
        operation: APPLY.command,
        ok,
        nothing_to_apply,
        apply: report,
    }
}

/// Connects, applies and classifies what came back.
pub fn execute(request: &ApplyRequest) -> Outcome {
    let (sql, options) = match connect(request) {
        Ok(connected) => connected,
        Err(error) => return Outcome::Failed(format!("{error:#}")),
    };
    if options.platform_profile == MssqlNativePlatformProfile::Platform8_5_1_1150 {
        return Outcome::Refused(unsupported_platform_text());
    }
    match call_apply(&sql, &options) {
        Ok(report) if report.nothing_to_apply => Outcome::NothingToApply(Box::new(report)),
        Ok(report) => Outcome::Applied(Box::new(report)),
        Err(error) => classify(&error, request.session_terminate, &|| {
            sql.client()
                .and_then(|client| other_sessions(client, &options.database).ok())
                .unwrap_or_default()
        }),
    }
}

/// THE SEAM: the one call into the own apply (`crate::mssql_config_apply`,
/// track apply #337). It applies the staged configuration exclusively and
/// asks its structural gate whether the stage needs a restructuring; the gate
/// is the apply's own default (`ApplyCheckGate` once that lands, the
/// conservative one until then). Everything above this function is the
/// platform's command line, everything below the platform's words for the
/// result.
fn call_apply(sql: &SqlExec, options: &ConfigApplyOptions) -> Result<ConfigApplyReport> {
    crate::mssql_config_apply::apply_staged_configuration(sql, options)
}

/// The SQL handle and the apply options of a request.
fn connect(request: &ApplyRequest) -> Result<(SqlExec, ConfigApplyOptions)> {
    let common = &request.common;
    let db_pwd_env = common
        .db_pwd_env
        .clone()
        .unwrap_or_else(|| "IBCMD_DB_PSW".to_string());
    let server = common.db_server.as_deref().map(sql_server_name);
    let config = resolve_connection(ConnectionRequest {
        settings: common.settings.as_deref(),
        native_config: common.native_config.as_deref(),
        format: None,
        platform: common.platform,
        source_version: common.source_version,
        dbms: common.dbms.as_deref(),
        db_server: server.as_deref(),
        db_name: common.db_name.as_deref(),
        db_user: common.db_user.as_deref(),
        db_pwd: common.db_pwd.as_deref(),
        db_pwd_env: &db_pwd_env,
        need: PlatformNeed::Given,
    })?;
    ensure_mssql(&config.dbms)?;
    let profile = native_profile(common, &config.db_server, &config.db_name)?;
    let sql = SqlExec::from_options(SqlOptions {
        sqlcmd: None,
        bcp: None,
        server: &config.db_server,
        user: config.db_user.as_deref(),
        password: config.db_pwd.as_deref(),
        password_env: &db_pwd_env,
        trust_server_certificate: true,
    })?;
    Ok((sql, apply_options(request, &config.db_name, profile)))
}

/// The options of the own apply for a request. The platform's `--force`
/// (confirm warnings) and `--session-terminate-message` need nothing here:
/// the apply raises no warning that asks for a confirmation and ends no
/// session.
pub fn apply_options(
    request: &ApplyRequest,
    database: &str,
    profile: MssqlNativePlatformProfile,
) -> ConfigApplyOptions {
    let mut options = ConfigApplyOptions::new(database, profile);
    options.exclusivity = match request.exclusivity {
        ExclusivityMode::Sql => Exclusivity::SqlSessions,
        ExclusivityMode::Assumed => Exclusivity::Assumed,
    };
    options
}

/// The storage layout the database is claimed to have: the platform named by
/// `--platform`, the settings or, when nothing names one, 8.3.27 (the only
/// one this apply is measured on; the apply verifies the claim against the
/// database).
fn native_profile(
    common: &Common,
    server: &str,
    database: &str,
) -> Result<MssqlNativePlatformProfile> {
    let settings = Settings::load(common.native_config.as_deref())?;
    let resolved = crate::settings::resolve_platform_with_source(
        common.platform.map(|platform| platform.display()),
        &settings,
        DatabaseTarget::new(Some(server), database),
        PlatformHint::None,
    )?;
    profile_of(resolved.value)
}

/// The native storage profile of a platform: its own for an exact build; for
/// a release the build this apply was measured on.
pub fn profile_of(spec: PlatformSpec) -> Result<MssqlNativePlatformProfile> {
    if let Some(profile) = spec.native_profile() {
        return Ok(profile);
    }
    match spec.release() {
        [8, 3, 27] => Ok(MssqlNativePlatformProfile::Platform8_3_27_2214),
        [8, 5, 1] => Ok(MssqlNativePlatformProfile::Platform8_5_1_1150),
        _ => bail!("для платформы {spec} не описана раскладка хранилища конфигурации"),
    }
}

fn unsupported_platform_text() -> String {
    "Применение конфигурации базы платформы 8.5 не поддерживается в этой версии ibcmd-rs \
(планируется в следующих): используйте штатный ibcmd infobase config apply"
        .to_string()
}

/// Sorts an error of the own apply into what the platform would have said.
///
/// `sessions_of` lists the sessions again when the apply refused because
/// others are connected (the apply's error carries them as text only).
pub fn classify(
    error: &anyhow::Error,
    terminate: SessionTerminate,
    sessions_of: &dyn Fn() -> Vec<OtherSession>,
) -> Outcome {
    if let Some(refusal) = error.downcast_ref::<StructuralRefusal>() {
        return Outcome::Refused(structural_text(&refusal.verdict));
    }
    let text = format!("{error:#}");
    if text.contains(SESSIONS_MARKER) {
        return sessions_outcome(&sessions_of(), terminate, &text);
    }
    if text.contains(SESSIONS_BLIND_MARKER) {
        return Outcome::Failed(format!(
            "{text}\nЕсли с базой никто не работает, укажите --exclusivity=assumed"
        ));
    }
    if needs_native_apply(&text) {
        return Outcome::Refused(format!("требуется штатный config apply: {text}"));
    }
    Outcome::Failed(text)
}

/// The own apply names a stage or a database it leaves to the platform's
/// `config apply` (or `config repair`) in words of this shape.
fn needs_native_apply(text: &str) -> bool {
    text.contains("run the native `ibcmd infobase config")
        || text.contains("need the native `ibcmd infobase config")
        || text.contains("needs the platform's own")
}

/// `требуется штатный config apply: <row>: <reason>; ...`, the way the check
/// of the restructure-check track words its refusal
/// (`apply_check::Verdict::refusal`).
pub fn structural_text(verdict: &GateVerdict) -> String {
    const SHOWN: usize = 8;
    let total = verdict.blockers.len() + verdict.blockers_omitted;
    let mut parts = verdict
        .blockers
        .iter()
        .take(SHOWN)
        .map(|blocker| format!("{}: {}", blocker.row, blocker.reason))
        .collect::<Vec<_>>();
    if total > parts.len() {
        parts.push(format!("и ещё {}", total - parts.len()));
    }
    if parts.is_empty() {
        parts.push("причина не названа".to_string());
    }
    format!("требуется штатный config apply: {}", parts.join("; "))
}

/// Other sessions keep the exclusive lock from being taken.
fn sessions_outcome(
    sessions: &[OtherSession],
    terminate: SessionTerminate,
    detail: &str,
) -> Outcome {
    let listing = sessions_text(sessions, detail);
    match terminate {
        SessionTerminate::Disable => Outcome::Failed(listing),
        SessionTerminate::Prompt | SessionTerminate::Force => Outcome::Refused(format!(
            "Параметр `--session-terminate={}` команды `infobase config apply` не поддерживается \
в этой версии ibcmd-rs (планируется в следующих): к базе подключены сеансы, а ibcmd-rs не завершает \
сеансы, закройте их сами\n{listing}",
            terminate.as_str()
        )),
    }
}

/// The platform's `Ошибка исключительной блокировки информационной базы.`
/// and its list of active sessions, from what SQL Server shows.
pub fn sessions_text(sessions: &[OtherSession], detail: &str) -> String {
    let mut out = String::from("Ошибка исключительной блокировки информационной базы.");
    if sessions.is_empty() {
        out.push_str(&format!("\nК базе подключены другие сеансы ({detail})"));
        return out;
    }
    out.push_str("\nАктивные сеансы и соединения:");
    let shown = sessions.len().min(SESSIONS_SHOWN);
    for (index, session) in sessions.iter().take(shown).enumerate() {
        let end = if index + 1 == sessions.len() { "" } else { ";" };
        out.push_str(&format!(
            "\nкомпьютер: {}, приложение: {}, соединение с СУБД: {} ({}, {}){end}",
            or_unknown(&session.host),
            or_unknown(&session.program),
            session.session_id,
            or_unknown(&session.login),
            session.status,
        ));
    }
    if sessions.len() > shown {
        out.push_str(&format!("\nи ещё {}", sessions.len() - shown));
    }
    out.push_str(
        "\nЗакройте их (если рабочий процесс лишь держит соединение из пула, остановите его) и повторите",
    );
    out
}

fn or_unknown(value: &str) -> &str {
    if value.is_empty() { "?" } else { value }
}

/// The generation as the platform prints it after `Создано поколение
/// конфигурации:`: the sixteen bytes of the GUID as stored (little-endian
/// fields), in hex, and eight zeros (measured: the generation
/// `40cfe0ac-6f3b-4851-85ba-295e568663f6` of a `versions` row is printed
/// `ace0cf403b6f514885ba295e568663f600000000`).
pub fn native_generation(generation: &str) -> Option<String> {
    let uuid = Uuid::parse_str(generation).ok()?;
    let mut text = uuid
        .to_bytes_le()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    text.push_str("00000000");
    Some(text)
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;
    use crate::dropin::parse::{Invocation, parse_infobase};
    use crate::mssql_config_apply::gate::{GateBlocker, GateVerdict};

    fn request(list: &[&str]) -> ApplyRequest {
        let args = list
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        match parse_infobase(&args) {
            Ok(Invocation::Apply(request)) => request,
            other => panic!("{list:?}: {other:?}"),
        }
    }

    fn session(id: i64, host: &str, program: &str) -> OtherSession {
        OtherSession {
            session_id: id,
            login: "sa".to_string(),
            host: host.to_string(),
            program: program.to_string(),
            status: "sleeping".to_string(),
            last_request_end: String::new(),
        }
    }

    #[test]
    fn the_generation_is_printed_as_the_platform_prints_it() {
        // measured on 8.3.27.2214: the line of the apply and the head of the
        // `versions` row it left
        assert_eq!(
            native_generation("40cfe0ac-6f3b-4851-85ba-295e568663f6").as_deref(),
            Some("ace0cf403b6f514885ba295e568663f600000000")
        );
        assert_eq!(
            native_generation("a3d987b5-e2d0-6e47-b11e-2b29ebd47040").as_deref(),
            Some("b587d9a3d0e2476eb11e2b29ebd4704000000000")
        );
        assert_eq!(native_generation("not a uuid"), None);
    }

    #[test]
    fn the_structural_refusal_is_the_checks_words() {
        let mut verdict = GateVerdict::default();
        assert_eq!(
            structural_text(&verdict),
            "требуется штатный config apply: причина не названа"
        );
        verdict.restructuring_required = true;
        verdict.blockers = vec![
            GateBlocker {
                row: "aaa.0".to_string(),
                reason: "descriptor differs".to_string(),
            },
            GateBlocker {
                row: "bbb".to_string(),
                reason: "new object".to_string(),
            },
        ];
        assert_eq!(
            structural_text(&verdict),
            "требуется штатный config apply: aaa.0: descriptor differs; bbb: new object"
        );
        verdict.blockers_omitted = 3;
        for index in 0..7 {
            verdict.blockers.push(GateBlocker {
                row: format!("r{index}"),
                reason: "x".to_string(),
            });
        }
        let text = structural_text(&verdict);
        assert!(text.ends_with("; и ещё 4"), "{text}");
        assert_eq!(text.matches("; ").count(), 8);
    }

    #[test]
    fn a_structural_refusal_is_exit_one_material() {
        let verdict = GateVerdict {
            restructuring_required: true,
            blockers: vec![GateBlocker {
                row: "row".to_string(),
                reason: "restructuring".to_string(),
            }],
            ..GateVerdict::default()
        };
        let error = anyhow::Error::new(StructuralRefusal { verdict });
        match classify(&error, SessionTerminate::Disable, &|| Vec::new()) {
            Outcome::Refused(text) => {
                assert_eq!(text, "требуется штатный config apply: row: restructuring")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn connected_sessions_are_refused_in_the_platforms_words() {
        let error = anyhow!(
            "exclusive access is not established: 2 other session(s) are connected to db:\n  session 57 (sa, PC, 1CV8, sleeping)"
        );
        let sessions = vec![
            session(57, "DESKTOP-1", "1CV8"),
            session(61, "DESKTOP-1", "1CV8C"),
        ];
        let outcome = classify(&error, SessionTerminate::Disable, &|| sessions.clone());
        match outcome {
            Outcome::Failed(text) => {
                assert!(text.starts_with(
                    "Ошибка исключительной блокировки информационной базы.\nАктивные сеансы и соединения:\n"
                ));
                assert!(text.contains(
                    "компьютер: DESKTOP-1, приложение: 1CV8, соединение с СУБД: 57 (sa, sleeping);\n"
                ));
                // the last one ends without a semicolon
                assert!(text.contains("1CV8C, соединение с СУБД: 61 (sa, sleeping)\n"));
                assert!(text.ends_with("остановите его) и повторите"));
            }
            other => panic!("{other:?}"),
        }
        // a wish to terminate them is a wish this version cannot grant
        for terminate in [SessionTerminate::Prompt, SessionTerminate::Force] {
            match classify(&error, terminate, &|| sessions.clone()) {
                Outcome::Refused(text) => {
                    assert!(
                        text.starts_with(&format!(
                            "Параметр `--session-terminate={}` команды `infobase config apply` не поддерживается в этой версии ibcmd-rs",
                            terminate.as_str()
                        )),
                        "{text}"
                    );
                    assert!(text.contains("компьютер: DESKTOP-1"), "{text}");
                }
                other => panic!("{other:?}"),
            }
        }
        // the list could not be read again: the apply's own words follow
        match classify(&error, SessionTerminate::Disable, &|| Vec::new()) {
            Outcome::Failed(text) => {
                assert!(text.contains("К базе подключены другие сеансы"), "{text}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_long_list_of_sessions_is_cut() {
        let sessions = (0..14)
            .map(|index| session(50 + index, "PC", "1CV8"))
            .collect::<Vec<_>>();
        let text = sessions_text(&sessions, "");
        assert_eq!(text.matches("компьютер: PC").count(), SESSIONS_SHOWN);
        assert!(text.contains("\nи ещё 4\n"), "{text}");
    }

    #[test]
    fn a_login_that_cannot_look_for_sessions_is_told_how_to_go_on() {
        let error = anyhow!(
            "exclusive access cannot be proven: the login lacks VIEW SERVER STATE, so other sessions are invisible"
        );
        match classify(&error, SessionTerminate::Disable, &|| Vec::new()) {
            Outcome::Failed(text) => {
                assert!(text.ends_with("укажите --exclusivity=assumed"), "{text}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn what_the_own_apply_leaves_to_the_platform_is_exit_one_material() {
        // the words of `mssql_config_apply` (c2d2de0f)
        for text in [
            "Params holds 3 dynamic-update overlay row(s) (names with _dynupdate_): run the native `ibcmd infobase config apply`",
            "SchemaStorage is not settled (1 row(s) with Status other than 100): an interrupted restructuring or apply; run the native `ibcmd infobase config repair` first",
            "the own config apply is measured on 8.3.27 only; the 8.5 storage is not verified for it yet: run the native `ibcmd infobase config apply`",
        ] {
            match classify(&anyhow!("{text}"), SessionTerminate::Disable, &|| {
                Vec::new()
            }) {
                Outcome::Refused(refusal) => {
                    assert!(
                        refusal.starts_with("требуется штатный config apply: "),
                        "{refusal}"
                    )
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        // anything else is a failure, with its context
        let error = anyhow!("connection refused").context("the config apply transaction failed");
        match classify(&error, SessionTerminate::Disable, &|| Vec::new()) {
            Outcome::Failed(text) => {
                assert_eq!(
                    text,
                    "the config apply transaction failed: connection refused"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_options_follow_the_request() {
        let profile = MssqlNativePlatformProfile::Platform8_3_27_2214;
        let options = apply_options(&request(&["config", "apply", "--db-name=b"]), "b", profile);
        assert_eq!(options.database, "b");
        assert_eq!(options.exclusivity, Exclusivity::SqlSessions);
        assert!(!options.dry_run && !options.rehearse);
        let options = apply_options(
            &request(&["config", "apply", "--exclusivity=assumed"]),
            "b",
            profile,
        );
        assert_eq!(options.exclusivity, Exclusivity::Assumed);
    }

    #[test]
    fn a_platform_names_a_storage_profile() {
        let profile = |text: &str| profile_of(crate::platform::parse(text).unwrap()).unwrap();
        assert_eq!(
            profile("8.3.27.2214"),
            MssqlNativePlatformProfile::Platform8_3_27_2214
        );
        assert_eq!(
            profile("8.3.27.1989"),
            MssqlNativePlatformProfile::Platform8_3_27_1989
        );
        // a release stands for the build this apply was measured on
        assert_eq!(
            profile("8.3.27"),
            MssqlNativePlatformProfile::Platform8_3_27_2214
        );
        assert_eq!(
            profile("8.5.1"),
            MssqlNativePlatformProfile::Platform8_5_1_1150
        );
        assert_eq!(
            profile("8.5.1.1150"),
            MssqlNativePlatformProfile::Platform8_5_1_1150
        );
    }

    #[test]
    fn the_report_file_says_what_was_done() {
        // the shape of the file is the caller's contract
        let value = serde_json::to_value(ApplyReportFile {
            operation: "infobase config apply",
            ok: true,
            nothing_to_apply: false,
            apply: &sample_report(),
        })
        .unwrap();
        assert_eq!(value["operation"], "infobase config apply");
        assert_eq!(value["ok"], true);
        assert_eq!(value["nothing_to_apply"], false);
        assert_eq!(value["apply"]["database"], "db");
    }

    fn sample_report() -> ConfigApplyReport {
        ConfigApplyReport {
            schema_version: 1,
            database: "db".to_string(),
            platform_profile: "platform-8.3.27.2214".to_string(),
            storage_schema_sha256: String::new(),
            dry_run: false,
            rehearsal: false,
            executed: true,
            nothing_to_apply: false,
            exclusivity: Exclusivity::SqlSessions,
            active_generation: None,
            new_generation: Some("40cfe0ac-6f3b-4851-85ba-295e568663f6".to_string()),
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
            timings: Default::default(),
        }
    }
}
