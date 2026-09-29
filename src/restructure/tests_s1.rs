//! The S1 gate: which reasons of the restructuring check are operations, and what the gate lets through
//! (the fixtures of case a2 stand in for the database).

use super::tests_plan::{
    CATALOG, NEW_ROW, OLD_ROW, client_as_string, inputs, options, string_client_inputs,
    widen_client,
};
use crate::apply_check::{Reason, ReasonClass, Verdict};
use crate::mssql_config_apply::gate::{GateBlocker, GateVerdict};
use crate::restructure::s1::{Operation, classify, classify_reason, decide};

fn reason(class: ReasonClass, object: &str, property: &str, change: &str) -> Reason {
    Reason {
        class,
        object: object.to_owned(),
        file_name: CATALOG.to_owned(),
        property: property.to_owned(),
        change: change.to_owned(),
    }
}

fn structure(object: &str, property: &str, change: &str) -> Reason {
    reason(ReasonClass::Structure, object, property, change)
}

/// The check's verdict for case a2: one new attribute.
fn check_of_a2() -> Verdict {
    Verdict {
        needs_restructuring: true,
        reasons: vec![structure(
            "Catalog._ДемоПартнеры",
            "ChildObjects/Attribute[ДемоНовыйРеквизит]",
            "added (a column is added or dropped)",
        )],
        ..Verdict::default()
    }
}

/// The conservative gate's verdict: the descriptors that differ are blockers.
fn conservative(rows: &[&str]) -> GateVerdict {
    GateVerdict {
        restructuring_required: !rows.is_empty(),
        blockers: rows
            .iter()
            .map(|row| GateBlocker {
                row: (*row).to_owned(),
                reason: "the descriptor's text differs from the active one: a metadata change, possibly structural".to_owned(),
            })
            .collect(),
        gate: "conservative".to_owned(),
        ..GateVerdict::default()
    }
}

#[test]
fn the_reasons_of_the_check_are_s1_operations_or_refusals() {
    let attribute = |property: &str, change: &str| {
        classify_reason(&structure(
            "Catalog._ДемоПартнеры",
            &format!("ChildObjects/Attribute[Х]{property}"),
            change,
        ))
    };
    // Built.
    assert_eq!(
        attribute("", "added (a column is added or dropped)"),
        Ok(Operation::AddAttribute("Х".to_owned()))
    );
    assert_eq!(
        classify_reason(&structure(
            "Document._ДемоЗаказПокупателя",
            "ChildObjects/Attribute[Х]",
            "added (a column is added or dropped)"
        )),
        Ok(Operation::AddAttribute("Х".to_owned()))
    );
    // Designed, not built: named as such.
    let not_built = |result: Result<Operation, String>| match result {
        Ok(Operation::NotBuilt { operation, .. }) => operation,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        attribute("", "removed (a column is added or dropped)"),
        Ok(Operation::RemoveAttribute("Х".to_owned()))
    );
    assert_eq!(
        attribute(
            "/Properties/Type/StringQualifiers/Length",
            "50 -> 100 (a property of an attribute no rule covers)"
        ),
        Ok(Operation::WidenString("Х".to_owned()))
    );
    assert_eq!(
        not_built(attribute(
            "/Properties/Indexing",
            "DontIndex -> Index (a property of an attribute no rule covers)"
        )),
        "switch the index of an attribute"
    );
    assert_eq!(
        not_built(classify_reason(&structure(
            "Catalog._ДемоКонтрагенты",
            "ChildObjects/TabularSection[ДемоНоваяТЧ]",
            "added (a tabular section is added, dropped or moved)"
        ))),
        "add a tabular section"
    );
    assert_eq!(
        not_built(classify_reason(&structure(
            "Catalog.ДемоНовыйСправочник",
            "",
            "added (Catalog; an object that owns tables or stored data is added or dropped)"
        ))),
        "add an object"
    );
    assert_eq!(
        not_built(classify_reason(&structure(
            "Configuration",
            "ChildObjects/Catalog[ДемоНовыйСправочник]",
            "added (an object that owns tables or stored data is added or dropped)"
        ))),
        "add an object"
    );

    // Refused: another class, another kind, another property.
    let data = classify_reason(&reason(
        ReasonClass::Data,
        "Catalog.X",
        "Predefined",
        "row changed",
    ));
    assert!(data.unwrap_err().contains("data change"));
    let unknown = classify_reason(&reason(
        ReasonClass::Unknown,
        "Enum.X",
        "",
        "the Enum row differs (5190 -> 5444 bytes) but both sides decode to the same XML",
    ));
    assert!(unknown.unwrap_err().contains("unknown change"));
    let register = classify_reason(&structure(
        "AccumulationRegister.X",
        "ChildObjects/Resource[Р]",
        "added (a column is added or dropped)",
    ));
    assert!(register.unwrap_err().contains("not in S1"));
    let length = classify_reason(&structure(
        "Catalog.X",
        "Properties/CodeLength",
        "9 -> 12 (a property no rule covers)",
    ));
    assert!(length.unwrap_err().contains("no S1 operation covers it"));
    let column_property = classify_reason(&structure(
        "Catalog.X",
        "ChildObjects/Attribute[А]/Properties/Type/NumberQualifiers/Precision",
        "10 -> 12",
    ));
    assert!(column_property.is_err());
}

