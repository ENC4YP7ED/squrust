//! # squrust-async
//!
//! The primary, idiomatic async Rust API for Squrust: connections, a query
//! builder with typed result mapping, row streams, transactions, a connection
//! pool, and a migration runner.
//!
//! ```no_run
//! use squrust_async::SqurustConnection;
//!
//! # async fn demo() -> squrust_async::Result<()> {
//! let conn = SqurustConnection::open_memory().await?;
//! conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)", ()).await?;
//! conn.execute("INSERT INTO t(name) VALUES (?)", ("ada",)).await?;
//! let names: Vec<String> = conn.query("SELECT name FROM t").fetch_all().await?;
//! assert_eq!(names, vec!["ada".to_string()]);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod connection;
pub mod error;
pub mod migrate;
pub mod pool;
pub mod query;
pub mod row;
pub mod stream;
pub mod transaction;

pub use connection::SqurustConnection;
pub use error::{Result, SqurustError};
pub use migrate::Migration;
pub use pool::{PooledConnection, SqurustPool};
pub use query::Query;
pub use row::SqurustRow;
pub use stream::RowStream;
pub use transaction::Transaction;

// Re-export trait surface so users need only depend on squrust-async.
pub use squrust_serde::{FromRow, FromValue, ToParams};
pub use squrust_sql::{SqlType, Value};
