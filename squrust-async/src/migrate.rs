//! Migration runner.

use crate::connection::SqurustConnection;
use crate::error::Result;

/// A single schema migration.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub description: &'static str,
    pub sql: &'static str,
}

/// Apply all not-yet-applied migrations in ascending version order, recording
/// each in the `_squrust_migrations` bookkeeping table.
pub async fn run_migrations(conn: &SqurustConnection, migrations: &[Migration]) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS _squrust_migrations(\
            version INTEGER PRIMARY KEY, description TEXT)",
        (),
    )
    .await?;

    let applied: Vec<i64> = conn
        .query("SELECT version FROM _squrust_migrations")
        .fetch_all::<i64>()
        .await?;

    let mut ordered: Vec<&Migration> = migrations.iter().collect();
    ordered.sort_by_key(|m| m.version);

    for m in ordered {
        if applied.contains(&(m.version as i64)) {
            continue;
        }
        conn.execute(m.sql, ()).await?;
        conn.execute(
            "INSERT INTO _squrust_migrations(version, description) VALUES (?, ?)",
            (m.version as i64, m.description.to_string()),
        )
        .await?;
    }
    Ok(())
}
