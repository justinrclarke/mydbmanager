//! Transfer orchestrator: plan, apply, verify.

use tokio::sync::broadcast;

use crate::conn::ConnectionProfile;
use crate::diff::diff_catalogs;
use crate::dump::{
    dump_sql, objects_to_create, restore_sql, row_counts, table_copy_order, DumpOptions,
    RestoreOptions,
};
use crate::error::{MyDbError, Result};
use crate::introspect::{ObjectType, SchemaCatalog};
use crate::provider::DatabaseProvider;
use crate::sql::effective_batch_size;

use super::{
    ObjectRef, TransferEvent, TransferEventKind, TransferMode, TransferOptions, TransferPlan,
    VerificationReport,
};

/// Refuse destructive modes against production unless `assume_yes`.
pub fn guard_production(target: &ConnectionProfile, options: &TransferOptions) -> Result<()> {
    if options.mode.is_destructive() && target.env.is_production() && !options.assume_yes {
        return Err(MyDbError::NeedsConfirmation(target.name.clone()));
    }
    Ok(())
}

/// Build a reviewable plan from two catalogs.
pub fn build_plan(
    source: &SchemaCatalog,
    target: &SchemaCatalog,
    options: &TransferOptions,
) -> TransferPlan {
    let dump_opts = dump_options(options);
    let mut plan = TransferPlan {
        mode: options.mode,
        ..Default::default()
    };
    match options.mode {
        TransferMode::CleanRecreate => {
            plan.to_drop = objects_to_create(target, &DumpOptions::default());
            plan.to_create = objects_to_create(source, &dump_opts);
            if !options.schema_only {
                plan.to_copy = table_copy_order(source, &dump_opts);
            }
            plan.sequences_to_set = sequence_refs(source);
        }
        TransferMode::TruncateReload => {
            plan.to_copy = table_copy_order(source, &dump_opts);
            plan.sequences_to_set = sequence_refs(source);
            if options.schema_only {
                plan.warnings
                    .push("schema_only with truncate-reload copies no data".into());
            }
        }
        TransferMode::DiffApply => {
            let diff = diff_catalogs(source, target);
            for item in &diff.items {
                let r = ObjectRef {
                    kind: item.kind,
                    schema: item.schema.clone(),
                    name: item.name.clone(),
                };
                match item.action {
                    crate::diff::DiffAction::Drop => plan.to_drop.push(r),
                    crate::diff::DiffAction::Create | crate::diff::DiffAction::Alter => {
                        plan.to_create.push(r);
                    }
                }
            }
            if !options.schema_only {
                plan.to_copy = table_copy_order(source, &dump_opts)
                    .into_iter()
                    .filter(|c| {
                        plan.to_create.iter().any(|o| {
                            o.kind == ObjectType::Table && o.schema == c.schema && o.name == c.table
                        })
                    })
                    .collect();
            }
        }
    }
    for glob in &options.exclude_tables {
        plan.warnings
            .push(format!("excluding tables matching {glob}"));
    }
    plan
}

fn sequence_refs(source: &SchemaCatalog) -> Vec<ObjectRef> {
    source
        .schemas
        .iter()
        .flat_map(|s| {
            s.sequences.iter().map(|seq| ObjectRef {
                kind: ObjectType::Sequence,
                schema: s.name.clone(),
                name: seq.name.clone(),
            })
        })
        .collect()
}

/// Human-readable plan for `--dry-run`.
pub fn format_plan(plan: &TransferPlan) -> String {
    let mut out = format!("transfer mode: {}\n", plan.mode);
    out.push_str(&format!("drop {} objects\n", plan.to_drop.len()));
    out.push_str(&format!("create {} objects\n", plan.to_create.len()));
    out.push_str(&format!("copy {} tables\n", plan.to_copy.len()));
    for t in &plan.to_copy {
        out.push_str(&format!(
            "  {}.{} (~{} rows)\n",
            t.schema, t.table, t.est_rows
        ));
    }
    out.push_str(&format!("set {} sequences\n", plan.sequences_to_set.len()));
    for w in &plan.warnings {
        out.push_str(&format!("warning: {w}\n"));
    }
    out
}

fn dump_options(options: &TransferOptions) -> DumpOptions {
    DumpOptions {
        exclude_tables: options.exclude_tables.clone(),
        schemas: options.schemas.clone(),
        schema_only: options.schema_only,
        skip_sequences: options.skip_sequence_values,
        clean: options.mode == TransferMode::CleanRecreate,
        ..Default::default()
    }
}

