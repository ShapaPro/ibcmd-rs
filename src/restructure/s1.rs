//! The structural gate of the own apply for the minimal set S1 (docs/apply/restructuring.md, section 12).
//!
//! The apply asks its gate whether the staged configuration is something it may move. The conservative
//! gate refuses every descriptor whose text differs. This gate lets through the changes of the set S1 --
//! built so far: new attributes of catalogs and documents -- and hands the apply the structure work to run
//! in its own transaction ([`StructurePhase`]); everything else stays a refusal, with the reason.
//!
//! The decision is a chain, and every link fails closed:
//!
//! 1. the conservative gate's verdict: the descriptors that differ are its blockers;
//! 2. the restructuring check (`apply_check::check_staged`) names each change (its *reasons*); every reason
//!    has to be one S1 operation ([`classify`]) -- a `data` or `unknown` reason, another kind of object,
//!    another property, an S1 operation that is designed but not built: refusal;
//! 3. the plan ([`crate::restructure::plan::plan`]) reads the descriptors itself and must arrive at the same
//!    objects and attributes as the check ([`decide`]: two independent decoders agreeing is the evidence);
//! 4. the blockers of the planned objects' descriptors are withdrawn -- and if any other blocker is left,
//!    the stage holds more than the restructuring and the refusal stands.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::apply_check::{Reason, ReasonClass, Verdict, check_staged};
use crate::mssql_config_apply::gate::{
    ConservativeGate, GateInput, GateVerdict, StructuralGate, StructurePhase,
};
use crate::mssql_config_apply::sqlgen::ParamsRewrite;
use crate::restructure::plan::{Inputs, Plan, PlanOptions, plan};
use crate::restructure::reader::{ClientSource, read_inputs};
use crate::sql::SqlExec;

pub const GATE_NAME: &str = "s1";

/// One operation of S1 that a reason of the restructuring check names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    /// `ChildObjects/Attribute[X]: added`.
    AddAttribute(String),
    /// Designed (section 12.3) but not built in this version: the operation and what it is about.
    NotBuilt {
        operation: &'static str,
        subject: String,
    },
}

/// The operations of one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectOperations {
    /// `Catalog._ДемоПартнеры`.
    pub object: String,
    /// The descriptor row (the object's uuid).
    pub row: String,
    pub operations: Vec<Operation>,
}

/// A reason the stage is not S1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub row: String,
    pub reason: String,
}

/// `Attribute[X]` -> `X`.
fn named(segment: &str, tag: &str) -> Option<String> {
    segment
        .strip_prefix(tag)?
        .strip_prefix('[')?
        .strip_suffix(']')
        .map(str::to_owned)
}

/// The S1 operation a reason of the check names, or why it is none.
pub fn classify_reason(reason: &Reason) -> std::result::Result<Operation, String> {
    let said = reason.summary();
    if reason.class != ReasonClass::Structure {
        return Err(format!(
            "{said}: the check calls it a {} change",
            reason.class.label()
        ));
    }
    let word = reason.change.split_whitespace().next().unwrap_or_default();
    let segments: Vec<&str> = if reason.property.is_empty() {
        Vec::new()
    } else {
        reason.property.split('/').collect()
    };
    let Some((kind, name)) = reason.object.split_once('.') else {
        // `Configuration: ChildObjects/Catalog[X]: added` -- the other half of a new object.
        if reason.object == "Configuration"
            && word == "added"
            && let ["ChildObjects", child] = segments.as_slice()
            && (named(child, "Catalog").is_some() || named(child, "Document").is_some())
        {
            return Ok(Operation::NotBuilt {
                operation: "add an object",
                subject: reason.property.clone(),
            });
        }
        return Err(format!("{said}: not a catalog or a document"));
    };
    if kind != "Catalog" && kind != "Document" {
        return Err(format!(
            "{said}: {kind} is not in S1 (catalogs and documents)"
        ));
    }
    match (segments.as_slice(), word) {
        ([], "added") => Ok(Operation::NotBuilt {
            operation: "add an object",
            subject: name.to_owned(),
        }),
        (["ChildObjects", child, rest @ ..], _) => {
            if let Some(attribute) = named(child, "Attribute") {
                match (rest, word) {
                    ([], "added") => Ok(Operation::AddAttribute(attribute)),
                    ([], "removed") => Ok(Operation::NotBuilt {
                        operation: "delete an attribute",
                        subject: attribute,
                    }),
                    (["Properties", "Type", "StringQualifiers", "Length"], _) => {
                        Ok(Operation::NotBuilt {
                            operation: "widen a string",
                            subject: attribute,
                        })
                    }
                    (["Properties", "Indexing"], _) => Ok(Operation::NotBuilt {
                        operation: "switch the index of an attribute",
                        subject: attribute,
                    }),
                    _ => Err(format!("{said}: no S1 operation covers it")),
                }
            } else if let Some(section) = named(child, "TabularSection") {
                match (rest, word) {
                    ([], "added") => Ok(Operation::NotBuilt {
                        operation: "add a tabular section",
                        subject: section,
                    }),
                    _ => Err(format!("{said}: no S1 operation covers it")),
                }
            } else {
                Err(format!("{said}: no S1 operation covers it"))
            }
        }
        _ => Err(format!("{said}: no S1 operation covers it")),
    }
}

