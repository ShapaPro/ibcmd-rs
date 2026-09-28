//! The platform ibcmd's `infobase` mode, parsed the way the platform's ibcmd
//! parses it (measured against 8.3.27.2214):
//!
//! - after the mode come command words (`config`, `export`, ...), options
//!   and arguments in any order;
//! - an option is `--name=value`, `--name value` or `-x value` (`-xvalue`
//!   is an error), names are case-sensitive;
//! - an option is known only from its own command on: the mode's options
//!   (`--dbms`, `--db-name`, ...) anywhere after the mode, `config`'s
//!   (`--user`, `--password`) after `config`, `export`'s after `export`.
//!
//! The whole native command tree is known, so every native command is
//! recognized: `config export` and `config import` are served, everything
//! else is refused by name (see `super`).

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::cli::InfobaseConfigSourceVersion;
use crate::platform::PlatformSpec;

/// One option of the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Opt {
    // common to every mode
    Help,
    Version,
    Pid,
    Remote,
    // the infobase mode
    Config,
    System,
    Dbms,
    DbServer,
    DbName,
    DbUser,
    DbPwd,
    RequestDbPwd,
    DbPath,
    Data,
    Lock,
    Temp,
    UsersData,
    SessionData,
    SttData,
    LogData,
    FtextData,
    Ftext2Data,
    OpenidData,
    BinDataStrg,
    ClntNotifData,
    WebsocketData,
    // `config`
    User,
    Password,
    // `config export`
    Base,
    File,
    Extension,
    Sync,
    Force,
    Threads,
    Archive,
    IgnoreUnresolvedRefs,
    // `config import`
    Out,
    // ibcmd-rs's own (no native spelling)
    Report,
    Platform,
    Sqlcmd,
    Settings,
    SourceVersion,
    DbPwdEnv,
    BaseFree,
}

/// How an option is spelled and whether it takes a value.
#[derive(Debug, Clone, Copy)]
pub struct OptSpec {
    pub opt: Opt,
    /// Long names, the first is the one messages use.
    pub long: &'static [&'static str],
    pub short: Option<char>,
    pub value: bool,
}

const fn flag(opt: Opt, long: &'static [&'static str], short: Option<char>) -> OptSpec {
    OptSpec {
        opt,
        long,
        short,
        value: false,
    }
}

const fn valued(opt: Opt, long: &'static [&'static str], short: Option<char>) -> OptSpec {
    OptSpec {
        opt,
        long,
        short,
        value: true,
    }
}

/// `ibcmd --help`, `--version`, `--pid`, `--remote`: every mode has them.
pub const GLOBAL_OPTIONS: &[OptSpec] = &[
    flag(Opt::Help, &["help"], Some('h')),
    flag(Opt::Help, &[], Some('?')),
    flag(Opt::Version, &["version"], Some('v')),
    valued(Opt::Pid, &["pid"], Some('p')),
    valued(Opt::Remote, &["remote"], Some('r')),
];

/// The infobase mode's own parameters, plus ibcmd-rs's.
pub const MODE_OPTIONS: &[OptSpec] = &[
    valued(Opt::Config, &["config"], Some('c')),
    valued(Opt::System, &["system"], None),
    valued(Opt::Dbms, &["dbms"], None),
    valued(Opt::DbServer, &["database-server", "db-server"], None),
    valued(Opt::DbName, &["database-name", "db-name"], None),
    valued(Opt::DbUser, &["database-user", "db-user"], None),
    valued(Opt::DbPwd, &["database-password", "db-pwd"], None),
    flag(
        Opt::RequestDbPwd,
        &["request-database-password", "request-db-pwd"],
        Some('W'),
    ),
    valued(Opt::DbPath, &["database-path", "db-path"], None),
    valued(Opt::Data, &["data"], Some('d')),
    valued(Opt::Lock, &["lock"], None),
    valued(Opt::Temp, &["temp"], Some('t')),
    valued(Opt::UsersData, &["users-data"], None),
    valued(Opt::SessionData, &["session-data"], None),
    valued(Opt::SttData, &["stt-data"], None),
    valued(Opt::LogData, &["log-data"], None),
    valued(Opt::FtextData, &["ftext-data"], None),
    valued(Opt::Ftext2Data, &["ftext2-data"], None),
    valued(Opt::OpenidData, &["openid-data"], None),
    valued(Opt::BinDataStrg, &["bin-data-strg"], None),
    valued(Opt::ClntNotifData, &["clnt-notif-data"], None),
    valued(Opt::WebsocketData, &["websocket-data"], None),
    valued(Opt::Report, &["report"], None),
    valued(Opt::Sqlcmd, &["sqlcmd"], None),
    valued(Opt::Settings, &["settings"], None),
    valued(Opt::Platform, &["platform"], None),
    valued(Opt::SourceVersion, &["source-version"], None),
    valued(Opt::DbPwdEnv, &["db-pwd-env"], None),
];

const CONFIG_OPTIONS: &[OptSpec] = &[
    valued(Opt::User, &["user"], Some('u')),
    valued(Opt::Password, &["password"], Some('P')),
];