/// Run a transfer. Events go to `events` when provided.
pub async fn transfer(
    source_profile: &ConnectionProfile,
    target_profile: &ConnectionProfile,
    source: &dyn DatabaseProvider,
    target: &dyn DatabaseProvider,
    mut options: TransferOptions,
    events: Option<broadcast::Sender<TransferEvent>>,
) -> Result<VerificationReport> {
    options.batch_size = effective_batch_size(options.batch_size);
    guard_production(target_profile, &options)?;
    let job_id = uuid::Uuid::new_v4().to_string();
    let emit = |event: TransferEvent| {
        if let Some(tx) = &events {
            let _ = tx.send(event);
        }
    };
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Started,
        format!("transfer {} → {}", source_profile.name, target_profile.name),
    ));
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Introspecting,
        "reading catalogs",
    ));
    let src_cat = source.introspect().await?;
    let dst_cat = target.introspect().await?;
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Planning,
        "building plan",
    ));
    let plan = build_plan(&src_cat, &dst_cat, &options);
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::PlanReady,
        format_plan(&plan).replace('\n', " | "),
    ));
    if options.dry_run {
        emit(TransferEvent::info(
            &job_id,
            TransferEventKind::Finished,
            "dry-run complete",
        ));
        return Ok(VerificationReport::compare(&src_cat, &dst_cat, &[], &[]));
    }

    let dump_opts = dump_options(&options);
    match options.mode {
        TransferMode::CleanRecreate => {
            emit(TransferEvent::info(
                &job_id,
                TransferEventKind::Creating,
                "recreating target from source dump",
            ));
            let sql = dump_sql(source, &src_cat, &dump_opts, &|e| emit(e), &job_id).await?;
            restore_sql(
                target,
                &sql,
                &RestoreOptions {
                    clean: true,
                    if_exists: true,
                    ..Default::default()
                },
                &|e| emit(e),
                &job_id,
            )
            .await?;
        }
        TransferMode::TruncateReload => {
            emit(TransferEvent::info(
                &job_id,
                TransferEventKind::Dropping,
                "truncating target tables",
            ));
            let kind = target.kind();
            for copy in table_copy_order(&dst_cat, &DumpOptions::default())
                .into_iter()
                .rev()
            {
                let q = kind.qualify(&copy.schema, &copy.table);
                let stmt = match kind {
                    crate::ProviderKind::Sqlite => format!("DELETE FROM {q}"),
                    _ => format!("TRUNCATE TABLE {q}"),
                };
                target.execute_query(&stmt).await?;
            }
            if !options.schema_only {
                let mut data_opts = dump_opts.clone();
                data_opts.data_only = true;
                data_opts.clean = false;
                let sql = dump_sql(source, &src_cat, &data_opts, &|e| emit(e), &job_id).await?;
                restore_sql(
                    target,
                    &sql,
                    &RestoreOptions::default(),
                    &|e| emit(e),
                    &job_id,
                )
                .await?;
            }
        }
        TransferMode::DiffApply => {
            emit(TransferEvent::info(
                &job_id,
                TransferEventKind::Creating,
                "applying schema diff",
            ));
            let diff = diff_catalogs(&src_cat, &dst_cat);
            let sql = diff.sql();
            if !sql.is_empty() {
                restore_sql(
                    target,
                    &sql,
                    &RestoreOptions::default(),
                    &|e| emit(e),
                    &job_id,
                )
                .await?;
            }
            if !options.schema_only && !plan.to_copy.is_empty() {
                let mut data_opts = dump_opts;
                data_opts.data_only = true;
                data_opts.tables = plan.to_copy.iter().map(|t| t.table.clone()).collect();
                let sql = dump_sql(source, &src_cat, &data_opts, &|e| emit(e), &job_id).await?;
                restore_sql(
                    target,
                    &sql,
                    &RestoreOptions::default(),
                    &|e| emit(e),
                    &job_id,
                )
                .await?;
            }
        }
    }

    if options.skip_verify {
        emit(TransferEvent::info(
            &job_id,
            TransferEventKind::Finished,
            "transfer complete (verify skipped)",
        ));
        return Ok(VerificationReport {
            passed: true,
            ..Default::default()
        });
    }
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Verifying,
        "comparing catalogs",
    ));
    let src_cat = source.introspect().await?;
    let dst_cat = target.introspect().await?;
    let src_rows = row_counts(source, &src_cat).await?;
    let dst_rows = row_counts(target, &dst_cat).await?;
    let report = VerificationReport::compare(&src_cat, &dst_cat, &src_rows, &dst_rows);
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Finished,
        if report.passed {
            "verification passed"
        } else {
            "verification found mismatches"
        }
        .to_string(),
    ));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::EnvironmentTag;

    #[test]
    fn production_guard_requires_yes() {
        let target = ConnectionProfile {
            name: "prod".into(),
            env: EnvironmentTag::Production,
            ..Default::default()
        };
        let mut opts = TransferOptions {
            mode: TransferMode::CleanRecreate,
            ..Default::default()
        };
        assert!(guard_production(&target, &opts).is_err());
        opts.assume_yes = true;
        assert!(guard_production(&target, &opts).is_ok());
    }
}