/// Every reason of the check as an S1 operation, by object; the reasons that are none, or whose operation
/// is not built, as refusals.
pub fn classify(verdict: &Verdict) -> (Vec<ObjectOperations>, Vec<Refusal>) {
    let mut objects: BTreeMap<String, ObjectOperations> = BTreeMap::new();
    let mut refusals = Vec::new();
    if verdict.incomplete {
        refusals.push(Refusal {
            row: String::new(),
            reason: "the check did not compare every body that carries data".to_owned(),
        });
    }
    for reason in &verdict.reasons {
        match classify_reason(reason) {
            Ok(Operation::NotBuilt { operation, subject }) => refusals.push(Refusal {
                row: reason.file_name.clone(),
                reason: format!(
                    "{}: the S1 operation \"{operation}\" ({subject}) is designed (docs/apply/restructuring.md, 12.3) but not built in this version",
                    reason.object
                ),
            }),
            Ok(operation) => objects
                .entry(reason.file_name.to_ascii_lowercase())
                .or_insert_with(|| ObjectOperations {
                    object: reason.object.clone(),
                    row: reason.file_name.clone(),
                    operations: Vec::new(),
                })
                .operations
                .push(operation),
            Err(text) => refusals.push(Refusal {
                row: reason.file_name.clone(),
                reason: text,
            }),
        }
    }
    if verdict.needs_restructuring && verdict.reasons.is_empty() {
        refusals.push(Refusal {
            row: String::new(),
            reason: "the check needs a restructuring and names no reason".to_owned(),
        });
    }
    (objects.into_values().collect(), refusals)
}