const EXPORT_OPTIONS: &[OptSpec] = &[
    valued(Opt::Base, &["base"], Some('b')),
    valued(Opt::File, &["file"], Some('f')),
    valued(Opt::Extension, &["extension"], Some('e')),
    flag(Opt::Sync, &["sync"], None),
    flag(Opt::Force, &["force"], None),
    valued(Opt::Threads, &["threads"], Some('T')),
    flag(Opt::Archive, &["archive"], Some('A')),
    flag(Opt::IgnoreUnresolvedRefs, &["ignore-unresolved-refs"], None),
];

const IMPORT_OPTIONS: &[OptSpec] = &[
    valued(Opt::Out, &["out"], Some('o')),
    valued(Opt::Extension, &["extension"], Some('e')),
    flag(Opt::BaseFree, &["base-free"], None),
];

/// What a command word leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Needs one of its subcommands.
    Group,
    /// `infobase config export`.
    Export,
    /// `infobase config import`.
    Import,
    /// A native command ibcmd-rs does not implement yet.
    Unsupported,
}

/// One command of the tree.
#[derive(Debug)]
pub struct Node {
    pub name: &'static str,
    /// The platform's own one-line description.
    pub summary: &'static str,
    pub kind: NodeKind,
    pub options: &'static [OptSpec],
    pub children: &'static [Node],
}

const fn unsupported(name: &'static str, summary: &'static str) -> Node {
    Node {
        name,
        summary,
        kind: NodeKind::Unsupported,
        options: &[],
        children: &[],
    }
}

/// The infobase mode as the platform's `ibcmd help infobase` lists it.
pub static INFOBASE: Node = Node {
    name: "infobase",
    summary: "Режим управления информационной базой",
    kind: NodeKind::Group,
    options: MODE_OPTIONS,
    children: &[
        unsupported("create", "Создание информационной базы"),
        unsupported("dump", "Выгрузка данных информационной базы"),
        unsupported("restore", "Загрузка данных информационной базы"),
        unsupported("clear", "Очистка информационной базы"),
        unsupported("replicate", "Репликация информационной базы"),
        Node {
            name: "config",
            summary: "Управление конфигурацией информационной базы",
            kind: NodeKind::Group,
            options: CONFIG_OPTIONS,
            children: &[
                unsupported("load", "Загрузка конфигурации"),
                unsupported("save", "Выгрузка конфигурации"),
                unsupported("check", "Проверка конфигурации"),
                unsupported("apply", "Обновление конфигурации базы данных"),
                unsupported("reset", "Возврат к конфигурации базы данных"),
                unsupported(
                    "repair",
                    "Восстановление конфигурации после незавершенной операции",
                ),
                Node {
                    name: "export",
                    summary: "Экспорт конфигурации в XML",
                    kind: NodeKind::Export,
                    options: EXPORT_OPTIONS,
                    children: &[
                        unsupported(
                            "info",
                            "Вывести информацию о состоянии конфигурации (ConfigDumpInfo)",
                        ),
                        unsupported(
                            "status",
                            "Вывести информацию о изменениях конфигурации относительно переданного состояния (ConfigDumpInfo)",
                        ),
                        unsupported("objects", "Экспорт выбранных объектов конфигурации в XML"),
                        unsupported(
                            "all-extensions",
                            "Экспорт всех расширений конфигурации в XML",
                        ),
                    ],
                },
                Node {
                    name: "import",
                    summary: "Импорт конфигурации из XML",
                    kind: NodeKind::Import,
                    options: IMPORT_OPTIONS,
                    children: &[
                        unsupported("files", "Импорт выбранных файлов конфигурации из XML"),
                        unsupported(
                            "all-extensions",
                            "Импорт всех расширений конфигурации из XML",
                        ),
                    ],
                },
                Node {
                    name: "support",
                    summary: "Настройка поддержки конфигурации",
                    kind: NodeKind::Unsupported,
                    options: &[],
                    children: &[unsupported("disable", "Снять конфигурацию с поддержки")],
                },
                Node {
                    name: "data-separation",
                    summary: "Получение информации о разделителях информационной базы",
                    kind: NodeKind::Unsupported,
                    options: &[],
                    children: &[unsupported(
                        "list",
                        "Вывод списка разделителей информационной базы",
                    )],
                },
                Node {
                    name: "extension",
                    summary: "Управление расширениями конфигурации",
                    kind: NodeKind::Unsupported,
                    options: &[],
                    children: &[
                        unsupported("create", "Создание расширения"),
                        unsupported("info", "Получить информацию о расширении"),
                        unsupported("list", "Получить список расширений"),
                        unsupported("update", "Установить указанные значения свойств расширения"),
                        unsupported("delete", "Удаление расширения"),
                    ],
                },
                unsupported(
                    "generation-id",
                    "Получить идентификатор поколения конфигурации",
                ),
                unsupported("sign", "Цифровая подпись конфигурации/расширения"),
            ],
        },
    ],
};