#[test]
fn a_verdict_is_split_into_objects_and_refusals() {
    let mut verdict = check_of_a2();
    let (objects, refusals) = classify(&verdict);
    assert!(refusals.is_empty());
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].object, "Catalog._ДемоПартнеры");
    assert_eq!(
        objects[0].operations,
        [Operation::AddAttribute("ДемоНовыйРеквизит".to_owned())]
    );

    // One built and one not built: the stage is refused as a whole, and says which.
    verdict.reasons.push(structure(
        "Catalog._ДемоПартнеры",
        "ChildObjects/Attribute[Х]/Properties/Indexing",
        "DontIndex -> Index (a property of an attribute no rule covers)",
    ));
    let (_, refusals) = classify(&verdict);
    assert_eq!(refusals.len(), 1);
    assert!(refusals[0].reason.contains("designed"), "{refusals:?}");

    // A check that needs a restructuring and names nothing is refused too.
    let silent = Verdict {
        needs_restructuring: true,
        ..Verdict::default()
    };
    assert_eq!(classify(&silent).1.len(), 1);
}

#[test]
fn a_planned_attribute_is_let_through_and_its_blocker_withdrawn() {
    let (verdict, phase) = decide(
        conservative(&[CATALOG]),
        &check_of_a2(),
        &inputs(OLD_ROW, NEW_ROW),
        &options(),
    );
    assert!(!verdict.restructuring_required, "{:?}", verdict.blockers);
    assert!(verdict.blockers.is_empty());
    assert!(verdict.gate.starts_with("s1"));
    let phase = phase.expect("a structure phase");
    assert_eq!(
        phase.tables,
        ["_Reference20", "_Reference20_VT155", "_Reference20_VT159"]
    );
    assert_eq!(phase.objects.len(), 1);
    assert!(phase.objects[0].contains("Fld11034 = ДемоНовыйРеквизит"));
    assert!(phase.sql.contains("create table dbo._Reference20NG"));
    assert!(phase.sql.contains("Modified = @now"));
    // The fixtures hold no cache rows and the options leave the caches alone.
    assert!(phase.params_rewrites.is_empty());
}

