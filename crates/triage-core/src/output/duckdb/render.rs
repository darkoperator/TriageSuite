//! `views.sql` as a pure function of the inventory.
//!
//! Pure in both senses: it touches no filesystem, and it derives nothing the
//! inventory does not already state. That is what lets the SQL be
//! regenerated for a relocated collection from the inventory alone, and what
//! lets every rendering rule be tested without writing a file.
//!
//! Two views per dataset. The raw one is all-VARCHAR and cannot fail on any
//! input. The typed one is a projection over it using `TRY_CAST`, which
//! yields NULL where a hard cast would fail the whole scan -- and keeps the
//! original cell text beside every typed column, so a conversion failure
//! stays distinguishable from a value that was never written.

use crate::output::duckdb::inventory::{Dataset, DatasetFile, FileIdentity, Inventory};
use crate::output::duckdb::sql::{quote_ident, quote_literal};
use std::fmt::Write as _;

/// Make `value` safe to place after `--` on a single line.
///
/// Every CR and LF becomes a space, as does every other ASCII control
/// character. Without this a `--` comment line ends wherever the value's
/// newline falls and everything after it is live SQL in a file an analyst
/// runs against evidence. The values interpolated into these comments are
/// runtime data, not constants: a `DatasetKey::Dynamic` id is a basename
/// derived from evidence content (a SQLite table name, a registry key), and
/// `out_root` is whatever the operator passed to `--out`. Task 2 also makes
/// a CSV header field with an embedded newline a supported shape, and header
/// names reach the per-column comments.
fn comment_safe(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Render the complete `views.sql` for `inv`.
pub fn render_sql(inv: &Inventory) -> String {
    let mut out = String::new();
    render_header(&mut out, inv);
    for dataset in &inv.datasets {
        render_dataset(&mut out, inv, dataset);
    }
    if inv.datasets.is_empty() {
        let _ = writeln!(
            out,
            "-- No views: this run produced nothing queryable (status above)."
        );
    }
    out
}

fn render_header(out: &mut String, inv: &Inventory) {
    let status = serde_json::to_string(&inv.status).unwrap_or_else(|_| "\"unknown\"".into());
    let _ = writeln!(out, "-- TriageSuite DuckDB views");
    let _ = writeln!(out, "-- generation: {}", comment_safe(&inv.generation));
    let _ = writeln!(out, "-- run_id:     {}", comment_safe(&inv.run_id));
    let _ = writeln!(
        out,
        "-- out_root:   {}",
        comment_safe(&inv.out_root.display().to_string())
    );
    let _ = writeln!(
        out,
        "-- status:     {}",
        comment_safe(status.trim_matches('"'))
    );
    let _ = writeln!(
        out,
        "-- Generated from duckdb/datasets.json. Do not edit; regenerate with"
    );
    let _ = writeln!(out, "--   TriageSuite duckdb regenerate --out <root>");
    let _ = writeln!(out);
}

fn render_dataset(out: &mut String, inv: &Inventory, dataset: &Dataset) {
    let included: Vec<&DatasetFile> = dataset
        .files
        .iter()
        .filter(|f| f.included_in_view)
        .collect();
    if included.is_empty() {
        let _ = writeln!(
            out,
            "-- {} / {}: no files included in a view.",
            comment_safe(&dataset.tool),
            comment_safe(&dataset.dataset_id)
        );
        let _ = writeln!(out);
        return;
    }

    let _ = writeln!(
        out,
        "-- {} / {}",
        comment_safe(&dataset.tool),
        comment_safe(&dataset.dataset_id)
    );
    if !dataset.effective_types.is_empty() {
        let _ = writeln!(
            out,
            "-- Typed columns keep their original cell text in a __text companion:"
        );
        let _ = writeln!(
            out,
            "--   a NULL typed value with non-NULL text is a conversion failure;"
        );
        let _ = writeln!(out, "--   both NULL means the cell was blank.");
        for (column, effective) in &dataset.effective_types {
            if let Some(semantics) = &effective.time_semantics {
                let _ = writeln!(
                    out,
                    "-- {}: {}",
                    comment_safe(column),
                    comment_safe(semantics)
                );
            }
        }
    }

    // Raw view: one read_csv per file, so _triage_host and _triage_identity
    // are literals from the run's own records rather than values parsed back
    // out of a filename.
    let _ = writeln!(
        out,
        "CREATE OR REPLACE VIEW {} AS",
        quote_ident(&dataset.raw_view)
    );
    for (index, file) in included.iter().enumerate() {
        if index > 0 {
            let _ = writeln!(out, "  UNION ALL BY NAME");
        }
        render_scan(out, inv, dataset, file);
    }
    let _ = writeln!(out, ";");
    let _ = writeln!(out);

    // Typed view.
    let _ = writeln!(
        out,
        "CREATE OR REPLACE VIEW {} AS",
        quote_ident(&dataset.view)
    );
    if dataset.effective_types.is_empty() {
        let _ = writeln!(out, "  SELECT * FROM {};", quote_ident(&dataset.raw_view));
        let _ = writeln!(out);
        return;
    }
    let replacements: Vec<String> = dataset
        .effective_types
        .iter()
        .map(|(column, effective)| {
            format!(
                "TRY_CAST({} AS {}) AS {}",
                quote_ident(column),
                effective.sql_type,
                quote_ident(column)
            )
        })
        .collect();
    let _ = writeln!(out, "  SELECT * REPLACE (");
    let _ = writeln!(out, "           {}),", replacements.join(",\n           "));
    let texts: Vec<String> = dataset
        .effective_types
        .iter()
        .map(|(column, effective)| {
            format!(
                "{} AS {}",
                quote_ident(column),
                quote_ident(&effective.text_column)
            )
        })
        .collect();
    let _ = writeln!(out, "         {}", texts.join(",\n         "));
    let _ = writeln!(out, "  FROM {};", quote_ident(&dataset.raw_view));
    let _ = writeln!(out);
}

fn render_scan(out: &mut String, inv: &Inventory, dataset: &Dataset, file: &DatasetFile) {
    let absolute = inv.out_root.join(&file.path);
    let meta = &dataset.metadata_columns;
    let identity = match &file.identity {
        Some(FileIdentity::User { name }) => quote_literal(&format!("user:{name}")),
        Some(FileIdentity::System) => quote_literal("system"),
        Some(FileIdentity::Unknown) => quote_literal("unknown"),
        // A merged file spans every user whose slice fed it; its per-row
        // attribution is its own TriageUser column.
        None => "NULL".to_string(),
    };
    let _ = writeln!(
        out,
        "  SELECT {} AS {},",
        quote_literal(&inv.run_id),
        quote_ident(&meta.run_id)
    );
    let _ = writeln!(
        out,
        "         {} AS {},",
        quote_literal(&file.host),
        quote_ident(&meta.host)
    );
    let _ = writeln!(
        out,
        "         {} AS {},",
        identity,
        quote_ident(&meta.identity)
    );
    let _ = writeln!(
        out,
        "         {} AS {},",
        quote_literal(&absolute.display().to_string()),
        quote_ident(&meta.output_file)
    );
    let _ = writeln!(out, "         *");
    let options = &inv.reader_options;
    let _ = writeln!(
        out,
        "  FROM read_csv([{}],",
        quote_literal(&absolute.display().to_string())
    );
    let _ = writeln!(
        out,
        "       header={}, all_varchar={}, union_by_name={},",
        options.header, options.all_varchar, options.union_by_name
    );
    let _ = writeln!(
        out,
        "       hive_partitioning={}, delim={}, quote={}, escape={},",
        options.hive_partitioning,
        quote_literal(&options.delim),
        quote_literal(&options.quote),
        quote_literal(&options.escape)
    );
    let _ = writeln!(out, "       ignore_errors={})", options.ignore_errors);
}