/// The platform ibcmd's other modes (`ibcmd help`), none served yet.
pub const OTHER_MODES: &[(&str, &str)] = &[
    ("server", "Режим настройки автономного сервера"),
    ("eventlog", "Режим работы с журналом регистрации"),
    ("config", "Режим работы с конфигурациями и расширениями"),
    ("extension", "Режим работы с расширениями"),
    ("mobile-app", "Режим работы с мобильным приложением"),
    ("mobile-client", "Режим работы с мобильным клиентом"),
    (
        "session",
        "Режим администрирования сеансов информационных баз",
    ),
    ("lock", "Режим администрирования блокировок"),
    (
        "binary-data-storage",
        "Режим администрирования хранилищ двоичных данных",
    ),
];

/// A command line refused before anything runs; `super::refusal_exit_code`
/// gives its exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// An unknown option, a value where none is taken, an extra argument.
    Parse(String),
    /// An option without its value, a command without its path.
    MissingValue(String),
    /// A command word that needs one of its subcommands.
    Incomplete { path: Vec<&'static str> },
    /// A native command ibcmd-rs does not implement yet.
    UnsupportedCommand(String),
    /// A native option of a served command that ibcmd-rs does not implement.
    UnsupportedOption { option: String, command: String },
    /// `--pid`/`--remote`: a running standalone server.
    UnsupportedServer(String),
    /// A DBMS the platform knows and ibcmd-rs does not serve.
    UnsupportedDbms(String),
    /// A DBMS the platform does not know either.
    UnknownDbms(String),
    /// No `--dbms`: the platform would open a file infobase.
    FileInfobase,
    /// An import from a `.zip`.
    ImportArchive(PathBuf),
    /// A value of the wrong shape.
    InvalidValue { option: String, value: String },
    /// Two options that exclude each other.
    Conflict { first: String, second: String },
}

/// The connection and the other options every served command shares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Common {
    /// `--config`/`-c`: the platform ibcmd's configuration file.
    pub native_config: Option<PathBuf>,
    pub dbms: Option<String>,
    pub db_server: Option<String>,
    pub db_name: Option<String>,
    pub db_user: Option<String>,
    pub db_pwd: Option<String>,
    /// `-W`: read the database password from STDIN.
    pub request_db_pwd: bool,
    pub data: Option<PathBuf>,
    pub temp: Option<PathBuf>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub report: Option<PathBuf>,
    pub sqlcmd: Option<PathBuf>,
    pub settings: Option<PathBuf>,
    /// `--platform` (ibcmd-rs's own): the platform whose XML format is read
    /// or written, a release (`8.3.27`) or an exact build of the registry.
    pub platform: Option<PlatformSpec>,
    pub source_version: Option<InfobaseConfigSourceVersion>,
    pub db_pwd_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    pub common: Common,
    pub threads: Option<usize>,
    /// The directory as given.
    pub path: OsString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    pub common: Common,
    pub base_free: bool,
    /// The directory as given.
    pub path: OsString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Help,
    Version,
    Export(ExportRequest),
    Import(ImportRequest),
}

/// The tokens of one command line, resolved against the tree.
struct Scan {
    /// The command path from the mode down (`["config", "export"]`).
    path: Vec<&'static Node>,
    /// Options in order: which one, how it was spelled, its value.
    options: Vec<(Opt, String, Option<String>)>,
    /// Arguments of the last command.
    arguments: Vec<OsString>,
    /// The first unknown option or option without its value.
    error: Option<Refusal>,
}

impl Scan {
    fn node(&self) -> &'static Node {
        self.path.last().copied().unwrap_or(&INFOBASE)
    }

    fn command(&self) -> String {
        std::iter::once("infobase")
            .chain(self.path.iter().map(|node| node.name))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn has(&self, opt: Opt) -> bool {
        self.options.iter().any(|(found, _, _)| *found == opt)
    }

    /// The last value given for an option.
    fn value(&self, opt: Opt) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(found, _, _)| *found == opt)
            .and_then(|(_, _, value)| value.as_deref())
    }

    fn spelled(&self, opt: Opt) -> Option<&str> {
        self.options
            .iter()
            .find(|(found, _, _)| *found == opt)
            .map(|(_, spelled, _)| spelled.as_str())
    }
}

/// The options in force at a command: its own, its ancestors', the mode's
/// and the global ones, innermost first.
fn lookup<'a>(
    path: &[&'static Node],
    matches: impl Fn(&OptSpec) -> bool + 'a,
) -> Option<&'static OptSpec> {
    path.iter()
        .rev()
        .flat_map(|node| node.options.iter())
        .chain(INFOBASE.options.iter())
        .chain(GLOBAL_OPTIONS.iter())
        .find(|spec| matches(spec))
}

