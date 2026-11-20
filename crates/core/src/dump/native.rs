//! Native dump/restore driven by [`SchemaCatalog`].

use crate::dump::{DumpOptions, RestoreOptions};
use crate::error::Result;
use crate::introspect::{
    CheckConstraint, Column, ForeignKey, Function, Index, Schema, SchemaCatalog, Sequence, Table,
    View,
};
use crate::provider::{DatabaseProvider, ProviderKind};
use crate::sql::{effective_batch_size, is_excluded, split_statements, sql_literal};
use crate::transfer::{ObjectRef, TableCopy, TransferEvent, TransferEventKind};

/// Emit schema (+ optional data) SQL for `catalog` using `provider` for row reads.
pub async fn dump_sql(
    provider: &dyn DatabaseProvider,
    catalog: &SchemaCatalog,
    options: &DumpOptions,
    emit: &impl Fn(TransferEvent),
    job_id: &str,
) -> Result<String> {
    let kind = catalog.provider;
    let mut sql = String::from("-- MyDB native dump (LF line endings)\n");
    sql.push_str(&format!("-- database: {}\n\n", catalog.database));
    sql.push_str(&preamble(kind));
    sql.push('\n');

    if options.clean {
        sql.push_str(&drop_sql(catalog, options));
        sql.push('\n');
    }

    if !options.data_only {
        sql.push_str(&schema_sql(catalog, options));
        sql.push('\n');
    }

    if !options.schema_only {
        emit(TransferEvent::info(
            job_id,
            TransferEventKind::Copying,
            "copying table data",
        ));
        sql.push_str(&data_sql(provider, catalog, options, &emit, job_id).await?);
        sql.push('\n');
        if !options.skip_sequences {
            sql.push_str(&sequence_values_sql(catalog, options));
        }
    }

    sql.push_str(&postamble(kind));
    if !sql.ends_with('\n') {
        sql.push('\n');
    }
    Ok(sql)
}

/// Apply a native SQL script to `provider`.
pub async fn restore_sql(
    provider: &dyn DatabaseProvider,
    script: &str,
    options: &RestoreOptions,
    emit: &impl Fn(TransferEvent),
    job_id: &str,
) -> Result<usize> {
    if options.dry_run {
        let n = split_statements(script).len();
        emit(TransferEvent::info(
            job_id,
            TransferEventKind::PlanReady,
            format!("dry-run: {n} statements"),
        ));
        return Ok(n);
    }
    let stmts = split_statements(script);
    let total = stmts.len();
    for (i, stmt) in stmts.iter().enumerate() {
        if i % 25 == 0 {
            emit(TransferEvent::info(
                job_id,
                TransferEventKind::Creating,
                format!("executing statement {}/{total}", i + 1),
            ));
        }
        provider.execute_query(stmt).await?;
    }
    Ok(total)
}

/// Schema-only SQL (no data, no setval).
pub fn schema_sql(catalog: &SchemaCatalog, options: &DumpOptions) -> String {
    let kind = catalog.provider;
    let mut out = String::new();
    for schema in filtered_schemas(catalog, options) {
        if kind.capabilities().schemas {
            out.push_str(&format!(
                "CREATE SCHEMA IF NOT EXISTS {};\n",
                kind.quote_ident(&schema.name)
            ));
        }
        for ext in &schema.extensions {
            out.push_str(&format!(
                "CREATE EXTENSION IF NOT EXISTS {};\n",
                kind.quote_ident(&ext.name)
            ));
        }
        for en in &schema.enums {
            let labels: Vec<_> = en
                .labels
                .iter()
                .map(|l| crate::sql::quote_string(l))
                .collect();
            out.push_str(&format!(
                "CREATE TYPE {} AS ENUM ({});\n",
                kind.qualify(&schema.name, &en.name),
                labels.join(", ")
            ));
        }
        if !options.skip_sequences && kind.capabilities().sequences {
            for seq in &schema.sequences {
                out.push_str(&create_sequence(kind, &schema.name, seq));
            }
        }
        for table in &schema.tables {
            if is_excluded(&schema.name, &table.name, &options.exclude_tables) {
                continue;
            }
            if !options.tables.is_empty()
                && !options
                    .tables
                    .iter()
                    .any(|t| t == &table.name || t == &format!("{}.{}", schema.name, table.name))
            {
                continue;
            }
            out.push_str(&create_table(kind, &schema.name, table));
        }
        for table in &schema.tables {
            if is_excluded(&schema.name, &table.name, &options.exclude_tables) {
                continue;
            }
            out.push_str(&table_constraints(kind, &schema.name, table));
        }
        for table in &schema.tables {
            if is_excluded(&schema.name, &table.name, &options.exclude_tables) {
                continue;
            }
            for idx in &table.indexes {
                out.push_str(&create_index(kind, &schema.name, &table.name, idx));
            }
        }
        for view in &schema.views {
            out.push_str(&create_view(kind, &schema.name, view, false));
        }
        for view in &schema.materialized_views {
            out.push_str(&create_view(kind, &schema.name, view, true));
        }
        for func in &schema.functions {
            out.push_str(&create_function(func));
        }
        for table in &schema.tables {
            for trig in &table.triggers {
                out.push_str(&trig.definition.trim_end().replace('\r', ""));
                if !trig.definition.trim_end().ends_with(';') {
                    out.push(';');
                }
                out.push('\n');
            }
        }
    }
    out
}

