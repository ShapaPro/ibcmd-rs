//! `infobase config export` and `infobase config import` against a Microsoft
//! SQL Server infobase, without the platform: the export reads the Config
//! table and writes the XML tree (`mssql_dump`), the import stages the tree
//! into ConfigSave (`mssql::stage_source_objects`), where the platform's own
//! `config apply` finds it, as it finds what its own `config import` wrote.
//!
//! The drop-in command line (`crate::dropin`) and the research round trip
//! (`crate::infobase_oracle`, `platform-oracle` builds only) call these.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use ibcmd_core::version::{PlatformBuild, XmlDialect};
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::adapters::mssql_legacy::MssqlLegacyAdapter;
use crate::cli::{
    InfobaseConfigExportArgs, InfobaseConfigFormat, InfobaseConfigImportArgs,
    InfobaseConfigSourceVersion, InfobaseImportStageMode, MssqlDumpConfigArgs,
    MssqlStageSourceObjectsArgs,
};
use crate::legacy_version::LegacyVersionAxes;

#[derive(Debug, Serialize)]
pub struct InfobaseConfigExportReport {
    pub operation: &'static str,
    pub backend: &'static str,
    pub format: &'static str,
    pub source_version: &'static str,
    pub dbms: String,
    pub db_server: String,
    pub db_name: String,
    pub db_user: Option<String>,
    pub password_source: Option<String>,
    /// The platform's ibcmd configuration file (`--config`), as given.
    pub native_config: Option<PathBuf>,
    pub output_dir: PathBuf,
    pub temp_dump_dir: PathBuf,
    /// Files of the written tree; counted only when asked (a walk of the
    /// whole tree: 140 709 files for ERP УХ).
    pub exported_files: Option<usize>,
    pub raw_rows: usize,
    pub metadata_xml_rows: usize,
    pub module_text_rows: usize,
    pub source_asset_rows: usize,
    pub dump_timings: crate::mssql_dump::MssqlDumpTimingReport,
}

#[derive(Debug, Serialize)]
pub struct InfobaseConfigImportReport {
    pub operation: &'static str,
    pub backend: &'static str,
    pub format: &'static str,
    /// The XML version the tree was read as: the one given, else the tree's
    /// own (`Configuration.xml`).
    pub source_version: Option<String>,
    pub dbms: String,
    pub db_server: String,
    pub db_name: String,
    pub db_user: Option<String>,
    pub native_config: Option<PathBuf>,
    pub source_dir: PathBuf,
    /// `base-free` (every row compiled from the tree: the target holds none
    /// of its configuration) or `patch` (the target's own rows patched).
    pub stage_mode: &'static str,
    /// Why that mode: asked for, or what the target's Config holds.
    pub stage_mode_reason: String,
    /// Rows in the target's Config before the import (-1: not read).
    pub target_config_rows: i64,
    pub staged_rows_before: i64,
    pub staged_rows_after: i64,
    pub scripts: Vec<PathBuf>,
}

/// The export refuses a directory that already holds files, as the
/// platform's own export does (`--force` does not change that).
#[derive(Debug)]
pub struct OutputDirectoryNotEmpty(pub PathBuf);

impl std::fmt::Display for OutputDirectoryNotEmpty {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "output directory is not empty: {}",
            self.0.display()
        )
    }
}

impl std::error::Error for OutputDirectoryNotEmpty {}

/// Everything a command was given to reach its database with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConnectionRequest<'a> {
    pub settings: Option<&'a Path>,
    pub native_config: Option<&'a Path>,
    pub format: Option<InfobaseConfigFormat>,
    pub source_version: Option<InfobaseConfigSourceVersion>,
    pub dbms: Option<&'a str>,
    pub db_server: Option<&'a str>,
    pub db_name: Option<&'a str>,
    pub db_user: Option<&'a str>,
    pub db_pwd: Option<&'a str>,
    pub db_pwd_env: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectionConfig {
    pub dbms: String,
    pub db_server: String,
    pub db_name: String,
    pub db_user: Option<String>,
    pub db_pwd: Option<String>,
    pub password_source: Option<String>,
    pub native_config: Option<PathBuf>,
    pub format: InfobaseConfigFormat,
    pub legacy_adapter: MssqlLegacyAdapter,
    /// Whether the XML version was asked for (command line or settings)
    /// rather than defaulted: an import reads an unasked one off the tree.
    pub xml_version_given: bool,
}