fn scan(args: &[OsString]) -> Scan {
    let mut scan = Scan {
        path: Vec::new(),
        options: Vec::new(),
        arguments: Vec::new(),
        error: None,
    };
    let mut index = 0;
    while index < args.len() {
        let raw = &args[index];
        index += 1;
        let text = raw.to_string_lossy();
        if text.len() > 1 && text.starts_with('-') {
            let (spec, spelled, attached) = if let Some(long) = text.strip_prefix("--") {
                let (name, attached) = match long.split_once('=') {
                    Some((name, value)) => (name, Some(value.to_string())),
                    None => (long, None),
                };
                let spec = (!name.is_empty())
                    .then(|| lookup(&scan.path, |spec| spec.long.contains(&name)))
                    .flatten();
                (spec, format!("--{name}"), attached)
            } else {
                let mut chars = text.chars().skip(1);
                let short = chars.next();
                let spec = match (short, chars.next()) {
                    (Some(short), None) => lookup(&scan.path, |spec| spec.short == Some(short)),
                    _ => None,
                };
                (spec, text.to_string(), None)
            };
            let Some(spec) = spec else {
                scan.error.get_or_insert(Refusal::Parse(text.to_string()));
                continue;
            };
            let value = if spec.value {
                match attached {
                    Some(value) => Some(value),
                    None if index < args.len() => {
                        index += 1;
                        Some(args[index - 1].to_string_lossy().into_owned())
                    }
                    None => {
                        let name = spec
                            .long
                            .first()
                            .map(|name| name.to_string())
                            .unwrap_or_else(|| spelled.clone());
                        scan.error.get_or_insert(Refusal::MissingValue(name));
                        continue;
                    }
                }
            } else {
                if attached.is_some() {
                    scan.error.get_or_insert(Refusal::Parse(text.to_string()));
                    continue;
                }
                None
            };
            scan.options.push((spec.opt, spelled, value));
            continue;
        }
        // A command word descends while no argument has been given at this
        // level; anything else is an argument of the command reached.
        let node = scan.node();
        if scan.arguments.is_empty()
            && let Some(child) = node
                .children
                .iter()
                .find(|child| OsStr::new(child.name) == raw)
        {
            scan.path.push(child);
            continue;
        }
        scan.arguments.push(raw.clone());
    }
    scan
}

/// Parses the arguments that follow `infobase`.
pub fn parse_infobase(args: &[OsString]) -> Result<Invocation, Refusal> {
    let scan = scan(args);
    if scan.has(Opt::Help) {
        return Ok(Invocation::Help);
    }
    if scan.has(Opt::Version) {
        return Ok(Invocation::Version);
    }
    let node = scan.node();
    match node.kind {
        NodeKind::Unsupported => return Err(Refusal::UnsupportedCommand(scan.command())),
        NodeKind::Group => {
            if let Some(error) = scan.error {
                return Err(error);
            }
            return Err(Refusal::Incomplete {
                path: scan.path.iter().map(|node| node.name).collect(),
            });
        }
        NodeKind::Export | NodeKind::Import => {}
    }
    if let Some(error) = scan.error.clone() {
        return Err(error);
    }
    for opt in [Opt::Pid, Opt::Remote] {
        if let Some(spelled) = scan.spelled(opt) {
            return Err(Refusal::UnsupportedServer(spelled.to_string()));
        }
    }
    let unsupported_options: &[Opt] = match node.kind {
        NodeKind::Export => &[
            Opt::Base,
            Opt::File,
            Opt::Extension,
            Opt::Sync,
            Opt::Archive,
        ],
        _ => &[Opt::Out, Opt::Extension],
    };
    for opt in unsupported_options {
        if let Some(spelled) = scan.spelled(*opt) {
            return Err(Refusal::UnsupportedOption {
                option: spelled.to_string(),
                command: scan.command(),
            });
        }
    }
    let common = common(&scan)?;
    let path = match scan.arguments.as_slice() {
        [] => {
            return Err(Refusal::MissingValue(
                match node.kind {
                    NodeKind::Export => "путь к каталогу файлов конфигурации",
                    _ => "путь к каталогу или архиву с файлами конфигурации",
                }
                .to_string(),
            ));
        }
        [path] => path.clone(),
        [_, extra, ..] => return Err(Refusal::Parse(extra.to_string_lossy().into_owned())),
    };
    Ok(match node.kind {
        NodeKind::Export => Invocation::Export(ExportRequest {
            common,
            threads: match scan.value(Opt::Threads) {
                None => None,
                Some(value) => Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .filter(|threads| *threads > 0)
                        .ok_or_else(|| Refusal::InvalidValue {
                            option: "--threads".to_string(),
                            value: value.to_string(),
                        })?,
                ),
            },
            path,
        }),
        _ => {
            let as_path = PathBuf::from(&path);
            if as_path.is_file() {
                return Err(Refusal::ImportArchive(as_path));
            }
            Invocation::Import(ImportRequest {
                common,
                base_free: scan.has(Opt::BaseFree),
                path,
            })
        }
    })
}

/// The DBMS the platform knows: MSSQLServer is served, the rest refused.
const KNOWN_DBMS: &[&str] = &["MSSQLServer", "PostgreSQL", "IBMDB2", "OracleDatabase"];