fn filtered_schemas<'a>(catalog: &'a SchemaCatalog, options: &DumpOptions) -> Vec<&'a Schema> {
    catalog
        .schemas
        .iter()
        .filter(|s| options.schemas.is_empty() || options.schemas.iter().any(|n| n == &s.name))
        .collect()
}

fn preamble(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Postgres => "SET client_encoding = 'UTF8';\nSET standard_conforming_strings = on;\nSET session_replication_role = replica;\n".into(),
        ProviderKind::Mysql => "SET FOREIGN_KEY_CHECKS=0;\nSET NAMES utf8mb4;\n".into(),
        ProviderKind::Sqlite => "PRAGMA foreign_keys=OFF;\n".into(),
    }
}

fn postamble(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Postgres => "SET session_replication_role = DEFAULT;\n".into(),
        ProviderKind::Mysql => "SET FOREIGN_KEY_CHECKS=1;\n".into(),
        ProviderKind::Sqlite => "PRAGMA foreign_keys=ON;\n".into(),
    }
}

fn drop_sql(catalog: &SchemaCatalog, options: &DumpOptions) -> String {
    let kind = catalog.provider;
    let mut out = String::new();
    for schema in filtered_schemas(catalog, options).into_iter().rev() {
        for table in schema.tables.iter().rev() {
            if is_excluded(&schema.name, &table.name, &options.exclude_tables) {
                continue;
            }
            for trig in table.triggers.iter().rev() {
                out.push_str(&drop_trigger(kind, &schema.name, &table.name, &trig.name));
            }
        }
        for view in schema.materialized_views.iter().rev() {
            out.push_str(&format!(
                "DROP MATERIALIZED VIEW IF EXISTS {};\n",
                kind.qualify(&schema.name, &view.name)
            ));
        }
        for view in schema.views.iter().rev() {
            out.push_str(&format!(
                "DROP VIEW IF EXISTS {};\n",
                kind.qualify(&schema.name, &view.name)
            ));
        }
        for func in schema.functions.iter().rev() {
            let q = kind.qualify(&schema.name, &func.name);
            match kind {
                ProviderKind::Mysql => out.push_str(&format!("DROP FUNCTION IF EXISTS {q};\n")),
                _ => out.push_str(&format!("DROP FUNCTION IF EXISTS {q} CASCADE;\n")),
            }
        }
        for table in schema.tables.iter().rev() {
            if is_excluded(&schema.name, &table.name, &options.exclude_tables) {
                continue;
            }
            let q = kind.qualify(&schema.name, &table.name);
            match kind {
                ProviderKind::Postgres => {
                    out.push_str(&format!("DROP TABLE IF EXISTS {q} CASCADE;\n"))
                }
                _ => out.push_str(&format!("DROP TABLE IF EXISTS {q};\n")),
            }
        }
        if !options.skip_sequences {
            for seq in schema.sequences.iter().rev() {
                out.push_str(&format!(
                    "DROP SEQUENCE IF EXISTS {} CASCADE;\n",
                    kind.qualify(&schema.name, &seq.name)
                ));
            }
        }
        for en in schema.enums.iter().rev() {
            out.push_str(&format!(
                "DROP TYPE IF EXISTS {} CASCADE;\n",
                kind.qualify(&schema.name, &en.name)
            ));
        }
    }
    out
}

fn drop_trigger(kind: ProviderKind, schema: &str, table: &str, name: &str) -> String {
    match kind {
        ProviderKind::Postgres => format!(
            "DROP TRIGGER IF EXISTS {} ON {};\n",
            kind.quote_ident(name),
            kind.qualify(schema, table)
        ),
        _ => format!("DROP TRIGGER IF EXISTS {};\n", kind.quote_ident(name)),
    }
}