fn sha256_upper(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// The plan as the apply takes it: T-SQL for its transaction and the cache rows as guarded rewrites.
fn phase_of(plan: &Plan, inputs: &Inputs) -> Result<StructurePhase> {
    let mut params_rewrites = Vec::new();
    for cache in &plan.caches {
        let stored = inputs
            .cache_rows
            .iter()
            .find(|(name, _)| *name == cache.row_name)
            .map(|(_, bytes)| bytes)
            .ok_or_else(|| anyhow::anyhow!("Params.{} was not read", cache.row_name))?;
        params_rewrites.push(ParamsRewrite {
            file_name: cache.row_name.clone(),
            old_data_size: inputs
                .cache_sizes
                .get(&cache.row_name)
                .copied()
                .unwrap_or(stored.len() as i64),
            old_sha256_hex: sha256_upper(stored),
            new_bytes: cache.row.clone(),
            set_creation: cache.set_creation,
        });
    }
    Ok(StructurePhase {
        sql: plan.phase_sql("@now")?,
        params_rewrites,
        tables: plan
            .tables()
            .map(|table| table.table.name.clone())
            .collect(),
        objects: plan
            .objects
            .iter()
            .map(|object| {
                format!(
                    "{} {} ({}): new attributes {}",
                    object.kind.label(),
                    object.object_name,
                    object.object,
                    object
                        .additions
                        .iter()
                        .map(|addition| format!("{} = {}", addition.field.name, addition.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect(),
        caches: plan
            .caches
            .iter()
            .map(|cache| format!("Params.{}: {}", cache.row_name, cache.what))
            .collect(),
    })
}

/// The decision, from what the database said: the conservative verdict, the check's verdict, the plan's
/// input. Pure, so that the refusals are testable without a database.
pub fn decide(
    mut verdict: GateVerdict,
    check: &Verdict,
    inputs: &Inputs,
    options: &PlanOptions,
) -> (GateVerdict, Option<StructurePhase>) {
    verdict.gate = format!("{GATE_NAME} (the conservative rule and the S1 restructurings)");
    if !verdict.restructuring_required {
        // Nothing the conservative gate refuses: a plain apply.
        return (verdict, None);
    }
    let refuse = |mut verdict: GateVerdict, row: &str, reason: String| {
        verdict.block(row, format!("S1: {reason}"));
        (verdict, None)
    };

    let (operations, refusals) = classify(check);
    if !refusals.is_empty() {
        for refusal in refusals {
            verdict.block(&refusal.row, format!("S1: {}", refusal.reason));
        }
        return (verdict, None);
    }
    if operations.is_empty() {
        return refuse(
            verdict,
            "",
            "the conservative gate refuses descriptors, the restructuring check names no change in them"
                .to_owned(),
        );
    }

    let plan = match plan(inputs, options) {
        Ok(plan) => plan,
        Err(error) => return refuse(verdict, "", format!("{error:#}")),
    };

    // The two decoders agree: the same objects, the same new attributes.
    let planned: BTreeMap<String, BTreeSet<&str>> = plan
        .objects
        .iter()
        .map(|object| {
            (
                object.object_uuid.to_ascii_lowercase(),
                object
                    .additions
                    .iter()
                    .map(|addition| addition.name.as_str())
                    .collect(),
            )
        })
        .collect();
    let named_by_check: BTreeMap<String, BTreeSet<&str>> = operations
        .iter()
        .map(|object| {
            (
                object.row.to_ascii_lowercase(),
                object
                    .operations
                    .iter()
                    .filter_map(|operation| match operation {
                        Operation::AddAttribute(name) => Some(name.as_str()),
                        Operation::NotBuilt { .. } => None,
                    })
                    .collect(),
            )
        })
        .collect();
    if planned != named_by_check {
        return refuse(
            verdict,
            "",
            format!(
                "the plan changes {planned:?} and the restructuring check names {named_by_check:?}: the two decoders disagree"
            ),
        );
    }

    // The planned objects' descriptors are the blockers the plan answers for; another blocker is a
    // change this gate does not cover.
    verdict
        .blockers
        .retain(|blocker| !planned.contains_key(&blocker.row.to_ascii_lowercase()));
    verdict.restructuring_required = !verdict.blockers.is_empty() || verdict.blockers_omitted > 0;
    if verdict.restructuring_required {
        return (verdict, None);
    }
    match phase_of(&plan, inputs) {
        Ok(phase) => (verdict, Some(phase)),
        Err(error) => refuse(verdict, "", format!("{error:#}")),
    }
}

/// The gate: the conservative rule plus the S1 restructurings.
pub struct S1Gate<'a> {
    sql: &'a SqlExec,
    conservative: ConservativeGate,
    options: PlanOptions,
    prepared: RefCell<Option<StructurePhase>>,
}

impl<'a> S1Gate<'a> {
    pub fn new(sql: &'a SqlExec, conservative: ConservativeGate, options: PlanOptions) -> Self {
        Self {
            sql,
            conservative,
            options,
            prepared: RefCell::new(None),
        }
    }
}

impl StructuralGate for S1Gate<'_> {
    fn name(&self) -> &'static str {
        GATE_NAME
    }

    fn check(&self, input: &GateInput<'_>) -> Result<GateVerdict> {
        self.prepared.borrow_mut().take();
        let verdict = self.conservative.check(input)?;
        if !verdict.restructuring_required {
            return Ok(GateVerdict {
                gate: GATE_NAME.to_owned(),
                ..verdict
            });
        }
        // The stage holds a change the conservative gate refuses: is it S1?
        let check = check_staged(self.sql, input.database, None)?;
        let mut source = ClientSource {
            client: input.client,
            database: input.database,
        };
        let (inputs, storage) = read_inputs(&mut source)?;
        let idle = storage
            .iter()
            .find(|row| row.schema_id == 0)
            .is_some_and(|row| row.is_idle());
        if !idle {
            let mut verdict = verdict;
            verdict.block(
                "SchemaStorage",
                "S1: the schema storage is not idle: an interrupted restructuring has to be finished first",
            );
            return Ok(verdict);
        }
        let (verdict, phase) = decide(verdict, &check, &inputs, &self.options);
        *self.prepared.borrow_mut() = phase;
        Ok(verdict)
    }

    fn take_structure(&self) -> Option<StructurePhase> {
        self.prepared.borrow_mut().take()
    }
}