impl ConnectionConfig {
    pub(crate) fn legacy_source_version(&self) -> Result<InfobaseConfigSourceVersion> {
        self.legacy_adapter.legacy_selector().ok_or_else(|| {
            anyhow!(
                "legacy MSSQL adapter does not support XML dialect {}",
                self.legacy_adapter.xml_dialect()
            )
        })
    }
}

impl InfobaseConfigExportArgs {
    fn connection(&self) -> ConnectionRequest<'_> {
        ConnectionRequest {
            settings: self.settings.as_deref(),
            native_config: self.native_config.as_deref(),
            format: self.format,
            source_version: self.source_version,
            dbms: self.dbms.as_deref(),
            db_server: self.db_server.as_deref(),
            db_name: self.db_name.as_deref(),
            db_user: self.db_user.as_deref(),
            db_pwd: self.db_pwd.as_deref(),
            db_pwd_env: &self.db_pwd_env,
        }
    }
}

impl InfobaseConfigImportArgs {
    fn connection(&self) -> ConnectionRequest<'_> {
        ConnectionRequest {
            settings: self.settings.as_deref(),
            native_config: self.native_config.as_deref(),
            format: self.format,
            source_version: self.source_version,
            dbms: self.dbms.as_deref(),
            db_server: self.db_server.as_deref(),
            db_name: self.db_name.as_deref(),
            db_user: self.db_user.as_deref(),
            db_pwd: self.db_pwd.as_deref(),
            db_pwd_env: &self.db_pwd_env,
        }
    }
}

pub fn export_config(args: &InfobaseConfigExportArgs) -> Result<InfobaseConfigExportReport> {
    let config = resolve_connection(args.connection())?;
    ensure_mssql(&config.dbms)?;
    export_config_report(
        &config,
        &args.sqlcmd,
        &args.db_pwd_env,
        &args.output_dir,
        args.overwrite,
        args.count_files,
        Vec::new(),
    )
}

pub(crate) fn export_config_report(
    config: &ConnectionConfig,
    sqlcmd: &Path,
    db_pwd_env: &str,
    output_dir_arg: &Path,
    overwrite: bool,
    count_exported_files: bool,
    file_names: Vec<String>,
) -> Result<InfobaseConfigExportReport> {
    let source_version = config.legacy_source_version()?;
    let output_dir = absolute_path(output_dir_arg)?;
    prepare_output_dir(&output_dir, overwrite)?;
    let dump_args = MssqlDumpConfigArgs {
        rows_dir: None,
        model_export: false,
        legacy_export: false,
        sqlcmd: sqlcmd.to_path_buf(),
        bcp_executable: crate::mssql_dump::bcp_executable_for_sqlcmd(sqlcmd),
        runtime_journal: None,
        server: config.db_server.clone(),
        sql_user: config.db_user.clone(),
        sql_pwd: config.db_pwd.clone(),
        sql_pwd_env: db_pwd_env.to_string(),
        database: config.db_name.clone(),
        output_dir: output_dir.clone(),
        overwrite: false,
        include_config_save: false,
        file_names,
        file_name_lists: Vec::new(),
        inflate: false,
        extract_module_text: true,
        extract_metadata_xml: true,
        require_complete_root_metadata: false,
        require_complete_source_assets: false,
        collect_all_source_asset_diagnostics: false,
        no_binary_rows: true,
        write_binary_rows: false,
        write_manifest: false,
        source_version,
    };
    let dump = crate::mssql_dump::dump_config(&dump_args)?;

    let exported_files = if count_exported_files {
        Some(count_files(&output_dir)?)
    } else {
        None
    };

    Ok(InfobaseConfigExportReport {
        operation: "infobase config export",
        backend: "mssql-config-direct",
        format: format_name(config.format),
        source_version: source_version.as_str(),
        dbms: config.dbms.clone(),
        db_server: config.db_server.clone(),
        db_name: config.db_name.clone(),
        db_user: config.db_user.clone(),
        password_source: config.password_source.clone(),
        native_config: config.native_config.clone(),
        output_dir,
        temp_dump_dir: dump_args.output_dir,
        exported_files,
        raw_rows: dump.total_rows,
        metadata_xml_rows: dump.total_metadata_xml_rows,
        module_text_rows: dump.total_module_text_rows,
        source_asset_rows: dump.total_source_asset_rows,
        dump_timings: dump.timings,
    })
}

