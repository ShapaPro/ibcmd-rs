//! The plan of case a2 against what the native apply did (fixtures cut from the lab twin).

use std::collections::BTreeSet;

use crate::metadata_model::brace::parse_row;
use crate::restructure::names::{DbNames, deflate, inflate};
use crate::restructure::plan::{Inputs, Phase, PlanOptions, StagedImage, plan};
use crate::restructure::schema::DbSchema;

const FIXTURES: &str = "tests/fixtures/native-evidence/restructure";

const STAGED_SCHEMA: &[u8] = include_bytes!(
    "../../tests/fixtures/native-evidence/restructure/dbschema-a2-staged-excerpt.txt"
);
const NATIVE_SCHEMA: &[u8] = include_bytes!(
    "../../tests/fixtures/native-evidence/restructure/dbschema-a2-native-excerpt.txt"
);
const NAMES: &[u8] =
    include_bytes!("../../tests/fixtures/native-evidence/restructure/dbnames-a2-excerpt.txt");
const OLD_ROW: &[u8] = include_bytes!(
    "../../tests/fixtures/native-evidence/restructure/catalog-reference20-old.deflate"
);
const NEW_ROW: &[u8] = include_bytes!(
    "../../tests/fixtures/native-evidence/restructure/catalog-reference20-new.deflate"
);
const STATEMENTS: &str =
    include_str!("../../docs/apply/evidence/restructuring/a2-structure-statements.sql");

const CATALOG: &str = "5eab8a1b-070f-4dcf-bdcc-a259c62c3693";
const VERSION: &str = "d516886c-0000-4000-8000-00000000abcd";

fn names_row(max: u64) -> Vec<u8> {
    DbNames {
        max,
        entries: Vec::new(),
    }
    .to_row()
    .unwrap()
}

fn inputs(old: &[u8], new: &[u8]) -> Inputs {
    let files: BTreeSet<String> = [CATALOG.to_owned()].into();
    let mut new_files = files.clone();
    new_files.insert("deleted".to_owned());
    Inputs {
        schema: STAGED_SCHEMA.to_vec(),
        main_names: DbNames::parse(NAMES).unwrap().to_row().unwrap(),
        // The header numbers of the extensions of the lab database.
        extension_names: vec![
            ("DBNames-Ext-1".to_owned(), names_row(10989)),
            ("DBNames-Ext-347eea02".to_owned(), names_row(7841)),
            ("DBNames-Ext-b3fa0ef0".to_owned(), names_row(11033)),
        ],
        predefined_tables: BTreeSet::new(),
        cache_rows: Vec::new(),
        root_row: Vec::new(),
        staged: StagedImage {
            old_files: files,
            new_files,
            old_descriptors: [(CATALOG.to_owned(), old.to_vec())].into(),
            new_descriptors: [(CATALOG.to_owned(), new.to_vec())].into(),
            deleted: Some(deflate("\u{feff}0".as_bytes()).unwrap()),
        },
    }
}

fn options() -> PlanOptions {
    PlanOptions {
        names_version: Some(VERSION.to_owned()),
        // the fixtures hold no cache rows (the corpus test has the real ones)
        skip_xdto: true,
        skip_registry: true,
        ..PlanOptions::default()
    }
}

fn error_of(inputs: &Inputs) -> String {
    format!("{:#}", plan(inputs, &options()).unwrap_err())
}

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[test]
fn the_fixtures_are_where_the_tests_expect_them() {
    assert!(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(FIXTURES)
            .join("catalog-reference20-new.deflate")
            .exists()
    );
}

#[test]
fn plans_case_a2_like_the_platform_did() {
    let plan = plan(&inputs(OLD_ROW, NEW_ROW), &options()).unwrap();
    assert_eq!(plan.objects.len(), 1);
    assert_eq!(plan.objects[0].object, "Reference20");
    assert_eq!(plan.objects[0].additions.len(), 1);
    let addition = &plan.objects[0].additions[0];
    assert_eq!(addition.uuid, "c60cdc87-198a-4f6e-8f17-76bcb1b1914b");
    assert_eq!(addition.name, "ДемоНовыйРеквизит");
    // The counter is shared with the extensions: 11033 there, 10824 here.
    assert_eq!(addition.number, 11034);
    assert_eq!(addition.field.name, "Fld11034");
    assert!(addition.field.nullable);
    assert_eq!(addition.position, 14);

    // The new entry is the platform's, byte for byte; ConfigChngR stays last.
    let ours = DbSchema::parse(&plan.new_schema).unwrap();
    let native = DbSchema::parse(NATIVE_SCHEMA).unwrap();
    assert_eq!(
        ours.tables()[ours.position("Reference20").unwrap()],
        native.tables()[native.position("Reference20").unwrap()]
    );
    assert_eq!(ours.len(), 4);
    assert_eq!(ours.position("ConfigChngR"), Some(3));
    assert_eq!(ours.position("Reference20"), Some(2));

    // DBNames: one more entry, the header moved to it.
    let names = DbNames::parse(&plan.new_names_text).unwrap();
    assert_eq!(names.max, 11034);
    let last = names.entries.last().unwrap();
    assert_eq!(
        (last.uuid.as_str(), last.kind.as_str(), last.number),
        ("c60cdc87-198a-4f6e-8f17-76bcb1b1914b", "Fld", 11034)
    );
    assert_eq!(inflate(&plan.new_names_row).unwrap(), plan.new_names_text);
    assert_eq!(
        crate::restructure::names::parse_version(&plan.names_version_row).unwrap(),
        VERSION
    );
}

