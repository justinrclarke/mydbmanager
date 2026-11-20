//! Environment-to-environment transfer engine (Phase 3).
//!
//! Pipeline: introspect source → build [`TransferPlan`] → apply to target →
//! verify (object counts, row counts, sequence values) → report. Progress is
//! published as [`TransferEvent`]s: the CLI renders them as progress bars,
//! the desktop app forwards them as Tauri events.

mod engine;

pub use engine::{build_plan, format_plan, guard_production, transfer};

use serde::{Deserialize, Serialize};

use crate::introspect::{ObjectType, SchemaCatalog};

/// How the target database is brought in line with the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransferMode {
    /// Drop and rebuild everything on the target (prod → dev refresh).
    #[default]
    CleanRecreate,
    /// Keep the target schema, truncate tables and reload data.
    TruncateReload,
    /// Apply only the schema diff (+ optional data for new tables).
    DiffApply,
}

impl TransferMode {
    /// Canonical kebab-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            TransferMode::CleanRecreate => "clean-recreate",
            TransferMode::TruncateReload => "truncate-reload",
            TransferMode::DiffApply => "diff-apply",
        }
    }

    /// True when the mode destroys existing target objects/data.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            TransferMode::CleanRecreate | TransferMode::TruncateReload
        )
    }
}

impl std::fmt::Display for TransferMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TransferMode {
    type Err = crate::MyDbError;

    fn from_str(s: &str) -> crate::Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "clean-recreate" | "clean_recreate" | "recreate" => Ok(TransferMode::CleanRecreate),
            "truncate-reload" | "truncate_reload" | "truncate" => Ok(TransferMode::TruncateReload),
            "diff-apply" | "diff_apply" | "diff" => Ok(TransferMode::DiffApply),
            other => Err(crate::MyDbError::InvalidValue(format!(
                "unknown transfer mode '{other}' (expected clean-recreate, truncate-reload or diff-apply)"
            ))),
        }
    }
}

/// User-facing knobs for a transfer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct TransferOptions {
    /// Target handling mode.
    pub mode: TransferMode,
    /// Restrict to these schemas (empty = all).
    pub schemas: Vec<String>,
    /// Tables to skip (glob patterns allowed, e.g. `audit_*`).
    pub exclude_tables: Vec<String>,
    /// Copy schema but no data.
    pub schema_only: bool,
    /// Leave target sequences at their defaults instead of copying source
    /// values (off by default — sequence values are copied).
    pub skip_sequence_values: bool,
    /// Insert batch size for the native data copier.
    pub batch_size: usize,
    /// Build and report the plan without touching the target.
    pub dry_run: bool,
    /// Skip the post-transfer verification pass.
    pub skip_verify: bool,
    /// Non-interactive confirmation (required for production targets).
    pub assume_yes: bool,
}

/// One table scheduled for data copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableCopy {
    /// Schema name.
    pub schema: String,
    /// Table name.
    pub table: String,
    /// Estimated row count from source statistics.
    pub est_rows: u64,
}

/// A fully qualified object reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    /// Object kind.
    pub kind: ObjectType,
    /// Schema name.
    pub schema: String,
    /// Object name.
    pub name: String,
}

impl std::fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}.{}", self.kind, self.schema, self.name)
    }
}

/// What the engine intends to do — reviewed by the user before execution
/// (`--dry-run` / the wizard's plan screen) and after (`verify`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransferPlan {
    /// Mode the plan was built for.
    pub mode: TransferMode,
    /// Objects that will be dropped on the target.
    pub to_drop: Vec<ObjectRef>,
    /// Objects that will be created on the target, in dependency order.
    pub to_create: Vec<ObjectRef>,
    /// Tables whose data will be copied, in FK-safe order.
    pub to_copy: Vec<TableCopy>,
    /// Sequences whose values will be set after data load.
    pub sequences_to_set: Vec<ObjectRef>,
    /// Non-fatal issues (excluded tables, unsupported objects, ...).
    pub warnings: Vec<String>,
}

/// Progress/lifecycle event kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransferEventKind {
    /// Transfer started.
    Started,
    /// Reading the source catalog.
    Introspecting,
    /// Building the execution plan.
    Planning,
    /// Plan is ready for review.
    PlanReady,
    /// Dropping target objects.
    Dropping,
    /// Creating objects on the target.
    Creating,
    /// Copying table data.
    Copying,
    /// Setting sequence values.
    Sequences,
    /// Verifying the result.
    Verifying,
    /// Transfer completed successfully.
    Finished,
    /// Something worth surfacing that did not abort the run.
    Warning,
    /// Terminal failure.
    Error,
}

impl TransferEventKind {
    /// Kebab-case name for logs and CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            TransferEventKind::Started => "started",
            TransferEventKind::Introspecting => "introspecting",
            TransferEventKind::Planning => "planning",
            TransferEventKind::PlanReady => "plan-ready",
            TransferEventKind::Dropping => "dropping",
            TransferEventKind::Creating => "creating",
            TransferEventKind::Copying => "copying",
            TransferEventKind::Sequences => "sequences",
            TransferEventKind::Verifying => "verifying",
            TransferEventKind::Finished => "finished",
            TransferEventKind::Warning => "warning",
            TransferEventKind::Error => "error",
        }
    }
}