fn create_sequence(kind: ProviderKind, schema: &str, seq: &Sequence) -> String {
    format!(
        "CREATE SEQUENCE {} AS {} START WITH {} INCREMENT BY {} MINVALUE {} MAXVALUE {} CACHE {}{};\n",
        kind.qualify(schema, &seq.name),
        seq.data_type,
        seq.start,
        seq.increment,
        seq.min,
        seq.max,
        seq.cache,
        if seq.cycle { " CYCLE" } else { " NO CYCLE" }
    )
}

fn create_table(kind: ProviderKind, schema: &str, table: &Table) -> String {
    let mut cols = Vec::new();
    for col in &table.columns {
        cols.push(column_sql(kind, col));
    }
    if let Some(pk) = &table.primary_key {
        let names: Vec<_> = pk.columns.iter().map(|c| kind.quote_ident(c)).collect();
        let cname = pk
            .name
            .as_deref()
            .map(|n| format!("CONSTRAINT {} ", kind.quote_ident(n)))
            .unwrap_or_default();
        cols.push(format!("{cname}PRIMARY KEY ({})", names.join(", ")));
    }
    if kind == ProviderKind::Sqlite {
        for fk in &table.foreign_keys {
            cols.push(inline_fk(kind, fk));
        }
        for chk in &table.checks {
            let expr = chk.expression.trim();
            let expr = expr.strip_prefix("CHECK").unwrap_or(expr).trim();
            cols.push(format!(
                "CONSTRAINT {} CHECK {expr}",
                kind.quote_ident(&chk.name)
            ));
        }
    }
    format!(
        "CREATE TABLE {} (\n  {}\n);\n",
        kind.qualify(schema, &table.name),
        cols.join(",\n  ")
    )
}

fn inline_fk(kind: ProviderKind, fk: &ForeignKey) -> String {
    let cols: Vec<_> = fk.columns.iter().map(|c| kind.quote_ident(c)).collect();
    let refs: Vec<_> = fk.ref_columns.iter().map(|c| kind.quote_ident(c)).collect();
    let mut s = format!(
        "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        kind.quote_ident(&fk.name),
        cols.join(", "),
        kind.qualify(&fk.ref_schema, &fk.ref_table),
        refs.join(", ")
    );
    if let Some(d) = &fk.on_delete {
        s.push_str(&format!(" ON DELETE {d}"));
    }
    if let Some(u) = &fk.on_update {
        s.push_str(&format!(" ON UPDATE {u}"));
    }
    s
}

fn column_sql(kind: ProviderKind, col: &Column) -> String {
    let mut s = format!("{} {}", kind.quote_ident(&col.name), col.data_type);
    if col.is_identity {
        match kind {
            ProviderKind::Postgres => {
                if let Some(gen) = &col.identity_generation {
                    s.push_str(&format!(" GENERATED {gen} AS IDENTITY"));
                }
            }
            ProviderKind::Mysql => s.push_str(" AUTO_INCREMENT"),
            ProviderKind::Sqlite => {}
        }
    }
    if let Some(expr) = &col.generated_expr {
        s.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED"));
    }
    if !col.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(default) = &col.default {
        s.push_str(" DEFAULT ");
        s.push_str(default);
    }
    s
}

fn table_constraints(kind: ProviderKind, schema: &str, table: &Table) -> String {
    if kind == ProviderKind::Sqlite {
        return String::new();
    }
    let mut out = String::new();
    let t = kind.qualify(schema, &table.name);
    for fk in &table.foreign_keys {
        out.push_str(&fk_sql(kind, &t, fk));
    }
    for chk in &table.checks {
        out.push_str(&check_sql(kind, &t, chk));
    }
    out
}

fn fk_sql(kind: ProviderKind, table: &str, fk: &ForeignKey) -> String {
    let cols: Vec<_> = fk.columns.iter().map(|c| kind.quote_ident(c)).collect();
    let refs: Vec<_> = fk.ref_columns.iter().map(|c| kind.quote_ident(c)).collect();
    let mut s = format!(
        "ALTER TABLE {table} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        kind.quote_ident(&fk.name),
        cols.join(", "),
        kind.qualify(&fk.ref_schema, &fk.ref_table),
        refs.join(", ")
    );
    if let Some(d) = &fk.on_delete {
        s.push_str(&format!(" ON DELETE {d}"));
    }
    if let Some(u) = &fk.on_update {
        s.push_str(&format!(" ON UPDATE {u}"));
    }
    s.push_str(";\n");
    s
}

