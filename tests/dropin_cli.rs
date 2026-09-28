//! The drop-in command line, run as a process: every native mode and
//! command either served or refused by name with exit code 1, in Russian,
//! without a panic and without any database (the cases here fail before a
//! connection would be opened).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use ibcmd_rs::dropin::parse::{NodeKind, OTHER_MODES, command_paths};

fn run(args: &[&str]) -> Output {
    run_with_stdin(args, None)
}

fn run_with_stdin(args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ibcmd-rs"))
        .args(args)
        // nothing may be launched: no PATH to find a platform or sqlcmd on
        .env("PATH", "")
        .env_remove("IBCMD_DB_PSW")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    child.wait_with_output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn assert_refused(args: &[&str], stream_has: &str) -> Output {
    let output = run(args);
    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    assert_eq!(
        output.status.code(),
        Some(1),
        "{args:?}\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains(stream_has) || stderr.contains(stream_has),
        "{args:?}: expected {stream_has:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    assert!(
        !stderr.contains("unrecognized subcommand"),
        "{args:?}: {stderr}"
    );
    output
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ibcmd-rs-dropin-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn arg(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_other_native_mode_is_refused_by_name() {
    for (mode, _) in OTHER_MODES {
        for args in [
            vec![*mode],
            vec![*mode, "--help"],
            vec![*mode, "list", "--x=1"],
        ] {
            assert_refused(
                &args,
                &format!(
                    "Режим `{mode}` не поддерживается в этой версии ibcmd-rs (планируется в следующих)"
                ),
            );
        }
    }
}

#[test]
fn every_native_infobase_command_is_served_or_refused_by_name() {
    for (path, kind) in command_paths() {
        let mut args = vec!["infobase"];
        args.extend(path.iter().copied());
        args.extend(["--dbms=MSSQLServer", "--db-name=ibcmd_rs_dropin_test"]);
        match kind {
            NodeKind::Unsupported => {
                assert_refused(
                    &args,
                    &format!(
                        "Команда `infobase {}` не поддерживается в этой версии ibcmd-rs (планируется в следующих)",
                        path.join(" ")
                    ),
                );
            }
            NodeKind::Group => {
                assert_refused(&args, "Указана неполная команда");
            }
            // served: without its path it asks for one
            NodeKind::Export | NodeKind::Import => {
                assert_refused(&args, "Не указано значение параметра");
            }
        }
    }
    assert_refused(&["infobase"], "Указана неполная команда");
    assert_refused(&["infobase", "config"], "ibcmd-rs infobase config load");
    assert_refused(&["infobase", "bogus"], "Указана неполная команда");
}

#[test]
fn unsupported_options_and_malformed_lines_are_refused() {
    let out = TempDir::new("options");
    for (option, needle) in [
        (
            "--sync",
            "Параметр `--sync` команды `infobase config export`",
        ),
        ("--archive", "Параметр `--archive`"),
        ("--base=info.xml", "Параметр `--base`"),
        ("--file=a.cf", "Параметр `--file`"),
        ("--extension=E", "Параметр `--extension`"),
        ("--remote=http://h:1545", "Параметр `--remote`"),
        ("--pid=1", "Параметр `--pid`"),
        ("--bogus", "Ошибка разбора параметра: --bogus"),
        ("-T4", "Ошибка разбора параметра: -T4"),
        (
            "--threads=many",
            "Недопустимое значение параметра --threads: many",
        ),
    ] {
        assert_refused(
            &[
                "infobase",
                "config",
                "export",
                "--dbms=MSSQLServer",
                "--db-name=b",
                option,
                out.arg(),
            ],
            needle,
        );
    }
    for (option, needle) in [
        (
            "--out=a.cf",
            "Параметр `--out` команды `infobase config import`",
        ),
        (
            "--extension=E",
            "Параметр `--extension` команды `infobase config import`",
        ),
    ] {
        assert_refused(
            &[
                "infobase",
                "config",
                "import",
                "--db-name=b",
                option,
                out.arg(),
            ],
            needle,
        );
    }
    for dbms in ["PostgreSQL", "IBMDB2", "OracleDatabase"] {
        assert_refused(
            &[
                "infobase",
                "config",
                "export",
                &format!("--dbms={dbms}"),
                out.arg(),
            ],
            &format!("СУБД `{dbms}` не поддерживается"),
        );
    }
    assert_refused(
        &["infobase", "config", "export", "--dbms=Foo", out.arg()],
        "Указанный тип СУБД не поддерживается: 'Foo'",
    );
    assert_refused(
        &["infobase", "config", "export", out.arg()],
        "Файловые информационные базы не поддерживаются",
    );
    assert_refused(
        &[
            "infobase",
            "config",
            "export",
            "--db-path=C:\\ib",
            out.arg(),
        ],
        "Файловые информационные базы не поддерживаются",
    );
    let archive = out.path().join("tree.zip");
    fs::write(&archive, b"PK").unwrap();
    assert_refused(
        &[
            "infobase",
            "config",
            "import",
            "--db-name=b",
            archive.to_str().unwrap(),
        ],
        "Импорт конфигурации из архива",
    );
}

#[test]
fn export_refuses_a_non_empty_directory_as_the_platform_does() {
    let out = TempDir::new("not-empty");
    fs::write(out.path().join("x.txt"), b"x").unwrap();
    let report = out.path().join("report.json");
    let output = run(&[
        "infobase",
        "config",
        "export",
        "--dbms=MSSQLServer",
        "--db-server=localhost",
        "--db-name=ibcmd_rs_dropin_test",
        "--force",
        &format!("--report={}", report.display()),
        out.arg(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        "[INFO] Экспорт конфигурации в XML...\n"
    );
    assert_eq!(
        text(&output.stderr).trim_end(),
        format!(
            "[ERROR] Операция невозможна, при выполнении экспорта конфигурации в XML обнаружены ошибки: Каталог {} не пуст.",
            out.arg()
        )
    );
    // the directory is left as it was; the report records the failure
    assert!(out.path().join("x.txt").is_file());
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(report["ok"], false);
    assert_eq!(report["operation"], "infobase config export");
}

#[test]
fn import_reports_a_missing_tree_in_the_platforms_words() {
    let out = TempDir::new("missing-tree");
    let missing = out.path().join("no-such-tree");
    let output = run(&[
        "infobase",
        "config",
        "import",
        "--dbms=MSSQLServer",
        "--db-name=ibcmd_rs_dropin_test",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        "[INFO] Импорт конфигурации из XML...\n"
    );
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("каталог файлов конфигурации не найден"),
        "{stderr}"
    );
    assert!(
        stderr
            .trim_end()
            .ends_with("[ERROR] Импорт конфигурации из XML завершен с ошибкой"),
        "{stderr}"
    );
}

#[test]
fn the_database_password_can_come_from_stdin() {
    let out = TempDir::new("stdin");
    fs::write(out.path().join("x.txt"), b"x").unwrap();
    // -W reads the password; the run then stops at the non-empty directory,
    // before any connection.
    let output = run_with_stdin(
        &[
            "infobase",
            "config",
            "export",
            "--dbms=MSSQLServer",
            "--db-name=b",
            "--db-user=sa",
            "-W",
            out.arg(),
        ],
        Some("secret\r\n"),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("не пуст"));
    // without -W and without a password the user is told how to give one
    let output = run(&[
        "infobase",
        "config",
        "export",
        "--dbms=MSSQLServer",
        "--db-name=b",
        "--db-user=sa",
        out.arg(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("не указан пароль пользователя сервера СУБД"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn help_and_version() {
    for args in [
        vec!["infobase", "--help"],
        vec!["infobase", "-?"],
        vec!["infobase", "config", "export", "-h"],
        vec!["help", "infobase"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        let help = text(&output.stdout);
        assert!(
            help.contains("Режим управления информационной базой"),
            "{args:?}"
        );
        assert!(help.contains("--db-server"), "{args:?}");
    }
    let output = run(&["help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("Поддерживаемые режимы"));
    let output = run(&["help", "source-diff"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("Usage"));
    assert_refused(&["help", "server"], "Режим `server` не поддерживается");
    assert_refused(&["help", "bogus"], "Неизвестный режим: bogus");
    let output = run(&["infobase", "--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).starts_with("ibcmd-rs "));
}
