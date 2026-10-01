//! `infobase config save` from rows (Untru/ibcmd-rs#352), proven without a
//! server: a platform
//! `.cf` is turned into the folder of stored rows a `Config` table would hold
//! (`<FileName>__part<N>.bin`, the element's packed bytes), saved back with
//! `mssql-save-config --rows-dir`, and the result is compared with the
//! original -- `cf export` file by file, `cf inspect` element by element,
//! and byte by byte where the container is this writer's own.
//!
//! The fixtures are files the platform wrote (Designer `/DumpCfg` of
//! 8.3.27.2214, `_onecdec/make_*_fixture.py`): Format16 behind the platform's
//! preamble for a configuration in a current compatibility mode, Format15 for
//! one in 8.3.10's (`config_compat/c10_e27`).

mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::{Compression, write::DeflateEncoder};
use ibcmd_cf::archive::decode_packed_archive;
use ibcmd_core::{artifact::StorageProfileId, limits::ResourceLimits};
use ibcmd_v8::writer::{Format15Document, Format15Element, write_format15_to_vec};
use serde_json::Value;

/// Platform-written configurations, Format16 and Format15.
const FIXTURES: [&str; 4] = [
    "choice_list_dates/input.cf",
    "home_page/two_variable/input.cf",
    "upgrade/exchange_plan/current.cf",
    "config_compat/c10_e27/input.cf",
];

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ibcmd-rs-cf-config-save-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ibcmd_rs(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ibcmd-rs"))
        .args(args)
        // nothing may be launched and no server reached
        .env("PATH", "")
        .output()
        .expect("run ibcmd-rs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The elements of a `.cf`: name and packed bytes, in the container's order.
fn packed_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let file = fs::File::open(path).unwrap();
    let limits = ResourceLimits::for_input_bytes(file.metadata().unwrap().len());
    let archive = decode_packed_archive(
        file,
        limits,
        StorageProfileId::parse("storage:test").unwrap(),
    )
    .unwrap();
    let (_, _, entries) = archive.into_parts();
    entries
        .into_iter()
        .map(|entry| entry.into_parts())
        .collect()
}

/// The folder of stored rows a `Config` table holding `entries` amounts to.
/// The row named `split` is stored in two parts, as `Config` keeps a row
/// over its part size.
fn write_rows_dir(dir: &Path, entries: &[(String, Vec<u8>)], split: Option<&str>) {
    fs::create_dir_all(dir).unwrap();
    for (name, bytes) in entries {
        if split == Some(name.as_str()) {
            let (first, second) = bytes.split_at(bytes.len() / 2);
            fs::write(dir.join(format!("{name}__part0.bin")), first).unwrap();
            fs::write(dir.join(format!("{name}__part1.bin")), second).unwrap();
        } else {
            fs::write(dir.join(format!("{name}__part0.bin")), bytes).unwrap();
        }
    }
}

fn save(rows_dir: &Path, output: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "mssql-save-config".as_ref(),
        "--rows-dir".as_ref(),
        rows_dir.as_os_str(),
    ];
    args.extend(extra.iter().map(|arg| std::ffi::OsStr::new(*arg)));
    args.push(output.as_os_str());
    ibcmd_rs(&args)
}

fn saved(rows_dir: &Path, output: &Path) -> Value {
    let run = save(rows_dir, output, &[]);
    assert!(
        run.status.success(),
        "{}\n{}",
        text(&run.stdout),
        text(&run.stderr)
    );
    serde_json::from_slice(&run.stdout).unwrap()
}

/// `cf inspect`: every element's name and packed and unpacked SHA-256.
fn inspect(path: &Path) -> Vec<(String, String, String)> {
    let run = ibcmd_rs(&["cf".as_ref(), "inspect".as_ref(), path.as_os_str()]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    let report: Value = serde_json::from_slice(&run.stdout).unwrap();
    report["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|element| {
            let field = |name: &str| element[name].as_str().unwrap().to_string();
            (
                field("name"),
                field("packed_sha256"),
                field("unpacked_sha256"),
            )
        })
        .collect()
}

/// The container this save writes for `entries`, built independently with
/// the V8 writer: Format15, 512-byte pages, the element count in the third
/// header word, elements in byte order of their names, headers without time.
fn expected_container(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    let document = Format15Document::new(
        sorted.len() as u32,
        sorted
            .iter()
            .map(|(name, bytes)| Format15Element::named(name, Some(bytes.clone())))
            .collect(),
    );
    write_format15_to_vec(&document).unwrap()
}

fn largest(entries: &[(String, Vec<u8>)]) -> String {
    entries
        .iter()
        .max_by_key(|(_, bytes)| bytes.len())
        .unwrap()
        .0
        .clone()
}