pub fn import_config(args: &InfobaseConfigImportArgs) -> Result<InfobaseConfigImportReport> {
    let config = resolve_connection(args.connection())?;
    ensure_mssql(&config.dbms)?;

    let mut stage_args = build_import_stage_args(&config, args)?;
    if !stage_args.source_root.is_dir() {
        bail!(
            "каталог файлов конфигурации не найден: {}",
            args.source_dir.display()
        );
    }
    let (base_free, reason, target_config_rows) = match args.stage_mode {
        InfobaseImportStageMode::BaseFree => (true, "asked for (--base-free)".to_string(), -1),
        InfobaseImportStageMode::Patch => (false, "asked for".to_string(), -1),
        InfobaseImportStageMode::Auto => {
            let configuration = tree_configuration_uuid(&stage_args.source_root)?;
            let target = crate::mssql::import_target_state(&stage_args, &configuration)?;
            let base_free = !target.holds_configuration;
            let reason = if target.config_rows == 0 {
                "the target's Config is empty".to_string()
            } else if base_free {
                format!(
                    "the target's Config ({} rows) holds no row of configuration {configuration}",
                    target.config_rows
                )
            } else {
                format!(
                    "the target's Config ({} rows) holds configuration {configuration}",
                    target.config_rows
                )
            };
            (base_free, reason, target.config_rows)
        }
    };
    stage_args.base_free = base_free;
    let report = crate::mssql::stage_source_objects(&stage_args)?;

    Ok(InfobaseConfigImportReport {
        operation: "infobase config import",
        backend: "mssql-configsave-stage",
        format: format_name(config.format),
        source_version: report.source_version.clone(),
        dbms: config.dbms,
        db_server: config.db_server,
        db_name: config.db_name,
        db_user: config.db_user,
        native_config: config.native_config,
        source_dir: stage_args.source_root,
        stage_mode: if base_free { "base-free" } else { "patch" },
        stage_mode_reason: reason,
        target_config_rows,
        staged_rows_before: report.before.row_count,
        staged_rows_after: report.after.row_count,
        scripts: report.scripts,
    })
}

/// The uuid of the configuration a tree describes (its `Configuration.xml`).
fn tree_configuration_uuid(source_root: &Path) -> Result<String> {
    let path = source_root.join("Configuration.xml");
    let xml = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(crate::metadata_model::root::configuration_facts(&xml)
        .with_context(|| format!("failed to read {}", path.display()))?
        .uuid)
}

fn build_import_stage_args(
    config: &ConnectionConfig,
    args: &InfobaseConfigImportArgs,
) -> Result<MssqlStageSourceObjectsArgs> {
    Ok(MssqlStageSourceObjectsArgs {
        server: config.db_server.clone(),
        sql_user: config.db_user.clone(),
        sql_pwd: config.db_pwd.clone(),
        sql_pwd_env: args.db_pwd_env.clone(),
        database: config.db_name.clone(),
        source_root: absolute_path(&args.source_dir)?,
        sqlcmd: args.sqlcmd.clone(),
        replace_config_save: args.replace_config_save,
        allow_non_lab: args.allow_non_lab,
        batch_size: args.batch_size,
        // An import reads the tree's own version unless one was asked for.
        source_version: if config.xml_version_given {
            Some(config.legacy_source_version()?)
        } else {
            None
        },
        path_prefix: args.path_prefix.clone(),
        script_output: args.script_output.clone(),
        script_only: false,
        bulk: false,
        per_row: false,
        bcp_executable: None,
        base_free: matches!(args.stage_mode, InfobaseImportStageMode::BaseFree),
    })
}