#[test]
fn the_statements_are_the_platforms_for_the_object() {
    let plan = plan(&inputs(OLD_ROW, NEW_ROW), &options()).unwrap();
    let statements = plan.statements();
    let trace = lf(STATEMENTS);

    // Everything the platform executed verbatim: create table, create index, drop, rename.
    for statement in statements.iter().filter(|s| {
        matches!(
            s.phase,
            Phase::Create | Phase::Indexes | Phase::DropOld | Phase::Rename
        )
    }) {
        if statement.sql.starts_with("drop table") {
            assert!(trace.contains(&statement.sql), "{}", statement.sql);
        } else if let Some(rename) = statement.sql.strip_prefix("EXEC sp_rename ") {
            // The trace passes the names as parameters; the arguments are the same.
            let arguments: Vec<&str> = rename.trim_end_matches(';').split(", ").collect();
            let first = arguments[0].trim_matches(|c| c == 'N' || c == '\'');
            let second = arguments[1].trim_matches(|c| c == 'N' || c == '\'');
            let needle = format!("-- args: N'{first}',N'{second}'");
            assert!(trace.contains(&needle), "{needle}");
        } else {
            assert!(
                trace.contains(&statement.sql),
                "not in the trace:\n{}",
                statement.sql
            );
        }
    }

    // The copy: the platform's statement with its parameters written as literals.
    let copies: Vec<_> = statements
        .iter()
        .filter(|s| s.phase == Phase::Load)
        .collect();
    assert_eq!(copies.len(), 3);
    for copy in copies {
        let with_parameter = copy.sql.replace("N''", "@P1");
        assert!(
            trace.contains(&with_parameter),
            "not in the trace:\n{with_parameter}"
        );
    }

    // The guard comes first, the publication last.
    assert_eq!(statements.first().unwrap().phase, Phase::Guard);
    assert_eq!(statements.last().unwrap().label, "ConfigSave: emptied");
    let publish: Vec<_> = statements
        .iter()
        .filter(|s| s.phase == Phase::Publish)
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(
        publish,
        [
            "SchemaStorage: idle, the new schema, empty generations",
            "DBSchema",
            "Params DBNames",
            "Params DBNamesVersion-DBNames",
            "Config: the changed staged rows replace the stored ones",
            "ConfigSave: emptied"
        ]
    );
}

#[test]
fn the_phase_text_is_the_statements_with_assertions() {
    let plan = plan(&inputs(OLD_ROW, NEW_ROW), &options()).unwrap();
    let text = plan.phase_sql("@now").unwrap();
    let trace = lf(STATEMENTS);
    // The DDL is the platform's, verbatim, in the platform's order.
    let mut from = 0;
    for statement in plan
        .statements()
        .iter()
        .filter(|s| matches!(s.phase, Phase::Create | Phase::Indexes | Phase::DropOld))
    {
        assert!(trace.contains(&statement.sql), "{}", statement.sql);
        let at = text[from..]
            .find(&statement.sql)
            .unwrap_or_else(|| panic!("not in the phase text, in order:\n{}", statement.sql));
        from += at + statement.sql.len();
    }
    // Every rename, table and index.
    assert_eq!(text.matches("EXEC sp_rename").count(), 13);
    // No parameter marker: the values are literals.
    assert!(!text.contains("@P1"), "a parameter marker");
    // The guards and the assertions.
    for number in [57400, 57401, 57402, 57403, 57404, 57405] {
        assert!(text.contains(&format!("THROW {number},")), "{number}");
    }
    // The publication uses the caller's timestamp and touches neither Config nor the caches.
    assert!(text.contains("Modified = @now WHERE FileName = N'DBNames'"));
    assert!(!text.contains("dbo.Config"));
    assert!(!text.contains(".si"));
    // The old rows the guards compare with are the plan's inputs.
    assert!(text.contains(&plan.old_schema_sha256.to_ascii_uppercase()));
    assert!(text.contains(&plan.old_names_sha256.to_ascii_uppercase()));
    // The alter method is not for the apply.
    let mut alter = plan.clone();
    alter.method = crate::restructure::plan::Method::AlterAdd;
    assert!(alter.phase_sql("@now").is_err());
}

