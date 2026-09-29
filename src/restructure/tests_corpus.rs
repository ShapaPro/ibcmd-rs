//! Conformance of the model against the lab corpora. The big samples live in the lab (snapshots of
//! `scripts/restructure-lab/snapshot.py`), so these tests skip themselves when the lab folder is
//! absent: set `IBCMD_RS_DDL_LAB` to another location, default `F:\ibcmd\lab\04\restructure`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

use crate::restructure::names::{DbNames, inflate};
use crate::restructure::plan::{Inputs, PlanOptions, StagedImage, plan};
use crate::restructure::schema::{
    DbSchema, IndexDef, TableView, declared_indexes, implicit_indexes, table_columns,
};

fn lab() -> Option<PathBuf> {
    let root = std::env::var_os("IBCMD_RS_DDL_LAB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"F:\ibcmd\lab\04\restructure"));
    root.join("snap").is_dir().then_some(root)
}

/// One snapshot of the lab: `snap/<db>/<label>/{svc.json,schema.txt}` and the blob store.
struct Snapshot {
    root: PathBuf,
    dir: PathBuf,
    svc: Value,
}

impl Snapshot {
    fn open(db: &str, label: &str) -> Option<Self> {
        let root = lab()?;
        let dir = root.join("snap").join(db).join(label);
        let text = std::fs::read_to_string(dir.join("svc.json")).ok()?;
        Some(Self {
            root,
            dir,
            svc: serde_json::from_str(&text).ok()?,
        })
    }

    fn blob(&self, sha: &str) -> Vec<u8> {
        std::fs::read(self.root.join("blobs").join(sha)).unwrap()
    }

    fn schema(&self) -> Vec<u8> {
        self.blob(self.svc["DBSchema"][0].as_str().unwrap())
    }

    /// The rows of a service table: (name, part) -> sha.
    fn rows(&self, table: &str) -> BTreeMap<(String, i64), String> {
        self.svc[table]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    (
                        row["name"].as_str().unwrap().to_owned(),
                        row["part"].as_i64().unwrap(),
                    ),
                    row["sha"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn row(&self, table: &str, name: &str) -> Option<Vec<u8>> {
        self.rows(table)
            .get(&(name.to_owned(), 0))
            .map(|sha| self.blob(sha))
    }

    fn schema_txt(&self) -> String {
        std::fs::read_to_string(self.dir.join("schema.txt")).unwrap()
    }
}

#[test]
fn corpus_schema_and_names_round_trip_byte_for_byte() {
    for (db, label) in [
        ("ibcmd_rs_04_ddl_bsp8327_a", "a2_after"),
        ("ibcmd_rs_04_ddl_bsp85_a", "s85_restored"),
    ] {
        let Some(snapshot) = Snapshot::open(db, label) else {
            eprintln!("skipped: no lab snapshot {db}/{label}");
            return;
        };
        let text = snapshot.schema();
        let schema = DbSchema::parse(&text).unwrap();
        assert!(schema.len() > 1700, "{db}: {} tables", schema.len());
        assert_eq!(schema.to_text(), text, "{db} DBSchema");

        let names_row = snapshot.row("Params", "DBNames").unwrap();
        let names_text = inflate(&names_row).unwrap();
        let names = DbNames::parse(&names_text).unwrap();
        assert!(names.entries.len() > 5000);
        assert_eq!(names.to_text(), names_text, "{db} DBNames");
        assert_eq!(DbNames::parse_row(&names.to_row().unwrap()).unwrap(), names);
    }
}

/// `dbschema_check.py` in Rust: every table (and sub-table) of the schema against the columns and
/// indexes the snapshot's database really has.
struct SqlTable {
    columns: Vec<(String, String, bool)>,
    indexes: Vec<(String, bool, bool, Vec<String>)>,
}

fn parse_schema_txt(text: &str) -> BTreeMap<String, SqlTable> {
    let mut tables = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("T ") {
            current = Some(name.to_owned());
            tables.insert(
                name.to_owned(),
                SqlTable {
                    columns: Vec::new(),
                    indexes: Vec::new(),
                },
            );
        } else if let Some(rest) = line.strip_prefix("  C ") {
            let table = tables.get_mut(current.as_ref().unwrap()).unwrap();
            let mut parts = rest.split(' ');
            let name = parts.next().unwrap().to_owned();
            let sql_type = parts.next().unwrap().to_owned();
            let nullable = parts.next().unwrap() == "NULL";
            table.columns.push((name, sql_type, nullable));
        } else if let Some(rest) = line.strip_prefix("  I ") {
            let table = tables.get_mut(current.as_ref().unwrap()).unwrap();
            let (name, rest) = rest.split_once(' ').unwrap();
            let (kind, rest) = rest.split_once(' ').unwrap();
            let (flags, rest) = rest.split_once("] (").unwrap();
            let flags = flags.trim_start_matches('[');
            let columns = rest.split(')').next().unwrap();
            table.indexes.push((
                name.to_owned(),
                flags.contains("UNIQUE") || flags.contains("PK"),
                kind == "CLUSTERED",
                columns.split(',').map(str::to_owned).collect(),
            ));
        }
    }
    tables
}

fn check_model_against_sql(db: &str, label: &str) -> Option<(usize, usize)> {
    let snapshot = Snapshot::open(db, label)?;
    let sql = parse_schema_txt(&snapshot.schema_txt());
    let schema = DbSchema::parse(&snapshot.schema()).unwrap();
    let mut tables = 0;
    let mut inline_keys = 0;
    let mut problems = Vec::new();
    for position in 0..schema.len() {
        let object = schema.view(position).unwrap();
        let mut jobs: Vec<(String, TableView<'_>, Option<TableView<'_>>)> =
            vec![(format!("_{}", object.name()), object, None)];
        for sub in object.subtables().unwrap() {
            jobs.push((
                format!("_{}_{}", object.name(), sub.name()),
                sub,
                Some(object),
            ));
        }
        for (sql_name, table, owner) in jobs {
            tables += 1;
            let Some(real) = sql.get(&sql_name) else {
                problems.push(format!("{sql_name}: missing in SQL"));
                continue;
            };
            let expected = table_columns(&table, owner.as_ref()).unwrap();
            let columns: Vec<_> = expected
                .iter()
                .map(|column| {
                    (
                        column.name.clone(),
                        column.sql_type.ddl().replace(", ", ","),
                        column.nullable,
                    )
                })
                .collect();
            if columns != real.columns {
                problems.push(format!("{sql_name}: columns differ"));
            }
            let mut expected_indexes: Vec<IndexDef> =
                declared_indexes(&table, owner.as_ref()).unwrap();
            let implicit = implicit_indexes(&table, owner.as_ref()).unwrap();
            expected_indexes.extend(implicit.iter().cloned());
            for index in &expected_indexes {
                let found = real
                    .indexes
                    .iter()
                    .any(|(name, unique, clustered, columns)| {
                        (index.name.is_empty() && name.starts_with("PK__") || *name == index.name)
                            && *unique == index.unique
                            && *clustered == index.clustered
                            && *columns == index.columns
                    });
                if !found {
                    problems.push(format!(
                        "{sql_name}: index {} is missing or different",
                        index.name
                    ));
                }
                if index.name.is_empty() {
                    inline_keys += 1;
                }
            }
            // Every real index is explained by the model.
            for (name, unique, clustered, columns) in &real.indexes {
                let explained = expected_indexes.iter().any(|index| {
                    (index.name.is_empty() && name.starts_with("PK__") || *name == index.name)
                        && index.unique == *unique
                        && index.clustered == *clustered
                        && index.columns == *columns
                });
                if !explained {
                    problems.push(format!("{sql_name}: real index {name} is not in the model"));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{db}: {} of {tables} tables differ, first: {:?}",
        problems.len(),
        &problems[..problems.len().min(6)]
    );
    Some((tables, inline_keys))
}

#[test]
fn corpus_model_reproduces_the_tables_and_indexes_of_the_database() {
    for (db, label) in [
        ("ibcmd_rs_04_ddl_bsp8327_a", "a2_after"),
        ("ibcmd_rs_04_ddl_bsp85_a", "s85_restored"),
    ] {
        match check_model_against_sql(db, label) {
            Some((tables, inline_keys)) => {
                eprintln!("{db}/{label}: {tables} tables match ({inline_keys} inline keys)");
                assert!(tables > 1700);
            }
            None => eprintln!("skipped: no lab snapshot {db}/{label}"),
        }
    }
}

/// The plan's input as it stands in a snapshot of a database that holds a staged image.
fn inputs_of(staged: &Snapshot) -> Inputs {
    let params = staged.rows("Params");
    let mut inputs = Inputs {
        schema: staged.schema(),
        main_names: staged.blob(&params[&("DBNames".to_owned(), 0)]),
        root_row: staged.row("Config", "root").unwrap_or_default(),
        ..Inputs::default()
    };
    for ((name, _), sha) in &params {
        if name.starts_with("DBNames-Ext-") {
            inputs
                .extension_names
                .push((name.clone(), staged.blob(sha)));
        }
        if name.ends_with(".si") || name == "siVersions" {
            inputs.cache_rows.push((name.clone(), staged.blob(sha)));
        }
    }
    let config = staged.rows("Config");
    let save = staged.rows("ConfigSave");
    let mut image = StagedImage {
        old_files: config.keys().map(|(name, _)| name.clone()).collect(),
        new_files: save.keys().map(|(name, _)| name.clone()).collect(),
        deleted: staged.row("ConfigSave", "deleted"),
        ..StagedImage::default()
    };
    let bare = |name: &str| name.len() == 36 && !name.contains('.');
    for ((name, part), sha) in &config {
        if *part == 0 && bare(name) {
            image.old_descriptors.insert(name.clone(), staged.blob(sha));
        }
    }
    for ((name, part), sha) in &save {
        if *part == 0 && bare(name) {
            image.new_descriptors.insert(name.clone(), staged.blob(sha));
        }
    }
    assert!(
        image.new_descriptors.len() > 4000 || image.new_descriptors.len() < 100,
        "{}",
        image.new_descriptors.len()
    );
    inputs.staged = image;
    inputs
}

/// The plan against what the native apply produced: `DBNames` text, `DBSchema` entries (but the two
/// system tables the platform upgraded on its own), and the derived caches -- the XDTO model and the
/// object registry -- as text (only the deflate stream differs).
fn assert_equals_native(plan: &crate::restructure::plan::Plan, after: &Snapshot) {
    // The derived caches.
    let cache = |name: &str| {
        plan.caches
            .iter()
            .find(|cache| cache.row_name == name)
            .unwrap_or_else(|| panic!("the plan does not update {name}"))
    };
    for name in [
        "ea13a2c9-0c2f-40fa-b855-710387e3271d.si",
        "1a621f0f-5568-4183-bd9f-f6ef670e7090.si",
    ] {
        let update = cache(name);
        let native_row = inflate(&after.row("Params", name).unwrap()).unwrap();
        let ours_row = inflate(&update.row).unwrap();
        assert_eq!(ours_row.len(), native_row.len(), "{name}");
        assert!(ours_row == native_row, "{name} differs from the platform's");
    }

    // DBNames: exactly what the platform stored.
    let native_names = inflate(&after.row("Params", "DBNames").unwrap()).unwrap();
    assert_eq!(plan.new_names_text, native_names);

    // DBSchema: every table entry is the platform's except the two system tables it upgraded on its
    // own (a platform-build drift, not a consequence of the change); the table order differs only by
    // where those two sit.
    let ours = DbSchema::parse(&plan.new_schema).unwrap();
    let native = DbSchema::parse(&after.schema()).unwrap();
    assert_eq!(ours.len(), native.len());
    let by_name = |schema: &DbSchema| -> BTreeMap<String, crate::metadata_model::brace::Brace> {
        (0..schema.len())
            .map(|position| {
                let table = &schema.tables()[position];
                (
                    schema.view(position).unwrap().name().to_owned(),
                    table.clone(),
                )
            })
            .collect()
    };
    let (ours_map, native_map) = (by_name(&ours), by_name(&native));
    assert_eq!(
        ours_map.keys().collect::<BTreeSet<_>>(),
        native_map.keys().collect::<BTreeSet<_>>()
    );
    let differing: Vec<_> = ours_map
        .iter()
        .filter(|(name, entry)| native_map[*name] != **entry)
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(differing, ["DbCopies", "DbCopiesUpdates"]);
    // The rebuilt tables sit at the end, before ConfigChngR, in the order they were rebuilt.
    assert_eq!(ours.position("ConfigChngR"), Some(ours.len() - 1));
    let rebuilt: Vec<&str> = plan
        .objects
        .iter()
        .map(|object| object.object.as_str())
        .collect();
    let tail: Vec<String> = (ours.len() - 1 - rebuilt.len()..ours.len() - 1)
        .map(|position| ours.view(position).unwrap().name().to_owned())
        .collect();
    assert_eq!(tail, rebuilt);
}

/// The plan of case a2 made from the staged snapshot against what the native apply produced.
#[test]
fn corpus_plan_of_case_a2_equals_the_native_result() {
    let (Some(staged), Some(after)) = (
        Snapshot::open("ibcmd_rs_04_ddl_bsp8327_a", "a2_staged"),
        Snapshot::open("ibcmd_rs_04_ddl_bsp8327_a", "a2_after"),
    ) else {
        eprintln!("skipped: no lab snapshots of case a2");
        return;
    };
    let plan = plan(&inputs_of(&staged), &PlanOptions::default()).unwrap();
    assert_eq!(plan.objects.len(), 1);
    assert_eq!(plan.objects[0].object, "Reference20");
    assert_equals_native(&plan, &after);

    // siVersions: the same rows in the same order, new versions for the two rows we rewrote.
    let versions = String::from_utf8(
        plan.caches
            .iter()
            .find(|cache| cache.row_name == "siVersions")
            .unwrap()
            .row
            .clone(),
    )
    .unwrap();
    let before = String::from_utf8(staged.row("Params", "siVersions").unwrap()).unwrap();
    let words = |text: &str| -> Vec<String> {
        text.split([',', '"', '{', '}'])
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (old_words, new_words) = (words(&before), words(&versions));
    assert_eq!(old_words.len(), new_words.len());
    let mut changed: Vec<String> = old_words
        .iter()
        .zip(&new_words)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, _)| old_words[index - 1].clone())
        .collect();
    changed.sort();
    assert_eq!(
        changed,
        [
            "1a621f0f-5568-4183-bd9f-f6ef670e7090.si",
            "ea13a2c9-0c2f-40fa-b855-710387e3271d.si"
        ]
        .map(str::to_owned)
        .to_vec()
    );
}

/// The types case (S1 step 1): attributes of every primitive type on five catalogs and a document, one
/// stage, against the native apply of the same stage.
#[test]
fn corpus_plan_of_the_types_case_equals_the_native_result() {
    let (Some(staged), Some(after)) = (
        Snapshot::open("ibcmd_rs_04_ddl_s1_base", "t1_staged"),
        Snapshot::open("ibcmd_rs_04_ddl_s1_t1_nat", "nat_after"),
    ) else {
        eprintln!("skipped: no lab snapshots of the types case");
        return;
    };
    let plan = plan(&inputs_of(&staged), &PlanOptions::default()).unwrap();
    let objects: Vec<&str> = plan
        .objects
        .iter()
        .map(|object| object.object.as_str())
        .collect();
    // The platform's order: the configuration's own (not the table numbers, not the names).
    assert_eq!(
        objects,
        [
            "Reference569",
            "Reference16",
            "Reference20",
            "Reference2598",
            "Reference9367",
            "Document39"
        ]
    );
    assert_equals_native(&plan, &after);
}