pub(crate) fn resolve_connection(request: ConnectionRequest<'_>) -> Result<ConnectionConfig> {
    let settings = match request.settings {
        Some(path) => Some(read_settings(path)?),
        None => None,
    };
    // ---- SETTINGS HAND-OFF (0.3) ------------------------------------------
    // `request.native_config` is the platform's ibcmd configuration file
    // (`--config`/`-c`). It is kept (and reported) but not read yet: the
    // settings layer of the platform track (`crate::settings`) reads the
    // connection from it, below the command line and above the defaults.
    // -----------------------------------------------------------------------
    let native_config = request.native_config.map(Path::to_path_buf);

    let format = request
        .format
        .or_else(|| settings_format(&settings))
        .unwrap_or(InfobaseConfigFormat::Xml);
    // ---- XML VERSION CALL POINT (0.3) -------------------------------------
    // The one place an `infobase config` command decides its XML version:
    // the hidden `--source-version`, else the `--settings` file, else 2.20
    // (platform 8.3.27); an import given neither reads it off the tree
    // (`xml_version_given`). The platform track switches this to
    // `crate::settings::resolve_platform(...)`.
    let legacy_adapter = resolve_legacy_adapter(&settings, request.source_version)?;
    let xml_version_given =
        request.source_version.is_some() || settings_xml_dialect(&settings)?.is_some();
    // -----------------------------------------------------------------------
    let dbms = first_value(request.dbms, settings_value(&settings, "dbms-type"))
        .unwrap_or_else(|| "MSSQLServer".to_string());
    let db_server = first_value(request.db_server, settings_value(&settings, "dbms-server"))
        .unwrap_or_else(|| "localhost".to_string());
    let db_name =
        first_value(request.db_name, settings_value(&settings, "dbms-base")).ok_or_else(|| {
            anyhow!(
                "не указано имя базы данных: передайте --dbms=MSSQLServer, --db-server и --db-name \
                 (файловые информационные базы не поддерживаются в этой версии ibcmd-rs)"
            )
        })?;
    let db_user = first_value(request.db_user, settings_value(&settings, "dbms-user"));
    let (db_pwd, password_source) = match db_user {
        Some(_) => resolve_password(request.db_pwd, &settings, request.db_pwd_env)?,
        None => (None, None),
    };

    Ok(ConnectionConfig {
        dbms,
        db_server,
        db_name,
        db_user,
        db_pwd,
        password_source,
        native_config,
        format,
        legacy_adapter,
        xml_version_given,
    })
}

