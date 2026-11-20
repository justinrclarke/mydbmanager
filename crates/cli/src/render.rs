//! Rendering helpers for CLI output: catalog trees, result tables, CSV/JSON.

use comfy_table::presets::UTF8_NO_BORDERS;
use comfy_table::{Attribute, Cell, ContentArrangement, Table};
use mydb_core::introspect::{ObjectType, SchemaCatalog};
use mydb_core::query::{QueryResult, Value};

/// Render a full catalog as a readable tree, optionally filtered to one schema.
pub fn catalog(catalog: &SchemaCatalog, schema_filter: Option<&str>) -> String {
    let mut out = String::new();
    let captured = catalog.captured_at.as_deref().unwrap_or("unknown time");
    out.push_str(&format!(
        "{} — {} (captured {})\n",
        catalog.database, catalog.provider, captured
    ));

    let schemas: Vec<_> = catalog
        .schemas
        .iter()
        .filter(|s| schema_filter.is_none_or(|f| f == s.name))
        .collect();

    if schemas.is_empty() {
        out.push_str("  (no matching schemas)\n");
        return out;
    }

    for schema in schemas {
        out.push_str(&format!("\nschema {}\n", schema.name));

        if !schema.extensions.is_empty() {
            out.push_str(&format!("  extensions ({})\n", schema.extensions.len()));
            for e in &schema.extensions {
                out.push_str(&format!(
                    "    {} {}\n",
                    e.name,
                    e.version.clone().unwrap_or_default()
                ));
            }
        }
        if !schema.enums.is_empty() {
            out.push_str(&format!("  enums ({})\n", schema.enums.len()));
            for e in &schema.enums {
                out.push_str(&format!("    {} ({})\n", e.name, e.labels.join(", ")));
            }
        }
        if !schema.sequences.is_empty() {
            out.push_str(&format!("  sequences ({})\n", schema.sequences.len()));
            for s in &schema.sequences {
                let last = s
                    .last_value
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into());
                let owned = s
                    .owned_by
                    .as_ref()
                    .map(|(t, c)| format!(" owned by {t}.{c}"))
                    .unwrap_or_default();
                out.push_str(&format!("    {} last_value={last}{owned}\n", s.name));
            }
        }
        if !schema.tables.is_empty() {
            out.push_str(&format!("  tables ({})\n", schema.tables.len()));
            let mut table = Table::new();
            table.load_preset(UTF8_NO_BORDERS);
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![
                Cell::new("NAME").add_attribute(Attribute::Bold),
                Cell::new("~ROWS").add_attribute(Attribute::Bold),
                Cell::new("COLS").add_attribute(Attribute::Bold),
                Cell::new("PK").add_attribute(Attribute::Bold),
                Cell::new("FKS").add_attribute(Attribute::Bold),
                Cell::new("IDX").add_attribute(Attribute::Bold),
                Cell::new("TRIGGERS").add_attribute(Attribute::Bold),
            ]);
            for t in &schema.tables {
                table.add_row(vec![
                    Cell::new(&t.name),
                    Cell::new(
                        t.row_estimate
                            .map(|r| r.to_string())
                            .unwrap_or_else(|| "-".into()),
                    ),
                    Cell::new(t.columns.len()),
                    Cell::new(
                        t.primary_key
                            .as_ref()
                            .map(|pk| pk.columns.join(","))
                            .unwrap_or_else(|| "-".into()),
                    ),
                    Cell::new(t.foreign_keys.len()),
                    Cell::new(t.indexes.len()),
                    Cell::new(t.triggers.len()),
                ]);
            }
            for line in table.to_string().lines() {
                out.push_str(&format!("    {line}\n"));
            }
        }
        if !schema.views.is_empty() {
            out.push_str(&format!("  views ({})\n", schema.views.len()));
            for v in &schema.views {
                out.push_str(&format!("    {}\n", v.name));
            }
        }
        if !schema.materialized_views.is_empty() {
            out.push_str(&format!(
                "  materialized views ({})\n",
                schema.materialized_views.len()
            ));
            for v in &schema.materialized_views {
                out.push_str(&format!("    {}\n", v.name));
            }
        }
        if !schema.functions.is_empty() {
            out.push_str(&format!("  functions ({})\n", schema.functions.len()));
            for f in &schema.functions {
                let kind = match f.kind {
                    mydb_core::introspect::FunctionKind::Procedure => "procedure",
                    mydb_core::introspect::FunctionKind::Function => "function",
                };
                out.push_str(&format!(
                    "    {}({}) → {} [{kind}]\n",
                    f.name,
                    f.args.join(", "),
                    f.return_type
                ));
            }
        }
    }
    out
}

/// Render a query result as an aligned table.
pub fn query_table(result: &QueryResult) -> String {
    if result.columns.is_empty() {
        return format!("OK ({} rows affected)\n", result.rows_affected);
    }
    let mut table = Table::new();
    table.load_preset(UTF8_NO_BORDERS);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(
        result
            .columns
            .iter()
            .map(|c| Cell::new(&c.name).add_attribute(Attribute::Bold))
            .collect::<Vec<_>>(),
    );
    for row in &result.rows {
        table.add_row(row.iter().map(value_cell).collect::<Vec<_>>());
    }
    format!(
        "{table}\n({} row{}, {}ms)\n",
        result.rows.len(),
        if result.rows.len() == 1 { "" } else { "s" },
        result.duration.as_millis()
    )
}

fn value_cell(value: &Value) -> Cell {
    match value {
        Value::Null => Cell::new("NULL").add_attribute(Attribute::Dim),
        other => Cell::new(other.to_string()),
    }
}

/// Render a query result as CSV (RFC 4180 quoting; NULL → empty field).
pub fn query_csv(result: &QueryResult) -> String {
    let mut out = String::new();
    out.push_str(
        &result
            .columns
            .iter()
            .map(|c| csv_field(&c.name))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
    for row in &result.rows {
        let fields: Vec<String> = row
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                other => csv_field(&other.to_string()),
            })
            .collect();
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Render a query result as a JSON array of objects.
pub fn query_json(result: &QueryResult) -> String {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> = result
        .rows
        .iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            for (i, col) in result.columns.iter().enumerate() {
                map.insert(
                    col.name.clone(),
                    row.get(i)
                        .map(value_to_json)
                        .unwrap_or(serde_json::Value::Null),
                );
            }
            map
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_default()
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::UInt(u) => serde_json::Value::Number((*u).into()),
        Value::Float(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(*f).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Value::Numeric(s) => serde_json::Value::String(s.clone()),
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::Uuid(s) => serde_json::Value::String(s.clone()),
        Value::Datetime(s) => serde_json::Value::String(s.clone()),
        Value::Bytes(b) => serde_json::Value::String(hex(b)),
        Value::Json(s) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("\\x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// One-line object reference used in plans and diffs.
pub fn object_ref(kind: ObjectType, schema: &str, name: &str) -> String {
    format!("{kind} {schema}.{name}")
}