#[test]
fn a_partial_stage_is_planned_like_a_whole_one() {
    // `import files --partial` stages the changed descriptor and root, version, versions only.
    let mut partial = inputs(OLD_ROW, NEW_ROW);
    partial.staged.old_files = ["root", "version", "versions", "other-file", CATALOG]
        .map(str::to_owned)
        .into();
    partial.staged.new_files = ["root", "version", "versions", CATALOG]
        .map(str::to_owned)
        .into();
    partial.staged.deleted = None;
    let whole = plan(&inputs(OLD_ROW, NEW_ROW), &options()).unwrap();
    let delta = plan(&partial, &options()).unwrap();
    assert_eq!(delta.new_schema, whole.new_schema);
    assert_eq!(delta.new_names_text, whole.new_names_text);
}

#[test]
fn refuses_what_it_cannot_prove() {
    let base = inputs(OLD_ROW, NEW_ROW);

    // No attribute is new.
    assert!(error_of(&inputs(OLD_ROW, OLD_ROW)).contains("adds no attribute"));

    // An attribute is removed (the images swapped).
    assert!(error_of(&inputs(NEW_ROW, OLD_ROW)).contains("removed"));

    // An object is added.
    let mut more = base.clone();
    more.staged
        .new_files
        .insert("aaaaaaaa-0000-4000-8000-000000000000".to_owned());
    assert!(error_of(&more).contains("adds 1"));

    // The deleted marker has content.
    let mut deleting = base.clone();
    deleting.staged.deleted = Some(deflate("\u{feff}1".as_bytes()).unwrap());
    assert!(error_of(&deleting).contains("deletes"));

    // A stored attribute changes its type.
    let text = String::from_utf8(inflate(NEW_ROW).unwrap()).unwrap();
    let start = text.find("\"Клиент\"").unwrap();
    let pattern = start + text[start..].find("{\"Pattern\"").unwrap();
    let mut changed = text.clone();
    changed.replace_range(
        pattern..pattern + "{\"Pattern\",\r\n{\"B\"}".len(),
        "{\"Pattern\",\r\n{\"S\",10,1}",
    );
    assert_ne!(changed, text);
    assert!(parse_row(changed.as_bytes()).is_ok());
    let retyped = inputs(OLD_ROW, &deflate(changed.as_bytes()).unwrap());
    assert!(error_of(&retyped).contains("changes its type"));

    // The stored schema disagrees with what the generator makes of the attribute.
    let schema = String::from_utf8(STAGED_SCHEMA.to_vec()).unwrap();
    let at = schema.find("\"Fld151\"").unwrap();
    let mut wrong = schema.clone();
    let boolean = at + wrong[at..].find("{\"L\",0,0,\"\",0}").unwrap();
    wrong.replace_range(
        boolean..boolean + "{\"L\",0,0,\"\",0}".len(),
        "{\"S\",10,0,\"\",0}",
    );
    let mut tampered = base.clone();
    tampered.schema = wrong.into_bytes();
    assert!(error_of(&tampered).contains("differs from what the generator makes"));

    // A companion table of the object exists: the change registration table and an empty
    // predefined-data table are harmless (case a2), a predefined-data table with rows is not.
    let mut with_companion = base.clone();
    let mut extra = DbSchema::parse(STAGED_SCHEMA).unwrap();
    let copy = extra.tables()[extra.position("DbCopies").unwrap()].clone();
    let renamed = String::from_utf8(DbSchema::from_tables(vec![copy]).to_text())
        .unwrap()
        .replace("\"DbCopies\"", "\"RefSInf4877\"");
    let companion = DbSchema::parse(renamed.as_bytes()).unwrap().tables()[0].clone();
    extra.insert_before("ConfigChngR", companion);
    with_companion.schema = extra.to_text();
    assert!(plan(&with_companion, &options()).is_ok());
    with_companion
        .predefined_tables
        .insert("RefSInf4877".to_owned());
    assert!(error_of(&with_companion).contains("companion table"));
}