fn common(scan: &Scan) -> Result<Common, Refusal> {
    let text = |opt| scan.value(opt).map(str::to_string);
    let path = |opt| scan.value(opt).map(PathBuf::from);
    if scan.has(Opt::DbPath) {
        return Err(Refusal::FileInfobase);
    }
    let dbms = match scan.value(Opt::Dbms) {
        Some(value) => {
            let known = KNOWN_DBMS
                .iter()
                .find(|known| known.eq_ignore_ascii_case(value.trim()));
            match known {
                Some(&"MSSQLServer") => Some("MSSQLServer".to_string()),
                Some(other) => return Err(Refusal::UnsupportedDbms(other.to_string())),
                None => return Err(Refusal::UnknownDbms(value.to_string())),
            }
        }
        None => None,
    };
    // Without --dbms the platform opens a file infobase. Here a server
    // database may still come from the settings (a settings file, `--config`,
    // the environment), so a missing one is the connection's to report
    // (`infobase::resolve_connection`), not the parser's.
    let source_version = match scan.value(Opt::SourceVersion) {
        None => None,
        Some(value) => Some(
            <InfobaseConfigSourceVersion as clap::ValueEnum>::from_str(value.trim(), true)
                .map_err(|_| Refusal::InvalidValue {
                    option: "--source-version".to_string(),
                    value: value.to_string(),
                })?,
        ),
    };
    let platform = match scan.value(Opt::Platform) {
        None => None,
        Some(value) => {
            Some(
                crate::platform::parse(value.trim()).map_err(|_| Refusal::InvalidValue {
                    option: "--platform".to_string(),
                    value: format!("{value} (известные версии: {})", known_platforms()),
                })?,
            )
        }
    };
    if platform.is_some() && source_version.is_some() {
        return Err(Refusal::Conflict {
            first: "--platform".to_string(),
            second: "--source-version".to_string(),
        });
    }
    Ok(Common {
        native_config: path(Opt::Config),
        dbms,
        db_server: text(Opt::DbServer),
        db_name: text(Opt::DbName),
        db_user: text(Opt::DbUser),
        db_pwd: text(Opt::DbPwd),
        request_db_pwd: scan.has(Opt::RequestDbPwd),
        data: path(Opt::Data),
        temp: path(Opt::Temp),
        user: text(Opt::User),
        password: text(Opt::Password),
        report: path(Opt::Report),
        sqlcmd: path(Opt::Sqlcmd),
        settings: path(Opt::Settings),
        platform,
        source_version,
        db_pwd_env: text(Opt::DbPwdEnv),
    })
}