#[test]
fn a_widened_string_is_let_through_and_narrowing_is_not() {
    let widened = widen_client(
        &client_as_string(OLD_ROW, 50),
        "{\"S\",50,1}",
        "{\"S\",200,1}",
    );
    let staged = string_client_inputs(&widened);
    let check = Verdict {
        needs_restructuring: true,
        reasons: vec![structure(
            "Catalog._ДемоПартнеры",
            "ChildObjects/Attribute[Клиент]/Properties/Type/StringQualifiers/Length",
            "50 -> 200 (a property of an attribute no rule covers)",
        )],
        ..Verdict::default()
    };
    let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &staged, &options());
    assert!(!verdict.restructuring_required, "{:?}", verdict.blockers);
    let phase = phase.expect("a structure phase");
    assert!(phase.objects[0].contains("widened attributes"));
    assert!(phase.objects[0].contains("(50 -> 200)"));
    assert!(phase.params_rewrites.is_empty() && phase.caches.is_empty());
    assert!(phase.sql.contains("create table dbo._Reference20NG"));

    // The check names another attribute than the plan widens: the decoders disagree.
    let mut other = check.clone();
    other.reasons[0].property =
        "ChildObjects/Attribute[Другой]/Properties/Type/StringQualifiers/Length".to_owned();
    let (verdict, phase) = decide(conservative(&[CATALOG]), &other, &staged, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        verdict
            .blockers
            .iter()
            .any(|blocker| blocker.reason.contains("disagree")),
        "{:?}",
        verdict.blockers
    );

    // A shorter limit: the check names it the same way, the plan refuses it.
    let narrowed = widen_client(
        &client_as_string(OLD_ROW, 50),
        "{\"S\",50,1}",
        "{\"S\",20,1}",
    );
    let (verdict, phase) = decide(
        conservative(&[CATALOG]),
        &check,
        &string_client_inputs(&narrowed),
        &options(),
    );
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        verdict
            .blockers
            .iter()
            .any(|blocker| blocker.reason.contains("the limit is shorter")),
        "{:?}",
        verdict.blockers
    );
}

#[test]
fn nothing_is_let_through_that_the_gate_does_not_cover() {
    let base = inputs(OLD_ROW, NEW_ROW);

    // Nothing to restructure: a plain apply.
    let (verdict, phase) = decide(conservative(&[]), &check_of_a2(), &base, &options());
    assert!(!verdict.restructuring_required && phase.is_none());

    // Another descriptor differs and nothing explains it: the refusal stands, for that row.
    let (verdict, phase) = decide(
        conservative(&[CATALOG, "ffffffff-0000-4000-8000-000000000000"]),
        &check_of_a2(),
        &base,
        &options(),
    );
    assert!(verdict.restructuring_required && phase.is_none());
    assert_eq!(verdict.blockers.len(), 1);
    assert_eq!(
        verdict.blockers[0].row,
        "ffffffff-0000-4000-8000-000000000000"
    );

    // An operation that is not built.
    let mut check = check_of_a2();
    check.reasons.push(structure(
        "Catalog._ДемоПартнеры",
        "ChildObjects/Attribute[Х]/Properties/Indexing",
        "DontIndex -> Index (a property of an attribute no rule covers)",
    ));
    let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        verdict.blockers.iter().any(
            |blocker| blocker.reason.starts_with("S1: ") && blocker.reason.contains("designed")
        ),
        "{:?}",
        verdict.blockers
    );

    // The check and the plan disagree about the attribute.
    let mut check = check_of_a2();
    check.reasons[0].property = "ChildObjects/Attribute[ДругойРеквизит]".to_owned();
    let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        verdict
            .blockers
            .iter()
            .any(|blocker| blocker.reason.contains("disagree")),
        "{:?}",
        verdict.blockers
    );

    // A conservative verdict with more blockers than it lists.
    let mut crowded = conservative(&[CATALOG]);
    crowded.blockers_omitted = 3;
    let (verdict, phase) = decide(crowded, &check_of_a2(), &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());

    // The plan refuses (the images are the same: no attribute is new).
    let (verdict, phase) = decide(
        conservative(&[CATALOG]),
        &check_of_a2(),
        &inputs(OLD_ROW, OLD_ROW),
        &options(),
    );
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        verdict
            .blockers
            .iter()
            .any(|blocker| blocker.reason.contains("adds no attribute")),
        "{:?}",
        verdict.blockers
    );
}