pub(crate) fn read_settings(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read settings {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

pub(crate) fn settings_value(settings: &Option<Value>, name: &str) -> Option<String> {
    settings
        .as_ref()?
        .get("vrunner")?
        .get(name)?
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn settings_format(settings: &Option<Value>) -> Option<InfobaseConfigFormat> {
    let value = settings_string_at(settings, &["ibcmd-rs", "config-format"])
        .or_else(|| settings_string_at(settings, &["ibcmd-rs", "format"]))
        .or_else(|| settings_string_at(settings, &["format"]))
        .or_else(|| settings_value(settings, "format"))?;
    parse_format(&value)
}

fn resolve_legacy_adapter(
    settings: &Option<Value>,
    cli_source_version: Option<InfobaseConfigSourceVersion>,
) -> Result<MssqlLegacyAdapter> {
    let xml_dialect = match cli_source_version {
        Some(selector) => selector.version_axes().xml_dialect().clone(),
        None => settings_xml_dialect(settings)?.unwrap_or_else(|| {
            XmlDialect::parse(InfobaseConfigSourceVersion::V2_20.as_str())
                .expect("default legacy XML dialect is valid")
        }),
    };
    let version_axes = LegacyVersionAxes::new(
        xml_dialect,
        settings_platform_build(settings)?,
        None,
        None,
        None,
    );
    let legacy_adapter = MssqlLegacyAdapter::new(version_axes)?;
    if legacy_adapter.legacy_selector().is_none() {
        bail!(
            "legacy MSSQL adapter does not support XML dialect {}",
            legacy_adapter.xml_dialect()
        );
    }
    Ok(legacy_adapter)
}

fn settings_xml_dialect(settings: &Option<Value>) -> Result<Option<XmlDialect>> {
    let value = settings_string_at(settings, &["ibcmd-rs", "source-version"])
        .or_else(|| settings_string_at(settings, &["ibcmd-rs", "xml-version"]))
        .or_else(|| settings_string_at(settings, &["ibcmd-rs", "xcf-version"]))
        .or_else(|| settings_string_at(settings, &["source-version"]))
        .or_else(|| settings_string_at(settings, &["xml-version"]))
        .or_else(|| settings_string_at(settings, &["xcf-version"]))
        .or_else(|| settings_value(settings, "source-version"))
        .or_else(|| settings_value(settings, "xml-version"))
        .or_else(|| settings_value(settings, "xcf-version"));
    let Some(value) = value else {
        return Ok(None);
    };
    if let Some(selector) = parse_legacy_source_selector(&value) {
        return Ok(Some(selector.version_axes().xml_dialect().clone()));
    }
    XmlDialect::parse(value.trim())
        .map(Some)
        .map_err(|error| anyhow!("invalid XML dialect `{value}` in settings: {error}"))
}

fn settings_platform_build(settings: &Option<Value>) -> Result<Option<PlatformBuild>> {
    let value = settings_string_at(settings, &["ibcmd-rs", "platform-version"])
        .or_else(|| settings_string_at(settings, &["platform-version"]))
        .or_else(|| settings_value(settings, "platform-version"));
    let Some(value) = value else {
        return Ok(None);
    };
    PlatformBuild::parse(value.trim())
        .map(Some)
        .map_err(|error| anyhow!("invalid platform build `{value}` in settings: {error}"))
}

pub(crate) fn settings_string_at(settings: &Option<Value>, path: &[&str]) -> Option<String> {
    let mut current = settings.as_ref()?;
    for segment in path {
        current = current.get(*segment)?;
    }
    current
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn parse_format(value: &str) -> Option<InfobaseConfigFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "xml" | "ibcmd-xml" | "source-tree" => Some(InfobaseConfigFormat::Xml),
        _ => None,
    }
}

fn parse_legacy_source_selector(value: &str) -> Option<InfobaseConfigSourceVersion> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "2.20" | "20" | "8.3" | "8.3.27" => Some(InfobaseConfigSourceVersion::V2_20),
        "2.21" | "21" | "8.5" | "8.5.1" => Some(InfobaseConfigSourceVersion::V2_21),
        _ if normalized.starts_with("8.3.27.") => Some(InfobaseConfigSourceVersion::V2_20),
        _ if normalized.starts_with("8.5.1.") => Some(InfobaseConfigSourceVersion::V2_21),
        _ => None,
    }
}

pub(crate) fn first_value(cli: Option<&str>, settings: Option<String>) -> Option<String> {
    cli.filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .or(settings)
}

fn resolve_password(
    cli_db_pwd: Option<&str>,
    settings: &Option<Value>,
    db_pwd_env: &str,
) -> Result<(Option<String>, Option<String>)> {
    if let Some(value) = cli_db_pwd.filter(|value| !value.is_empty()) {
        return Ok((Some(value.to_string()), Some("--db-pwd".to_string())));
    }
    if let Ok(value) = env::var(db_pwd_env) {
        return Ok((Some(value), Some(format!("env:{db_pwd_env}"))));
    }
    if let Some(value) = settings_value(settings, "dbms-pwd") {
        return Ok((Some(value), Some("settings".to_string())));
    }
    bail!(
        "не указан пароль пользователя сервера СУБД: передайте --db-pwd (--database-password), -W или переменную окружения {db_pwd_env}"
    )
}

pub(crate) fn ensure_mssql(dbms: &str) -> Result<()> {
    if dbms.eq_ignore_ascii_case("MSSQLServer") || dbms.eq_ignore_ascii_case("MSSQL") {
        return Ok(());
    }
    bail!("unsupported dbms for direct infobase config operation: {dbms}")
}

