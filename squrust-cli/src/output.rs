//! Result-set rendering in the supported output modes.

use anyhow::Result;
use clap::ValueEnum;
use comfy_table::{Cell, Table, presets::UTF8_FULL};
use squrust_sql::Value;

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum Mode {
    Table,
    Csv,
    Json,
    Line,
}

pub fn render(mode: Mode, columns: &[String], rows: &[Vec<Value>]) -> Result<()> {
    match mode {
        Mode::Table => render_table(columns, rows),
        Mode::Csv => render_csv(columns, rows)?,
        Mode::Json => render_json(columns, rows)?,
        Mode::Line => render_line(columns, rows),
    }
    Ok(())
}

fn render_table(columns: &[String], rows: &[Vec<Value>]) {
    if columns.is_empty() {
        return;
    }
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(columns.iter().map(Cell::new));
    for row in rows {
        table.add_row(row.iter().map(|v| Cell::new(display(v))));
    }
    println!("{table}");
}

fn render_csv(columns: &[String], rows: &[Vec<Value>]) -> Result<()> {
    let mut wtr = csv::Writer::from_writer(std::io::stdout());
    wtr.write_record(columns)?;
    for row in rows {
        wtr.write_record(row.iter().map(display))?;
    }
    wtr.flush()?;
    Ok(())
}

fn render_json(columns: &[String], rows: &[Vec<Value>]) -> Result<()> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut obj = serde_json::Map::new();
        for (i, v) in row.iter().enumerate() {
            let key = columns.get(i).cloned().unwrap_or_else(|| i.to_string());
            obj.insert(key, to_json(v));
        }
        out.push(serde_json::Value::Object(obj));
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::Value::Array(out))?);
    Ok(())
}

fn render_line(columns: &[String], rows: &[Vec<Value>]) {
    for row in rows {
        for (i, v) in row.iter().enumerate() {
            let key = columns.get(i).map(String::as_str).unwrap_or("");
            println!("{key} = {}", display(v));
        }
        println!();
    }
}

fn display(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        other => other.to_display_string(),
    }
}

fn to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Integer(i) => J::from(*i),
        Value::Real(r) => serde_json::Number::from_f64(*r).map(J::Number).unwrap_or(J::Null),
        Value::Boolean(b) => J::Bool(*b),
        Value::Text(t) => J::String(t.clone()),
        Value::Json(j) => j.clone(),
        Value::Blob(b) => J::String(String::from_utf8_lossy(b).into_owned()),
    }
}