/// A single progress event, streamed to CLI or UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferEvent {
    /// Correlates events belonging to one run.
    pub job_id: String,
    /// Event kind.
    pub kind: TransferEventKind,
    /// Human-readable message.
    pub message: String,
    /// Table the event refers to, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// Rows processed so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_done: Option<u64>,
    /// Total rows expected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_total: Option<u64>,
}

impl TransferEvent {
    /// Simple lifecycle message.
    pub fn info(
        job_id: impl Into<String>,
        kind: TransferEventKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            kind,
            message: message.into(),
            table: None,
            rows_done: None,
            rows_total: None,
        }
    }

    /// Per-table copy progress.
    pub fn progress(
        job_id: impl Into<String>,
        table: impl Into<String>,
        rows_done: u64,
        rows_total: u64,
    ) -> Self {
        let table = table.into();
        Self {
            job_id: job_id.into(),
            kind: TransferEventKind::Copying,
            message: format!("copying {table}"),
            table: Some(table),
            rows_done: Some(rows_done),
            rows_total: Some(rows_total),
        }
    }
}

/// Outcome of the verification pass comparing source and target.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerificationReport {
    /// Per-type object counts on the source.
    pub source_counts: crate::introspect::ObjectCounts,
    /// Per-type object counts on the target.
    pub target_counts: crate::introspect::ObjectCounts,
    /// Per-table row count mismatches (`(schema.table, source, target)`).
    pub row_mismatches: Vec<(String, u64, u64)>,
    /// Sequences whose target value differs from the source.
    pub sequence_mismatches: Vec<(String, Option<i128>, Option<i128>)>,
    /// Whether everything matched.
    pub passed: bool,
}

impl VerificationReport {
    /// Build a report from two catalogs and per-table row counts.
    pub fn compare(
        source: &SchemaCatalog,
        target: &SchemaCatalog,
        source_rows: &[(String, u64)],
        target_rows: &[(String, u64)],
    ) -> Self {
        let source_counts = source.object_counts();
        let target_counts = target.object_counts();

        let row_mismatches: Vec<(String, u64, u64)> = source_rows
            .iter()
            .filter_map(|(table, src_count)| {
                let dst_count = target_rows
                    .iter()
                    .find(|(t, _)| t == table)
                    .map(|(_, c)| *c)
                    .unwrap_or(0);
                (dst_count != *src_count).then(|| (table.clone(), *src_count, dst_count))
            })
            .collect();

        let mut sequence_mismatches = Vec::new();
        for schema in &source.schemas {
            for seq in &schema.sequences {
                let target_value = target
                    .find_sequence(&schema.name, &seq.name)
                    .and_then(|s| s.last_value);
                if target_value != seq.last_value {
                    sequence_mismatches.push((
                        format!("{}.{}", schema.name, seq.name),
                        seq.last_value,
                        target_value,
                    ));
                }
            }
        }

        let passed = source_counts == target_counts
            && row_mismatches.is_empty()
            && sequence_mismatches.is_empty();

        Self {
            source_counts,
            target_counts,
            row_mismatches,
            sequence_mismatches,
            passed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_mode_parses_and_flags_destructive() {
        assert_eq!(
            "clean-recreate".parse::<TransferMode>().unwrap(),
            TransferMode::CleanRecreate
        );
        assert_eq!(
            "truncate".parse::<TransferMode>().unwrap(),
            TransferMode::TruncateReload
        );
        assert_eq!(
            "diff".parse::<TransferMode>().unwrap(),
            TransferMode::DiffApply
        );
        assert!(TransferMode::CleanRecreate.is_destructive());
        assert!(TransferMode::TruncateReload.is_destructive());
        assert!(!TransferMode::DiffApply.is_destructive());
        assert!("merge".parse::<TransferMode>().is_err());
    }

    #[test]
    fn defaults_copy_sequence_values() {
        let opts = TransferOptions::default();
        assert!(!opts.skip_sequence_values);
        assert_eq!(opts.batch_size, 0); // engine applies its own default
        assert_eq!(opts.mode, TransferMode::CleanRecreate);
    }

    #[test]
    fn verification_detects_missing_sequence_value() {
        let source = SchemaCatalog {
            database: "src".into(),
            provider: crate::ProviderKind::Postgres,
            captured_at: None,
            schemas: vec![{
                let mut s = crate::introspect::Schema::new("public");
                s.sequences.push(crate::introspect::Sequence {
                    name: "users_id_seq".into(),
                    data_type: "bigint".into(),
                    start: 1,
                    increment: 1,
                    min: 1,
                    max: i64::MAX as i128,
                    cache: 1,
                    cycle: false,
                    last_value: Some(42),
                    owned_by: None,
                });
                s
            }],
        };
        let mut target = source.clone();
        target.database = "dst".into();
        target.schemas[0].sequences[0].last_value = Some(1);

        let report = VerificationReport::compare(&source, &target, &[], &[]);
        assert!(!report.passed);
        assert_eq!(report.sequence_mismatches.len(), 1);
        assert_eq!(report.sequence_mismatches[0].1, Some(42));
        assert_eq!(report.sequence_mismatches[0].2, Some(1));
    }
}