fn check_sql(kind: ProviderKind, table: &str, chk: &CheckConstraint) -> String {
    let expr = chk.expression.trim();
    let expr = expr.strip_prefix("CHECK").unwrap_or(expr).trim();
    format!(
        "ALTER TABLE {table} ADD CONSTRAINT {} CHECK {expr};\n",
        kind.quote_ident(&chk.name)
    )
}

fn create_index(kind: ProviderKind, schema: &str, table: &str, idx: &Index) -> String {
    if idx.name.starts_with("sqlite_autoindex_") && !idx.unique {
        return String::new();
    }
    let index_name = if idx.name.starts_with("sqlite_autoindex_") {
        format!("{table}_uq_{}", idx.columns.join("_"))
    } else {
        idx.name.clone()
    };
    let unique = if idx.unique { "UNIQUE " } else { "" };
    let cols: Vec<_> = idx.columns.iter().map(|c| kind.quote_ident(c)).collect();
    let mut s = format!(
        "CREATE {unique}INDEX {} ON {} ({})",
        kind.quote_ident(&index_name),
        kind.qualify(schema, table),
        cols.join(", ")
    );
    if kind == ProviderKind::Postgres {
        if let Some(method) = &idx.method {
            s = format!(
                "CREATE {unique}INDEX {} ON {} USING {method} ({})",
                kind.quote_ident(&idx.name),
                kind.qualify(schema, table),
                cols.join(", ")
            );
        }
        if let Some(pred) = &idx.partial {
            s.push_str(&format!(" WHERE {pred}"));
        }
    }
    s.push_str(";\n");
    s
}

fn create_view(kind: ProviderKind, schema: &str, view: &View, materialized: bool) -> String {
    let kw = if materialized {
        "CREATE MATERIALIZED VIEW"
    } else {
        "CREATE VIEW"
    };
    let def = view.definition.trim().trim_end_matches(';');
    format!("{kw} {} AS\n{def};\n", kind.qualify(schema, &view.name))
}

fn create_function(func: &Function) -> String {
    let mut def = func.definition.trim_end().replace('\r', "");
    if def.is_empty() {
        return String::new();
    }
    if !def.ends_with(';') {
        def.push(';');
    }
    def.push('\n');
    def
}

fn sequence_values_sql(catalog: &SchemaCatalog, options: &DumpOptions) -> String {
    let kind = catalog.provider;
    if !kind.capabilities().sequences {
        return String::new();
    }
    let mut out = String::new();
    for schema in filtered_schemas(catalog, options) {
        for seq in &schema.sequences {
            if let Some(v) = seq.last_value {
                out.push_str(&format!(
                    "SELECT setval('{}', {v}, true);\n",
                    kind.qualify(&schema.name, &seq.name).replace('"', "")
                ));
            }
        }
    }
    out
}

async fn data_sql(
    provider: &dyn DatabaseProvider,
    catalog: &SchemaCatalog,
    options: &DumpOptions,
    emit: &impl Fn(TransferEvent),
    job_id: &str,
) -> Result<String> {
    let kind = catalog.provider;
    let batch = effective_batch_size(0);
    let mut out = String::new();
    for copy in table_copy_order(catalog, options) {
        let qualified = kind.qualify(&copy.schema, &copy.table);
        emit(TransferEvent::progress(
            job_id,
            &copy.table,
            0,
            copy.est_rows,
        ));
        let result = provider
            .execute_query(&format!("SELECT * FROM {qualified}"))
            .await?;
        if result.rows.is_empty() {
            continue;
        }
        let col_names: Vec<_> = result
            .columns
            .iter()
            .map(|c| kind.quote_ident(&c.name))
            .collect();
        let col_list = col_names.join(", ");
        for chunk in result.rows.chunks(batch) {
            let mut tuples = Vec::with_capacity(chunk.len());
            for row in chunk {
                let vals: Vec<_> = row.iter().map(|v| sql_literal(kind, v)).collect();
                tuples.push(format!("({})", vals.join(", ")));
            }
            out.push_str(&format!(
                "INSERT INTO {qualified} ({col_list}) VALUES\n  {};\n",
                tuples.join(",\n  ")
            ));
        }
        emit(TransferEvent::progress(
            job_id,
            &copy.table,
            result.rows.len() as u64,
            result.rows.len() as u64,
        ));
    }
    Ok(out)
}

