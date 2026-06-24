//! Interactive REPL with multi-line buffering and dot-commands.

use anyhow::Result;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use squrust_async::{SqurustConnection, Value};

use crate::output::Mode;

pub async fn run(conn: &SqurustConnection, mut mode: Mode) -> Result<()> {
    let mut editor = DefaultEditor::new()?;
    let history = history_path();
    if let Some(h) = &history {
        let _ = editor.load_history(h);
    }

    println!("squrust shell — enter SQL terminated by ';', or .help for commands");
    let mut buffer = String::new();

    loop {
        let prompt = if buffer.is_empty() { "sq> " } else { "...> " };
        match editor.readline(prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if buffer.is_empty() && trimmed.starts_with('.') {
                    let _ = editor.add_history_entry(line.as_str());
                    if handle_dot(conn, trimmed, &mut mode).await? {
                        break;
                    }
                    continue;
                }
                buffer.push_str(&line);
                buffer.push('\n');
                if buffer.trim_end().ends_with(';') {
                    let sql = std::mem::take(&mut buffer);
                    let _ = editor.add_history_entry(sql.trim());
                    crate::run_sql(conn, &sql, mode).await?;
                }
            }
            Err(ReadlineError::Interrupted) => {
                buffer.clear();
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("{e}");
                break;
            }
        }
    }

    if let Some(h) = &history {
        let _ = editor.save_history(h);
    }
    Ok(())
}

/// Returns `Ok(true)` when the shell should exit.
async fn handle_dot(conn: &SqurustConnection, cmd: &str, mode: &mut Mode) -> Result<bool> {
    let mut parts = cmd.split_whitespace();
    let name = parts.next().unwrap_or("");
    match name {
        ".exit" | ".quit" => return Ok(true),
        ".help" => print_help(),
        ".tables" => {
            let names = conn.table_names();
            println!("{}", names.join("  "));
        }
        ".schema" => {
            let table = parts.next();
            for sql in conn.schema(table) {
                let sql = sql.trim_end_matches(';');
                println!("{sql};");
            }
        }
        ".mode" => match parts.next().map(parse_mode) {
            Some(Some(m)) => *mode = m,
            _ => eprintln!("usage: .mode table|csv|json|line"),
        },
        ".import" => {
            let file = parts.next();
            let table = parts.next();
            match (file, table) {
                (Some(f), Some(t)) => import_csv(conn, f, t).await?,
                _ => eprintln!("usage: .import FILE TABLE"),
            }
        }
        other => eprintln!("unknown command: {other} (try .help)"),
    }
    Ok(false)
}

async fn import_csv(conn: &SqurustConnection, file: &str, table: &str) -> Result<()> {
    let mut rdr = csv::Reader::from_path(file)?;
    let headers: Vec<String> = rdr.headers()?.iter().map(|s| s.to_string()).collect();
    let cols = headers.join(", ");
    let placeholders = vec!["?"; headers.len()].join(", ");
    let sql = format!("INSERT INTO {table}({cols}) VALUES ({placeholders})");

    let mut count = 0u64;
    for record in rdr.records() {
        let record = record?;
        let params: Vec<Value> = record.iter().map(|s| Value::Text(s.to_string())).collect();
        conn.execute(&sql, params).await?;
        count += 1;
    }
    println!("imported {count} rows into {table}");
    Ok(())
}

fn parse_mode(s: &str) -> Option<Mode> {
    match s.to_ascii_lowercase().as_str() {
        "table" => Some(Mode::Table),
        "csv" => Some(Mode::Csv),
        "json" => Some(Mode::Json),
        "line" => Some(Mode::Line),
        _ => None,
    }
}

fn print_help() {
    println!(
        ".tables            List tables\n\
         .schema [TABLE]    Show CREATE statements\n\
         .mode MODE         Set output mode (table, csv, json, line)\n\
         .import FILE TABLE Import a CSV file into a table\n\
         .help              Show this help\n\
         .exit / .quit      Exit"
    );
}

fn history_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".sq_history"))
}
