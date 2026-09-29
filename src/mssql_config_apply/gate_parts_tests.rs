//! The conservative gate's rules for body rows of several parts (the platform
//! cuts a value at 10 MB), against a database that is a table of canned
//! answers.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::mssql_config_apply::versions::deflate_row;
use crate::sql::{Dbms, ScriptVariables, SqlRow, SqlValue};

const CONFIG: &str = "66193438-abc5-410b-a1f1-a204102d1a62";
const TEMPLATE: &str = "0808b882-fde2-508b-9da9-4c3765442bbf";
const CATALOG: &str = "11111111-2222-4333-8444-555555555555";

/// Answers the two reads the role check makes: the `root` row, and the scan of
/// the active descriptors (there are none). Counts the reads.
#[derive(Default)]
struct Canned {
    reads: AtomicUsize,
}

impl SqlClient for Canned {
    fn dbms(&self) -> Dbms {
        Dbms::SqlServer
    }
    fn max_connections(&self) -> usize {
        1
    }
    fn run_script(&self, _script: &str, _variables: ScriptVariables) -> Result<()> {
        unreachable!("the gate only reads")
    }
    fn execute(&self, _statement: &str, _params: &[SqlParam<'_>]) -> Result<u64> {
        unreachable!("the gate only reads")
    }
    fn read_rows(
        &self,
        query: &str,
        _params: &[SqlParam<'_>],
        each: &mut dyn FnMut(SqlRow) -> Result<()>,
    ) -> Result<()> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if query.contains("FileName = N'root'") {
            let root = format!("\u{feff}{{2,{CONFIG},0}}");
            return each(SqlRow {
                result_set: 0,
                values: vec![SqlValue::Binary(deflate_row(root.as_bytes()).unwrap())],
            });
        }
        if query.contains("FileName LIKE N'________-____-____-____-____________'") {
            return Ok(());
        }
        panic!("no canned answer for: {query}");
    }
    fn query_json(&self, _query: &str) -> Result<Option<String>> {
        unreachable!()
    }
    fn write_rows(
        &self,
        _table: &str,
        _columns: &[&str],
        _rows: &[Vec<SqlParam<'_>>],
    ) -> Result<u64> {
        unreachable!()
    }
}

fn meta(name: &str, part: i32, sha256: &str) -> RowMeta {
    RowMeta {
        name: name.to_owned(),
        part,
        data_size: 1,
        byte_len: 1,
        attributes: 0,
        creation: String::new(),
        modified: String::new(),
        sha256: sha256.to_owned(),
    }
}

/// Runs the conservative gate on `staged` against `active`; `kinds` names the
/// objects the caller has analysed (the gate reads no other object kind here).
fn check(
    client: &Canned,
    staged: &[RowMeta],
    active: &[RowMeta],
    kinds: &[(&str, &'static str)],
) -> GateVerdict {
    let active: HashMap<(String, i32), RowMeta> =
        active.iter().map(|row| (row.key(), row.clone())).collect();
    let accepted_new_rows = HashSet::new();
    let accepted_owner_descriptors = HashSet::new();
    let consumed_rows = HashSet::new();
    let new_object_kinds: HashMap<String, &'static str> = kinds
        .iter()
        .map(|(uuid, kind)| ((*uuid).to_owned(), *kind))
        .collect();
    ConservativeGate::default()
        .check(&GateInput {
            client,
            database: "testdb",
            staged,
            active: &active,
            accepted_new_rows: &accepted_new_rows,
            accepted_owner_descriptors: &accepted_owner_descriptors,
            new_object_kinds: &new_object_kinds,
            consumed_rows: &consumed_rows,
        })
        .unwrap()
}

fn blocker_reasons(verdict: &GateVerdict) -> Vec<String> {
    verdict
        .blockers
        .iter()
        .map(|blocker| format!("{}: {}", blocker.row, blocker.reason))
        .collect()
}

#[test]
fn a_row_of_two_parts_is_one_change() {
    // the first part is unchanged, the second one differs: the row changed, and
    // its first part stands for it in the role check
    let name = format!("{TEMPLATE}.0");
    let client = Canned::default();
    let verdict = check(
        &client,
        &[meta(&name, 0, "a"), meta(&name, 1, "b")],
        &[meta(&name, 0, "a"), meta(&name, 1, "c")],
        &[(TEMPLATE, "Template")],
    );
    assert!(
        !verdict.restructuring_required,
        "{:?}",
        blocker_reasons(&verdict)
    );
    assert_eq!(verdict.stats.extra_parts, 1);
    assert_eq!(verdict.stats.rows_identical, 0);
    assert_eq!(verdict.stats.bodies_by_role.get("Template"), Some(&1));
}

#[test]
fn a_row_of_two_parts_that_did_not_change_needs_no_role_check() {
    let name = format!("{TEMPLATE}.0");
    let client = Canned::default();
    let verdict = check(
        &client,
        &[meta(&name, 0, "a"), meta(&name, 1, "b")],
        &[meta(&name, 0, "a"), meta(&name, 1, "b")],
        &[],
    );
    assert!(!verdict.restructuring_required);
    assert_eq!(verdict.stats.rows_identical, 2);
    assert_eq!(verdict.stats.extra_parts, 0);
    assert_eq!(client.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn a_new_row_of_two_parts_counts_its_extra_part() {
    // no active row at all: a new body of an object the caller has analysed
    let name = format!("{TEMPLATE}.0");
    let client = Canned::default();
    let verdict = check(
        &client,
        &[meta(&name, 0, "a"), meta(&name, 1, "b")],
        &[],
        &[(TEMPLATE, "Template")],
    );
    assert!(
        !verdict.restructuring_required,
        "{:?}",
        blocker_reasons(&verdict)
    );
    assert_eq!(verdict.stats.extra_parts, 1);
}

#[test]
fn a_part_whose_first_part_is_not_staged_is_refused() {
    let name = format!("{TEMPLATE}.0");
    let client = Canned::default();
    let verdict = check(
        &client,
        &[meta(&name, 1, "b")],
        &[meta(&name, 0, "a"), meta(&name, 1, "c")],
        &[(TEMPLATE, "Template")],
    );
    assert!(verdict.restructuring_required);
    let reasons = blocker_reasons(&verdict);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("first part is not staged")),
        "{reasons:?}"
    );
}

#[test]
fn a_row_of_several_parts_is_never_compared_as_text() {
    // predefined data is a role the gate admits only when the text is unchanged;
    // the comparison reads one part, so a row of two parts cannot be proved
    let name = format!("{CATALOG}.1c");
    let client = Canned::default();
    let verdict = check(
        &client,
        &[meta(&name, 0, "a"), meta(&name, 1, "b")],
        &[meta(&name, 0, "a"), meta(&name, 1, "c")],
        &[(CATALOG, "Catalog")],
    );
    assert!(verdict.restructuring_required);
    let reasons = blocker_reasons(&verdict);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("several parts")),
        "{reasons:?}"
    );
}

#[test]
fn a_single_part_over_a_row_of_several_parts_is_refused_the_same_way() {
    let name = format!("{CATALOG}.1c");
    let client = Canned::default();
    let verdict = check(
        &client,
        &[meta(&name, 0, "b")],
        &[meta(&name, 0, "a"), meta(&name, 1, "c")],
        &[(CATALOG, "Catalog")],
    );
    assert!(verdict.restructuring_required);
    assert!(
        blocker_reasons(&verdict)
            .iter()
            .any(|reason| reason.contains("several parts"))
    );
}

#[test]
fn a_consumed_row_is_not_judged_and_an_unconsumed_one_is_refused() {
    let client = Canned::default();
    let staged = [meta("deleted", 0, "a")];
    let judge = |consumed: &HashSet<String>| {
        let active: HashMap<(String, i32), RowMeta> = HashMap::new();
        let accepted = HashSet::new();
        let kinds = HashMap::new();
        ConservativeGate::default()
            .check(&GateInput {
                client: &client,
                database: "testdb",
                staged: &staged,
                active: &active,
                accepted_new_rows: &accepted,
                accepted_owner_descriptors: &accepted,
                new_object_kinds: &kinds,
                consumed_rows: consumed,
            })
            .unwrap()
    };
    let refused = judge(&HashSet::new());
    assert!(refused.restructuring_required);
    let mut consumed = HashSet::new();
    consumed.insert("deleted".to_owned());
    let passed = judge(&consumed);
    assert!(
        !passed.restructuring_required,
        "{:?}",
        blocker_reasons(&passed)
    );
}

#[test]
fn a_descriptor_has_one_part() {
    let client = Canned::default();
    let verdict = check(
        &client,
        &[meta(CATALOG, 0, "a"), meta(CATALOG, 1, "b")],
        &[meta(CATALOG, 0, "a")],
        &[],
    );
    assert!(verdict.restructuring_required);
    assert!(
        blocker_reasons(&verdict)
            .iter()
            .any(|reason| reason.contains("a part number other than 0"))
    );
}
