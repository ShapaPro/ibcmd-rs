//! `ibcmd-rs mssql-restructure`: the research command of the prototype.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::restructure::exec::{ExecOptions, ExecReport, run as run_plan};
use crate::restructure::plan::{Inputs, Method, Plan, PlanOptions, plan};
use crate::restructure::reader::{database_name, read_inputs};
use crate::restructure::storage::STATUS_IDLE;
use crate::sql::mssql::TdsPool;
use crate::sql::{SqlLogin, SqlTarget};

#[derive(Debug, Clone, Args)]
pub struct MssqlRestructureArgs {
    /// SQL Server (`host`, `host\instance`, `host,port`); Windows login unless --sql-user is given.
    #[arg(long, default_value = "localhost")]
    pub server: String,
    #[arg(long)]
    pub sql_user: Option<String>,
    #[arg(long)]
    pub sql_pwd: Option<String>,
    #[arg(long, default_value = "IBCMD_DB_PSW")]
    pub sql_pwd_env: String,
    /// The infobase's database. Writes are refused for a database that is not a lab one
    /// (`ibcmd_rs_04_*` / `ibcmd_rs_05_*`) unless --allow-non-lab is given.
    #[arg(long)]
    pub database: String,
    /// Plan and print; write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Run the whole restructure in its transaction, verify, and roll it back.
    #[arg(long)]
    pub trial: bool,
    /// Do not refuse when other sessions are connected to the database.
    #[arg(long)]
    pub skip_session_check: bool,
    #[arg(long)]
    pub allow_non_lab: bool,
    /// The guid of the new `DBNamesVersion-DBNames` (default: random).
    #[arg(long)]
    pub names_version: Option<String>,
    /// Research: add the column with `ALTER TABLE ... ADD` (it lands at the end of the physical
    /// table) instead of rebuilding the object's tables as the platform does.
    #[arg(long)]
    pub alter_add: bool,
    /// Leave the XDTO model cache (`Params` `*.si`) as it is, stale: a new attribute then is unknown to XDTO
    /// serialization until the platform rebuilds the cache.
    #[arg(long)]
    pub skip_xdto: bool,
    /// Leave the object registry (`Params` `1a621f0f-....si`) as it is, stale: the new attribute is not
    /// listed in the search information until the platform rebuilds it.
    #[arg(long)]
    pub skip_registry: bool,
    /// Write the JSON report here as well.
    #[arg(long)]
    pub report: Option<PathBuf>,
    /// Write the new DBSchema text, the new DBNames text and the statements into this folder.
    #[arg(long)]
    pub dump_plan: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct AdditionReport {
    pub attribute: String,
    pub uuid: String,
    pub field: String,
    pub number: u64,
    pub position: usize,
}

#[derive(Debug, Serialize)]
pub struct TableReport {
    pub table: String,
    pub columns: usize,
    pub indexes: Vec<String>,
    pub copy_columns: usize,
}

#[derive(Debug, Serialize)]
pub struct StatementReport {
    pub phase: String,
    pub label: String,
    pub sql: String,
}

#[derive(Debug, Serialize)]
pub struct ObjectReport {
    pub kind: String,
    pub object: String,
    pub name: String,
    pub uuid: String,
    pub additions: Vec<AdditionReport>,
    pub tables: Vec<TableReport>,
}

#[derive(Debug, Serialize)]
pub struct RestructureReport {
    pub database: String,
    pub mode: String,
    pub summary: String,
    pub objects: Vec<ObjectReport>,
    pub caches: Vec<String>,
    pub old_schema_sha256: String,
    pub new_schema_sha256: String,
    pub new_names_sha256: String,
    pub statements: Vec<StatementReport>,
    pub execution: Option<ExecReport>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_lab_database(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["ibcmd_rs_04_", "ibcmd_rs_05_"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

pub fn run(args: &MssqlRestructureArgs) -> Result<RestructureReport> {
    let writes = !args.dry_run;
    if writes && !is_lab_database(&args.database) && !args.allow_non_lab {
        bail!(
            "{} is not a lab database (ibcmd_rs_04_* / ibcmd_rs_05_*): a restructure writes it; pass --allow-non-lab to continue",
            args.database
        );
    }
    let password = match &args.sql_user {
        Some(_) => args
            .sql_pwd
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var(&args.sql_pwd_env).ok()),
        None => None,
    };
    if args.sql_user.is_some() && password.is_none() {
        bail!(
            "SQL login {:?} needs a password: pass --sql-pwd or set {}",
            args.sql_user.as_deref().unwrap_or_default(),
            args.sql_pwd_env
        );
    }
    let target = SqlTarget {
        server: args.server.clone(),
        database: Some(args.database.clone()),
        login: SqlLogin::from_user(args.sql_user.as_deref(), password.as_deref()),
        trust_server_certificate: true,
    };
    let pool = TdsPool::new(target, 1)?;
    let mut connection = pool.dedicated()?;
    let connected = database_name(&mut connection)?;
    if !connected.eq_ignore_ascii_case(&args.database) {
        bail!("connected to {connected}, not {}", args.database);
    }

    let (inputs, storage) = read_inputs(&mut connection)?;
    let main = storage
        .iter()
        .find(|row| row.schema_id == 0)
        .context("SchemaStorage has no main row")?;
    if !main.is_idle() {
        bail!(
            "SchemaStorage is not idle (status {}, generations {} / {} bytes): an interrupted restructure has to be finished first",
            main.status,
            main.new_gen_created.len(),
            main.new_gen_dropped.len()
        );
    }
    debug_assert_eq!(main.status, STATUS_IDLE);
    let plan = make_plan(&inputs, args)?;

    let mode = if args.dry_run {
        "dry-run"
    } else if args.trial {
        "trial"
    } else {
        "apply"
    };
    let mut report = describe(&plan, &args.database, mode);
    if let Some(folder) = &args.dump_plan {
        dump(&plan, folder)?;
    }
    if writes {
        report.execution = Some(run_plan(
            &mut connection,
            &plan,
            ExecOptions {
                trial: args.trial,
                skip_session_check: args.skip_session_check,
            },
        )?);
    }
    if let Some(path) = &args.report {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(report)
}

fn make_plan(inputs: &Inputs, args: &MssqlRestructureArgs) -> Result<Plan> {
    plan(
        inputs,
        &PlanOptions {
            names_version: args.names_version.clone(),
            skip_xdto: args.skip_xdto,
            skip_registry: args.skip_registry,
            method: if args.alter_add {
                Method::AlterAdd
            } else {
                Method::Rebuild
            },
        },
    )
}

fn describe(plan: &Plan, database: &str, mode: &str) -> RestructureReport {
    RestructureReport {
        database: database.to_owned(),
        mode: mode.to_owned(),
        summary: plan.summary(),
        objects: plan
            .objects
            .iter()
            .map(|object| ObjectReport {
                kind: object.kind.label().to_owned(),
                object: object.object.clone(),
                name: object.object_name.clone(),
                uuid: object.object_uuid.clone(),
                additions: object
                    .additions
                    .iter()
                    .map(|addition| AdditionReport {
                        attribute: addition.name.clone(),
                        uuid: addition.uuid.clone(),
                        field: addition.field.name.clone(),
                        number: addition.number,
                        position: addition.position,
                    })
                    .collect(),
                tables: object
                    .tables
                    .iter()
                    .map(|table| TableReport {
                        table: table.table.name.clone(),
                        columns: table.table.columns.len(),
                        indexes: table
                            .table
                            .indexes
                            .iter()
                            .map(|index| {
                                format!(
                                    "{}{}{} ({})",
                                    if index.unique { "unique " } else { "" },
                                    if index.clustered { "clustered " } else { "" },
                                    if index.name.is_empty() {
                                        "primary key"
                                    } else {
                                        &index.name
                                    },
                                    index.columns.join(", ")
                                )
                            })
                            .collect(),
                        copy_columns: table.insert_columns.len(),
                    })
                    .collect(),
            })
            .collect(),
        caches: plan
            .caches
            .iter()
            .map(|cache| format!("{}: {}", cache.row_name, cache.what))
            .collect(),
        old_schema_sha256: plan.old_schema_sha256.clone(),
        new_schema_sha256: sha256_hex(&plan.new_schema),
        new_names_sha256: sha256_hex(&plan.new_names_text),
        statements: plan
            .statements()
            .into_iter()
            .map(|statement| StatementReport {
                phase: format!("{:?}", statement.phase).to_ascii_lowercase(),
                label: statement.label,
                sql: statement.sql.chars().take(400).collect(),
            })
            .collect(),
        execution: None,
    }
}

fn dump(plan: &Plan, folder: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(folder)?;
    std::fs::write(folder.join("DBSchema.new.txt"), &plan.new_schema)?;
    std::fs::write(folder.join("DBNames.new.txt"), &plan.new_names_text)?;
    let mut script = String::new();
    for statement in plan.statements() {
        script.push_str(&format!(
            "-- {:?}: {}\n{}\n\n",
            statement.phase, statement.label, statement.sql
        ));
    }
    std::fs::write(folder.join("restructure.sql"), script)?;
    Ok(())
}
