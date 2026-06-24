//! `sq` — a drop-in-ish command-line shell for Squrust, modelled on `sqlite3`.

mod output;
mod repl;

use std::io::{IsTerminal, Read};

use anyhow::Result;
use clap::Parser;
use squrust_async::SqurustConnection;

use output::Mode;

#[derive(Parser, Debug)]
#[command(name = "sq", version, about = "Squrust SQL shell")]
struct Args {
    /// Database file (use ":memory:" for a transient database).
    #[arg(default_value = ":memory:")]
    database: String,

    /// SQL to run. If omitted, read from stdin (when piped) or start a REPL.
    sql: Option<String>,

    /// Output mode.
    #[arg(long, value_enum, default_value_t = Mode::Table)]
    mode: Mode,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let conn = if args.database == ":memory:" {
        SqurustConnection::open_memory().await?
    } else {
        SqurustConnection::open(&args.database).await?
    };

    if let Some(sql) = args.sql {
        run_sql(&conn, &sql, args.mode).await?;
    } else if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        run_sql(&conn, &buf, args.mode).await?;
    } else {
        repl::run(&conn, args.mode).await?;
    }

    // Fold the WAL into the main file so the on-disk `.db` is a complete,
    // stock-readable SQLite database.
    if args.database != ":memory:" {
        let _ = conn.checkpoint();
    }
    Ok(())
}

/// Parse a (possibly multi-statement) string and run each statement, rendering
/// result sets and reporting affected-row counts for DML.
pub async fn run_sql(conn: &SqurustConnection, sql: &str, mode: Mode) -> Result<()> {
    let statements = squrust_sql::parser::parse(sql)?;
    for stmt in &statements {
        let text = stmt.to_string();
        if is_query(&text) {
            match conn.fetch_raw(&text, ()).await {
                Ok((cols, rows)) => output::render(mode, &cols, &rows)?,
                Err(e) => eprintln!("Error: {e}"),
            }
        } else {
            match conn.execute(&text, ()).await {
                Ok(_) => {}
                Err(e) => eprintln!("Error: {e}"),
            }
        }
    }
    Ok(())
}

pub fn is_query(sql: &str) -> bool {
    let kw = sql
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(kw.as_str(), "SELECT" | "WITH" | "VALUES")
}