/// The releases and builds of the platform registry, for a refused
/// `--platform`.
fn known_platforms() -> String {
    crate::platform::known()
        .map(|known| {
            known
                .iter()
                .map(|spec| spec.display())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Every command path of the tree, with its kind (for the tests and the
/// help): `["config", "export", "info"]` and so on.
pub fn command_paths() -> Vec<(Vec<&'static str>, NodeKind)> {
    fn walk(
        node: &'static Node,
        prefix: &mut Vec<&'static str>,
        out: &mut Vec<(Vec<&'static str>, NodeKind)>,
    ) {
        for child in node.children {
            prefix.push(child.name);
            out.push((prefix.clone(), child.kind));
            walk(child, prefix, out);
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(&INFOBASE, &mut Vec::new(), &mut out);
    out
}

/// The node a command path leads to.
pub fn node_at(path: &[&str]) -> Option<&'static Node> {
    let mut node = &INFOBASE;
    for name in path {
        node = node.children.iter().find(|child| child.name == *name)?;
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn parse(list: &[&str]) -> Result<Invocation, Refusal> {
        parse_infobase(&args(list))
    }

    fn export(list: &[&str]) -> ExportRequest {
        match parse(list) {
            Ok(Invocation::Export(request)) => request,
            other => panic!("{list:?}: {other:?}"),
        }
    }

    fn import(list: &[&str]) -> ImportRequest {
        match parse(list) {
            Ok(Invocation::Import(request)) => request,
            other => panic!("{list:?}: {other:?}"),
        }
    }

    #[test]
    fn the_native_export_line_of_the_lab_scripts_parses() {
        // scripts/empty-load/run_empty.ps1 and scripts/timing/run_timing.ps1
        let request = export(&[
            "config",
            "export",
            "--dbms=MSSQLServer",
            "--db-server=localhost",
            "--db-name=ibcmd_rs_empty_bsp8327_ours_20260925_t1",
            r"--data=F:\ibcmd\lab\model\empty\run\ibdata",
            "--user=Администратор",
            "--force",
            r"F:\ibcmd\lab\model\empty\run\export",
        ]);
        assert_eq!(request.common.dbms.as_deref(), Some("MSSQLServer"));
        assert_eq!(request.common.db_server.as_deref(), Some("localhost"));
        assert_eq!(
            request.common.db_name.as_deref(),
            Some("ibcmd_rs_empty_bsp8327_ours_20260925_t1")
        );
        assert_eq!(
            request.common.data,
            Some(PathBuf::from(r"F:\ibcmd\lab\model\empty\run\ibdata"))
        );
        assert_eq!(request.common.user.as_deref(), Some("Администратор"));
        assert_eq!(
            request.path,
            OsString::from(r"F:\ibcmd\lab\model\empty\run\export")
        );
        assert_eq!(request.threads, None);
    }

    #[test]
    fn the_native_import_line_of_the_lab_scripts_parses() {
        // scripts/timing/run_timing.ps1: `infobase config import @na <tree>`
        let request = import(&[
            "config",
            "import",
            "--dbms=MSSQLServer",
            "--db-server=localhost",
            "--db-name=ibcmd_rs_tm_bsp_20260924",
            r"--data=F:\ibcmd\lab\timing\out\native_data_load_bsp8327_t1",
            "--user=Администратор",
            r"F:\ibcmd\lab\parity\bsp\native",
        ]);
        assert_eq!(
            request.common.db_name.as_deref(),
            Some("ibcmd_rs_tm_bsp_20260924")
        );
        assert_eq!(
            request.path,
            OsString::from(r"F:\ibcmd\lab\parity\bsp\native")
        );
        assert!(!request.base_free);
    }

    #[test]
    fn the_platform_is_one_the_registry_knows() {
        let request = export(&[
            "config",
            "export",
            "--dbms=MSSQLServer",
            "--db-name=b",
            "--platform=8.5.1",
            "out",
        ]);
        assert_eq!(
            request.common.platform.map(|platform| platform.display()),
            Some("8.5.1")
        );
        let request = import(&["config", "import", "--platform", "8.3.27.2214", "tree"]);
        assert_eq!(
            request.common.platform.map(|platform| platform.display()),
            Some("8.3.27.2214")
        );
        match parse(&["config", "export", "--platform=8.4", "out"]) {
            Err(Refusal::InvalidValue { option, value }) => {
                assert_eq!(option, "--platform");
                assert!(value.starts_with("8.4 (известные версии: "), "{value}");
                assert!(value.contains("8.5.1"), "{value}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse(&[
                "config",
                "export",
                "--platform=8.3.27",
                "--source-version=2.20",
                "out",
            ]),
            Err(Refusal::Conflict {
                first: "--platform".to_string(),
                second: "--source-version".to_string(),
            })
        );
    }

    #[test]
    fn both_spellings_and_both_value_forms_are_accepted() {
        let long = export(&[
            "config",
            "export",
            "--dbms",
            "MSSQLServer",
            "--database-server",
            "sql01",
            "--database-name",
            "base",
            "--database-user",
            "sa",
            "--database-password",
            "secret",
            "out",
        ]);
        let short = export(&[
            "config",
            "export",
            "--dbms=MSSQLServer",
            "--db-server=sql01",
            "--db-name=base",
            "--db-user=sa",
            "--db-pwd=secret",
            "out",
        ]);
        assert_eq!(long, short);
        assert_eq!(long.common.db_user.as_deref(), Some("sa"));
        assert_eq!(long.common.db_pwd.as_deref(), Some("secret"));

        let request = export(&[
            "config",
            "export",
            "-c",
            r"C:\ibcmd\ibcmd.yml",
            "-d",
            r"C:\data",
            "-t",
            r"C:\temp",
            "-W",
            "-T",
            "4",
            "--db-name",
            "base",
            "out",
        ]);
        assert_eq!(
            request.common.native_config,
            Some(PathBuf::from(r"C:\ibcmd\ibcmd.yml"))
        );
        assert_eq!(request.common.data, Some(PathBuf::from(r"C:\data")));
        assert_eq!(request.common.temp, Some(PathBuf::from(r"C:\temp")));
        assert!(request.common.request_db_pwd);
        assert_eq!(request.threads, Some(4));
        for spelling in ["--request-db-pwd", "--request-database-password"] {
            let request = export(&["config", "export", spelling, "--db-name=b", "out"]);
            assert!(request.common.request_db_pwd);
        }
        let request = export(&["config", "export", "--config=cfg.yml", "--db-name=b", "out"]);
        assert_eq!(request.common.native_config, Some(PathBuf::from("cfg.yml")));
        let request = export(&["config", "export", "--threads=8", "--db-name=b", "out"]);
        assert_eq!(request.threads, Some(8));
    }

    #[test]
    fn mode_options_go_anywhere_and_command_options_after_their_command() {
        let request = export(&[
            "--dbms=MSSQLServer",
            "config",
            "--db-name=base",
            "export",
            "out",
            "--db-server=sql01",
        ]);
        assert_eq!(request.common.db_name.as_deref(), Some("base"));
        assert_eq!(request.common.db_server.as_deref(), Some("sql01"));
        // `config`'s own options after `config`
        let request = export(&[
            "config",
            "-u",
            "Админ",
            "-P",
            "",
            "export",
            "--db-name=b",
            "o",
        ]);
        assert_eq!(request.common.user.as_deref(), Some("Админ"));
        assert_eq!(request.common.password.as_deref(), Some(""));
        // an `export` option before `export` is unknown there, as natively
        assert_eq!(
            parse(&["config", "--threads", "4", "export", "--db-name=b", "o"]),
            Err(Refusal::Parse("--threads".to_string()))
        );
        assert_eq!(
            parse(&["--force", "config", "export", "--db-name=b", "o"]),
            Err(Refusal::Parse("--force".to_string()))
        );
    }

    #[test]
    fn the_data_directory_options_are_accepted() {
        let mut list = vec!["config", "export", "--db-name=b"];
        let options = [
            "--system=s",
            "--lock=l",
            "--users-data=u",
            "--session-data=s",
            "--stt-data=s",
            "--log-data=l",
            "--ftext-data=f",
            "--ftext2-data=f",
            "--openid-data=o",
            "--bin-data-strg=b",
            "--clnt-notif-data=c",
            "--websocket-data=w",
            "--ignore-unresolved-refs",
            "--force",
        ];
        list.extend(options);
        list.push("out");
        let request = export(&list);
        assert_eq!(request.path, OsString::from("out"));
    }

    #[test]
    fn parse_errors_name_the_argument_as_natively() {
        assert_eq!(
            parse(&["config", "export", "--bogus", "--db-name=b", "o"]),
            Err(Refusal::Parse("--bogus".to_string()))
        );
        assert_eq!(
            parse(&["config", "export", "--DBMS=MSSQLServer", "--db-name=b", "o"]),
            Err(Refusal::Parse("--DBMS=MSSQLServer".to_string()))
        );
        assert_eq!(
            parse(&["config", "export", "-T4", "--db-name=b", "o"]),
            Err(Refusal::Parse("-T4".to_string()))
        );
        assert_eq!(
            parse(&["config", "export", "--force=yes", "--db-name=b", "o"]),
            Err(Refusal::Parse("--force=yes".to_string()))
        );
        assert_eq!(
            parse(&["config", "export", "--db-name=b", "o", "--db-server"]),
            Err(Refusal::MissingValue("database-server".to_string()))
        );
        assert_eq!(
            parse(&["config", "export", "--db-name=b", "a", "b"]),
            Err(Refusal::Parse("b".to_string()))
        );
        assert_eq!(
            parse(&["config", "export", "--db-name=b"]),
            Err(Refusal::MissingValue(
                "путь к каталогу файлов конфигурации".to_string()
            ))
        );
        assert_eq!(
            parse(&["config", "export", "--threads=0", "--db-name=b", "o"]),
            Err(Refusal::InvalidValue {
                option: "--threads".to_string(),
                value: "0".to_string()
            })
        );
        assert_eq!(
            parse(&[
                "config",
                "export",
                "--source-version=3.0",
                "--db-name=b",
                "o"
            ]),
            Err(Refusal::InvalidValue {
                option: "--source-version".to_string(),
                value: "3.0".to_string()
            })
        );
    }

    #[test]
    fn the_dbms_is_checked_as_natively() {
        let request = export(&["config", "export", "--dbms=mssqlserver", "--db-name=b", "o"]);
        assert_eq!(request.common.dbms.as_deref(), Some("MSSQLServer"));
        for dbms in ["PostgreSQL", "IBMDB2", "OracleDatabase"] {
            assert_eq!(
                parse(&["config", "export", &format!("--dbms={dbms}"), "o"]),
                Err(Refusal::UnsupportedDbms(dbms.to_string()))
            );
        }
        assert_eq!(
            parse(&["config", "export", "--dbms=Foo", "o"]),
            Err(Refusal::UnknownDbms("Foo".to_string()))
        );
        // no --dbms and no database: left to the connection, which may find
        // one in the settings (and says so when it does not)
        assert!(matches!(
            parse(&["config", "export", "o"]),
            Ok(Invocation::Export(_))
        ));
        assert_eq!(
            parse(&["config", "export", "--db-path=C:\\ib", "o"]),
            Err(Refusal::FileInfobase)
        );
        assert_eq!(
            parse(&["config", "import", "--database-path", "C:\\ib", "o"]),
            Err(Refusal::FileInfobase)
        );
        // a database name alone means the server database
        let request = export(&["config", "export", "--db-name=b", "o"]);
        assert_eq!(request.common.dbms, None);
    }

    #[test]
    fn unsupported_options_of_served_commands_are_named() {
        for option in [
            "--base=dump.xml",
            "-b",
            "--file=a.cf",
            "--extension=E",
            "-e",
            "--sync",
            "--archive",
            "-A",
        ] {
            let mut list = vec!["config", "export", "--db-name=b"];
            list.push(option);
            if matches!(option, "-b" | "-e") {
                list.push("value");
            }
            list.push("out");
            match parse(&list) {
                Err(Refusal::UnsupportedOption {
                    option: named,
                    command,
                }) => {
                    assert_eq!(command, "infobase config export");
                    assert!(option.starts_with(&named), "{option} named {named}");
                }
                other => panic!("{option}: {other:?}"),
            }
        }
        for option in ["--out=a.cf", "--extension=E"] {
            match parse(&["config", "import", "--db-name=b", option, "tree"]) {
                Err(Refusal::UnsupportedOption { command, .. }) => {
                    assert_eq!(command, "infobase config import")
                }
                other => panic!("{option}: {other:?}"),
            }
        }
        for option in ["--pid=12", "--remote=http://host:1545", "-r", "-p"] {
            let mut list = vec!["config", "export", "--db-name=b", option];
            if option.len() == 2 {
                list.push("value");
            }
            list.push("out");
            assert!(
                matches!(parse(&list), Err(Refusal::UnsupportedServer(_))),
                "{option}"
            );
        }
    }

    #[test]
    fn every_native_command_is_recognized() {
        let paths = command_paths();
        for (path, kind) in &paths {
            let result = parse(&path.iter().copied().collect::<Vec<_>>());
            match kind {
                NodeKind::Unsupported => assert_eq!(
                    result,
                    Err(Refusal::UnsupportedCommand(format!(
                        "infobase {}",
                        path.join(" ")
                    ))),
                    "{path:?}"
                ),
                NodeKind::Group => assert!(
                    matches!(result, Err(Refusal::Incomplete { .. })),
                    "{path:?}: {result:?}"
                ),
                // served: without its path it asks for one
                NodeKind::Export | NodeKind::Import => assert!(
                    matches!(result, Err(Refusal::MissingValue(_))),
                    "{path:?}: {result:?}"
                ),
            }
        }
        let names = paths
            .iter()
            .map(|(path, _)| path.join(" "))
            .collect::<Vec<_>>();
        for expected in [
            "create",
            "dump",
            "restore",
            "clear",
            "replicate",
            "config load",
            "config save",
            "config check",
            "config apply",
            "config reset",
            "config repair",
            "config export info",
            "config export status",
            "config export objects",
            "config export all-extensions",
            "config import files",
            "config import all-extensions",
            "config support",
            "config support disable",
            "config data-separation",
            "config data-separation list",
            "config extension",
            "config extension create",
            "config extension info",
            "config extension list",
            "config extension update",
            "config extension delete",
            "config generation-id",
            "config sign",
        ] {
            assert!(names.iter().any(|name| name == expected), "{expected}");
        }
    }

    #[test]
    fn unsupported_commands_are_refused_whatever_their_options() {
        assert_eq!(
            parse(&[
                "config",
                "apply",
                "--dbms=MSSQLServer",
                "--db-server=localhost",
                "--db-name=b",
                "--force",
                "--dynamic=disable",
                "--session-terminate=force",
            ]),
            Err(Refusal::UnsupportedCommand(
                "infobase config apply".to_string()
            ))
        );
        assert_eq!(
            parse(&[
                "create",
                "--dbms=MSSQLServer",
                "--db-name=b",
                "--locale=ru_RU",
                "--create-database",
                "--import=tree",
                "--apply",
                "--force",
            ]),
            Err(Refusal::UnsupportedCommand("infobase create".to_string()))
        );
        assert_eq!(
            parse(&["config", "save", "--db-name=b", "--user=Админ", "a.cf"]),
            Err(Refusal::UnsupportedCommand(
                "infobase config save".to_string()
            ))
        );
        assert_eq!(
            parse(&["config", "export", "objects", "-r", "Catalog.A"]),
            Err(Refusal::UnsupportedCommand(
                "infobase config export objects".to_string()
            ))
        );
    }

    #[test]
    fn a_group_without_its_command_is_incomplete() {
        assert_eq!(parse(&[]), Err(Refusal::Incomplete { path: vec![] }));
        assert_eq!(
            parse(&["config"]),
            Err(Refusal::Incomplete {
                path: vec!["config"]
            })
        );
        assert_eq!(
            parse(&["config", "exprot", "out"]),
            Err(Refusal::Incomplete {
                path: vec!["config"]
            })
        );
        assert_eq!(parse(&["bogus"]), Err(Refusal::Incomplete { path: vec![] }));
    }

    #[test]
    fn help_and_version_win_anywhere() {
        for help in ["--help", "-h", "-?"] {
            assert_eq!(parse(&[help]), Ok(Invocation::Help));
            assert_eq!(parse(&["config", "export", help]), Ok(Invocation::Help));
            assert_eq!(parse(&["config", "apply", help]), Ok(Invocation::Help));
        }
        for version in ["--version", "-v"] {
            assert_eq!(parse(&[version]), Ok(Invocation::Version));
        }
    }

    #[test]
    fn a_directory_named_like_a_subcommand_needs_to_come_later() {
        // `export info` is the subcommand, as natively; an argument given
        // first keeps later words as arguments.
        assert!(matches!(
            parse(&["config", "export", "info"]),
            Err(Refusal::UnsupportedCommand(_))
        ));
        let request = export(&["config", "export", "--db-name=b", r".\info"]);
        assert_eq!(request.path, OsString::from(r".\info"));
    }

    #[test]
    fn the_ibcmd_rs_options_parse() {
        let request = export(&[
            "config",
            "export",
            "--db-name=b",
            "--report=r.json",
            "--sqlcmd=C:\\sql\\SQLCMD.EXE",
            "--settings=s.json",
            "--source-version=8.5.1",
            "--db-pwd-env=MY_PWD",
            "out",
        ]);
        assert_eq!(request.common.report, Some(PathBuf::from("r.json")));
        assert_eq!(
            request.common.sqlcmd,
            Some(PathBuf::from("C:\\sql\\SQLCMD.EXE"))
        );
        assert_eq!(request.common.settings, Some(PathBuf::from("s.json")));
        assert_eq!(
            request.common.source_version,
            Some(InfobaseConfigSourceVersion::V2_21)
        );
        assert_eq!(request.common.db_pwd_env.as_deref(), Some("MY_PWD"));
        let request = import(&["config", "import", "--settings=s.json", "--base-free", "t"]);
        assert!(request.base_free);
        assert_eq!(request.common.db_name, None);
    }
}