/// FK-safe table order for copies.
pub fn table_copy_order(catalog: &SchemaCatalog, options: &DumpOptions) -> Vec<TableCopy> {
    type Node = (String, String, u64, Vec<(String, String)>);
    let mut nodes: Vec<Node> = Vec::new();
    for schema in filtered_schemas(catalog, options) {
        for table in &schema.tables {
            if is_excluded(&schema.name, &table.name, &options.exclude_tables) {
                continue;
            }
            if !options.tables.is_empty()
                && !options
                    .tables
                    .iter()
                    .any(|t| t == &table.name || t == &format!("{}.{}", schema.name, table.name))
            {
                continue;
            }
            let deps: Vec<(String, String)> = table
                .foreign_keys
                .iter()
                .map(|fk| (fk.ref_schema.clone(), fk.ref_table.clone()))
                .filter(|(s, t)| !(s == &schema.name && t == &table.name))
                .collect();
            nodes.push((
                schema.name.clone(),
                table.name.clone(),
                table.row_estimate.unwrap_or(0),
                deps,
            ));
        }
    }
    let mut remaining = nodes;
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        if let Some(idx) = remaining.iter().position(|(_, _, _, deps)| {
            deps.iter().all(|(s, t)| {
                ordered
                    .iter()
                    .any(|c: &TableCopy| &c.schema == s && &c.table == t)
                    || !remaining.iter().any(|(rs, rt, _, _)| rs == s && rt == t)
            })
        }) {
            let (schema, table, est, _) = remaining.remove(idx);
            ordered.push(TableCopy {
                schema,
                table,
                est_rows: est,
            });
        } else {
            let (schema, table, est, _) = remaining.remove(0);
            ordered.push(TableCopy {
                schema,
                table,
                est_rows: est,
            });
        }
    }
    ordered
}

/// Objects that would be created from `catalog` (for transfer plans).
pub fn objects_to_create(catalog: &SchemaCatalog, options: &DumpOptions) -> Vec<ObjectRef> {
    use crate::introspect::ObjectType;
    let mut out = Vec::new();
    for schema in filtered_schemas(catalog, options) {
        for en in &schema.enums {
            out.push(ObjectRef {
                kind: ObjectType::Enum,
                schema: schema.name.clone(),
                name: en.name.clone(),
            });
        }
        for seq in &schema.sequences {
            out.push(ObjectRef {
                kind: ObjectType::Sequence,
                schema: schema.name.clone(),
                name: seq.name.clone(),
            });
        }
        for table in &schema.tables {
            if is_excluded(&schema.name, &table.name, &options.exclude_tables) {
                continue;
            }
            out.push(ObjectRef {
                kind: ObjectType::Table,
                schema: schema.name.clone(),
                name: table.name.clone(),
            });
        }
        for view in &schema.views {
            out.push(ObjectRef {
                kind: ObjectType::View,
                schema: schema.name.clone(),
                name: view.name.clone(),
            });
        }
        for view in &schema.materialized_views {
            out.push(ObjectRef {
                kind: ObjectType::MaterializedView,
                schema: schema.name.clone(),
                name: view.name.clone(),
            });
        }
        for func in &schema.functions {
            out.push(ObjectRef {
                kind: ObjectType::Function,
                schema: schema.name.clone(),
                name: func.name.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspect::{PrimaryKey, Schema};

    fn tiny_catalog() -> SchemaCatalog {
        let mut catalog = SchemaCatalog::new("appdb", ProviderKind::Postgres);
        let mut schema = Schema::new("public");
        schema.tables.push(Table {
            name: "users".into(),
            columns: vec![Column {
                name: "id".into(),
                ordinal: 1,
                data_type: "bigint".into(),
                nullable: false,
                default: None,
                is_identity: true,
                identity_generation: Some("BY DEFAULT".into()),
                identity_start: None,
                identity_increment: None,
                generated_expr: None,
                comment: None,
            }],
            primary_key: Some(PrimaryKey {
                name: Some("users_pkey".into()),
                columns: vec!["id".into()],
            }),
            ..Default::default()
        });
        catalog.schemas.push(schema);
        catalog
    }

    #[test]
    fn emits_create_table() {
        let sql = schema_sql(&tiny_catalog(), &DumpOptions::default());
        assert!(sql.contains("CREATE TABLE \"public\".\"users\""));
        assert!(sql.contains("PRIMARY KEY"));
        assert!(!sql.contains('\r'), "SQL must be LF-only");
    }
}