fn format_name(format: InfobaseConfigFormat) -> &'static str {
    match format {
        InfobaseConfigFormat::Xml => "xml",
    }
}

pub(crate) fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path)
            .with_context(|| format!("failed to resolve {}", path.display()));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

pub(crate) fn prepare_output_dir(path: &Path, overwrite: bool) -> Result<()> {
    if path.exists() {
        if !path.is_dir() {
            bail!(
                "output path exists and is not a directory: {}",
                path.display()
            );
        }
        if fs::read_dir(path)?.next().is_some() && !overwrite {
            return Err(anyhow::Error::new(OutputDirectoryNotEmpty(
                path.to_path_buf(),
            )));
        }
        if overwrite {
            clear_directory(path)?;
        }
    } else {
        fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    }
    Ok(())
}

fn clear_directory(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
fn is_internal_dump_path(relative: &Path) -> bool {
    use std::ffi::OsStr;
    use std::path::Component;

    if relative == Path::new("manifest.json") {
        return true;
    }
    let Some(Component::Normal(first)) = relative.components().next() else {
        return false;
    };
    matches!(
        first,
        name if name == OsStr::new("Config")
            || name == OsStr::new("ConfigSave")
            || name == OsStr::new("Config_inflated")
            || name == OsStr::new("ConfigSave_inflated")
            || name == OsStr::new("Config_module_text")
            || name == OsStr::new("ConfigSave_module_text")
    )
}

fn count_files(root: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::InfobaseConfigImportArgs;
    use std::path::PathBuf;

    #[test]
    fn reads_format_from_top_level_settings() {
        let settings: Option<Value> = Some(serde_json::json!({
            "format": "ibcmd-xml",
            "ibcmd-rs": {
                "source-version": "8.5.1"
            },
            "vrunner": {
                "dbms-base": "servicedesk"
            }
        }));

        assert_eq!(settings_format(&settings), Some(InfobaseConfigFormat::Xml));
        assert_eq!(
            settings_xml_dialect(&settings)
                .unwrap()
                .map(|dialect| dialect.to_string()),
            Some("2.21".to_string())
        );
        assert_eq!(settings_platform_build(&settings).unwrap(), None);
        assert_eq!(
            settings_value(&settings, "dbms-base"),
            Some("servicedesk".to_string())
        );
    }

    #[test]
    fn settings_version_axes_are_separate_and_fail_closed() {
        let default = resolve_legacy_adapter(&None, None).unwrap();
        assert_eq!(default.xml_dialect().to_string(), "2.20");
        assert_eq!(default.version_axes().platform_build(), None);

        let platform_only = Some(serde_json::json!({
            "ibcmd-rs": { "platform-version": "8.5.1.1150" }
        }));
        let resolved = resolve_legacy_adapter(&platform_only, None).unwrap();
        assert_eq!(resolved.xml_dialect().to_string(), "2.20");
        assert_eq!(
            resolved
                .version_axes()
                .platform_build()
                .map(ToString::to_string)
                .as_deref(),
            Some("8.5.1.1150")
        );

        for dialect in ["2.17", "2.99"] {
            let settings = Some(serde_json::json!({
                "ibcmd-rs": { "xml-version": dialect }
            }));
            let error = resolve_legacy_adapter(&settings, None).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("legacy MSSQL adapter does not support XML dialect")
            );
        }

        let malformed_xml = Some(serde_json::json!({
            "ibcmd-rs": { "xml-version": "not-a-version" }
        }));
        assert!(
            resolve_legacy_adapter(&malformed_xml, None)
                .unwrap_err()
                .to_string()
                .contains("invalid XML dialect")
        );

        let malformed_platform = Some(serde_json::json!({
            "ibcmd-rs": { "platform-version": "8.5.invalid" }
        }));
        assert!(
            resolve_legacy_adapter(&malformed_platform, None)
                .unwrap_err()
                .to_string()
                .contains("invalid platform build")
        );
    }

    #[test]
    fn skips_only_internal_dump_roots() {
        assert!(is_internal_dump_path(Path::new("Config/versions.bin")));
        assert!(is_internal_dump_path(Path::new("Config_module_text/a.bsl")));
        assert!(is_internal_dump_path(Path::new("manifest.json")));
        assert!(!is_internal_dump_path(Path::new("Configuration.xml")));
        assert!(!is_internal_dump_path(Path::new(
            "Constants/UseFeature.xml"
        )));
        assert!(!is_internal_dump_path(Path::new("ConfigDumpInfo.xml")));
    }

    fn import_args() -> InfobaseConfigImportArgs {
        InfobaseConfigImportArgs {
            settings: None,
            native_config: None,
            format: Some(InfobaseConfigFormat::Xml),
            source_version: Some(InfobaseConfigSourceVersion::V2_21),
            dbms: Some("MSSQLServer".to_string()),
            db_server: Some("localhost".to_string()),
            db_name: Some("ut_ibcmd".to_string()),
            db_user: Some("sa_import".to_string()),
            db_pwd: Some("secret".to_string()),
            db_pwd_env: "IMPORT_SQL_PWD".to_string(),
            user: None,
            password: None,
            password_env: "IBCMD_USER_PSW".to_string(),
            sqlcmd: PathBuf::from("sqlcmd"),
            replace_config_save: true,
            allow_non_lab: true,
            batch_size: Some(250),
            path_prefix: vec!["Catalogs/Валюты".to_string()],
            script_output: Some(PathBuf::from(r"C:\temp\stage.sql")),
            stage_mode: InfobaseImportStageMode::Auto,
            source_dir: PathBuf::from(r".\fixtures\source"),
        }
    }

    #[test]
    fn builds_import_stage_args_with_sql_auth() {
        let args = import_args();
        let config = resolve_connection(args.connection()).unwrap();
        let stage_args = build_import_stage_args(&config, &args).unwrap();

        assert_eq!(stage_args.server, "localhost");
        assert_eq!(stage_args.sql_user.as_deref(), Some("sa_import"));
        assert_eq!(stage_args.sql_pwd.as_deref(), Some("secret"));
        assert_eq!(stage_args.sql_pwd_env, "IMPORT_SQL_PWD");
        assert_eq!(stage_args.database, "ut_ibcmd");
        assert_eq!(stage_args.batch_size, Some(250));
        assert_eq!(
            stage_args.source_version,
            Some(InfobaseConfigSourceVersion::V2_21)
        );
        assert_eq!(stage_args.path_prefix, vec!["Catalogs/Валюты".to_string()]);
        assert_eq!(
            stage_args.script_output,
            Some(PathBuf::from(r"C:\temp\stage.sql"))
        );
        assert!(!stage_args.base_free);
    }

    #[test]
    fn an_import_without_a_version_reads_the_trees_own() {
        let mut args = import_args();
        args.source_version = None;
        let config = resolve_connection(args.connection()).unwrap();
        assert!(!config.xml_version_given);
        let stage_args = build_import_stage_args(&config, &args).unwrap();
        assert_eq!(stage_args.source_version, None);

        args.stage_mode = InfobaseImportStageMode::BaseFree;
        let stage_args = build_import_stage_args(&config, &args).unwrap();
        assert!(stage_args.base_free);
    }

    #[test]
    fn the_native_config_file_is_kept_for_the_settings_layer() {
        let mut args = import_args();
        args.native_config = Some(PathBuf::from(r"C:\ibcmd\ibcmd.yml"));
        let config = resolve_connection(args.connection()).unwrap();
        assert_eq!(
            config.native_config.as_deref(),
            Some(Path::new(r"C:\ibcmd\ibcmd.yml"))
        );
    }

    #[test]
    fn a_non_empty_output_directory_is_refused_by_type() {
        let root = std::env::temp_dir().join(format!(
            "ibcmd-rs-export-not-empty-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("x.txt"), b"x").unwrap();
        let error = prepare_output_dir(&root, false).unwrap_err();
        assert!(error.downcast_ref::<OutputDirectoryNotEmpty>().is_some());
        assert!(root.join("x.txt").is_file());
        let _ = fs::remove_dir_all(&root);
    }
}