#[test]
fn rows_saved_as_a_cf_export_and_inspect_as_the_platform_file() {
    for fixture in FIXTURES {
        let original = common::fixture(fixture);
        let scratch = Scratch::new(&fixture.replace('/', "-"));
        let entries = packed_entries(&original);
        let rows = scratch.join("rows");
        let split = largest(&entries);
        write_rows_dir(&rows, &entries, Some(&split));

        let output = scratch.join("saved.cf");
        let report = saved(&rows, &output);
        assert_eq!(report["elements"], entries.len(), "{fixture}");
        assert_eq!(report["multi_part_rows"], 1, "{fixture}");
        assert_eq!(report["source"], "rows-dir", "{fixture}");
        assert_eq!(report["revision"], "format15", "{fixture}");
        assert_eq!(report["storage_word"], entries.len(), "{fixture}");
        assert_eq!(report["left_out"], Value::Array(Vec::new()), "{fixture}");

        // (a) the same tree, file for file
        let (ours, theirs) = (scratch.join("ours"), scratch.join("theirs"));
        let run_ours = common::export(&output, &ours);
        let run_theirs = common::export(&original, &theirs);
        assert_eq!(
            run_ours.status.code(),
            run_theirs.status.code(),
            "{fixture}"
        );
        assert!(
            common::files(&theirs).len() > 1,
            "{fixture}: the original exports nothing"
        );
        common::assert_tree_eq(&theirs, &ours);

        // (b) the same elements, in the same order, with the same bytes
        assert_eq!(inspect(&output), inspect(&original), "{fixture}");

        // the container: this writer's layout, byte for byte
        let bytes = fs::read(&output).unwrap();
        assert_eq!(bytes, expected_container(&entries), "{fixture}");
        assert_eq!(&bytes[0..4], &0x7fff_ffff_u32.to_le_bytes(), "{fixture}");
        assert_eq!(&bytes[4..8], &512_u32.to_le_bytes(), "{fixture}");
        assert_eq!(
            &bytes[8..12],
            &(entries.len() as u32).to_le_bytes(),
            "{fixture}"
        );
        // ... and not the platform's (see docs/evidence/cf-config-save.md)
        assert_ne!(bytes, fs::read(&original).unwrap(), "{fixture}");
    }
}

#[test]
fn a_saved_file_saves_back_byte_for_byte() {
    let scratch = Scratch::new("fixed-point");
    let entries = packed_entries(&common::fixture(FIXTURES[1]));
    write_rows_dir(&scratch.join("rows1"), &entries, None);
    let first = scratch.join("first.cf");
    saved(&scratch.join("rows1"), &first);

    let again = packed_entries(&first);
    let mut sorted = entries.clone();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(again, sorted);
    write_rows_dir(&scratch.join("rows2"), &again, Some(&largest(&again)));
    let second = scratch.join("second.cf");
    saved(&scratch.join("rows2"), &second);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
}

#[test]
fn the_rows_an_online_generation_publishes_are_saved() {
    const GENERATION: &str = "06cb0442-0c47-4fad-986a-f08f28287c1b";
    let scratch = Scratch::new("generation");
    let entries = packed_entries(&common::fixture(FIXTURES[0]));
    let rows = scratch.join("rows");
    write_rows_dir(&rows, &entries, None);
    // An online update keeps the new body of a row under an alias and records
    // its generation in `DynamicallyUpdated` (stored as it is, not deflated).
    let body = entries
        .iter()
        .find(|(name, _)| name.ends_with(".0"))
        .unwrap()
        .0
        .clone();
    let base = body.strip_suffix(".0").unwrap();
    let replacement = entries.iter().find(|(name, _)| name == "root").unwrap();
    fs::write(
        rows.join(format!("{base}_dynupdate_{GENERATION}.0__part0.bin")),
        &replacement.1,
    )
    .unwrap();
    fs::write(
        rows.join("DynamicallyUpdated__part0.bin"),
        format!("{{1,1,{GENERATION}}}"),
    )
    .unwrap();

    let output = scratch.join("saved.cf");
    let report = saved(&rows, &output);
    assert_eq!(report["elements"], entries.len());
    assert_eq!(report["generation_aliases"], 1);
    assert_eq!(
        report["left_out"],
        serde_json::json!(["DynamicallyUpdated"])
    );
    let saved_entries = packed_entries(&output);
    let names = saved_entries
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(!names.contains(&"DynamicallyUpdated"), "{names:?}");
    assert!(!names.iter().any(|name| name.contains("_dynupdate_")));
    let published = saved_entries
        .iter()
        .find(|(name, _)| *name == body)
        .unwrap();
    assert_eq!(published.1, replacement.1, "the alias's bytes, published");
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn what_is_no_configuration_is_refused_and_nothing_is_left_behind() {
    let scratch = Scratch::new("refused");
    let entries = packed_entries(&common::fixture(FIXTURES[0]));
    let refused = |tag: &str, rows: Vec<(String, Vec<u8>)>, needle: &str| {
        let dir = scratch.join(tag);
        write_rows_dir(&dir.join("rows"), &rows, None);
        let output = dir.join("saved.cf");
        let run = save(&dir.join("rows"), &output, &[]);
        assert!(!run.status.success(), "{tag}");
        assert!(
            text(&run.stderr).contains(needle),
            "{tag}: {}",
            text(&run.stderr)
        );
        // no file, and no temporary one beside it
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "{tag}: {:?}",
            fs::read_dir(&dir).unwrap().collect::<Vec<_>>()
        );
    };
    let without = |dropped: &str| {
        entries
            .iter()
            .filter(|(name, _)| name != dropped)
            .cloned()
            .collect::<Vec<_>>()
    };
    refused("no-root", without("root"), "`root`");
    let mut unfinished = entries.clone();
    unfinished.push(("commit".to_string(), deflate(b"{}")));
    refused("unfinished", unfinished, "config repair");
    let mut damaged = without("version");
    damaged.push(("version".to_string(), b"not deflate".to_vec()));
    refused("damaged", damaged, "version");

    // an existing file is kept unless asked to replace it
    let rows = scratch.join("rows");
    write_rows_dir(&rows, &entries, None);
    let output = scratch.join("existing.cf");
    fs::write(&output, b"keep me").unwrap();
    let run = save(&rows, &output, &[]);
    assert!(!run.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"keep me");
    let run = save(&rows, &output, &["--overwrite"]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert_eq!(fs::read(&output).unwrap(), expected_container(&entries));
}
