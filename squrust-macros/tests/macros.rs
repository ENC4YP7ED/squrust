//! Integration tests for the Squrust derive and function-like macros.

use squrust_async::{SqurustConnection, ToParams};
use squrust_macros::{FromRow, ToParams, migrate, sql};

#[derive(Debug, PartialEq, FromRow, ToParams)]
struct User {
    id: i64,
    name: String,
    email: Option<String>,
}

#[tokio::test]
async fn derive_from_row_and_to_params() {
    let conn = SqurustConnection::open_memory().await.unwrap();
    conn.execute(
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, email TEXT)",
        (),
    )
    .await
    .unwrap();

    let alice = User {
        id: 1,
        name: "alice".into(),
        email: Some("a@example.com".into()),
    };
    let bob = User {
        id: 2,
        name: "bob".into(),
        email: None,
    };

    // ToParams derive feeds the positional bind list.
    conn.execute(
        "INSERT INTO users(id, name, email) VALUES (?, ?, ?)",
        alice.to_params(),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO users(id, name, email) VALUES (?, ?, ?)",
        bob.to_params(),
    )
    .await
    .unwrap();

    // FromRow derive maps result rows back to the struct. The query text is
    // validated against squrust.schema at compile time by sql!.
    let users: Vec<User> = conn
        .query(sql!("SELECT id, name, email FROM users ORDER BY id"))
        .fetch_all()
        .await
        .unwrap();

    assert_eq!(
        users,
        vec![
            User {
                id: 1,
                name: "alice".into(),
                email: Some("a@example.com".into())
            },
            User {
                id: 2,
                name: "bob".into(),
                email: None
            },
        ]
    );
}

#[tokio::test]
async fn migrate_macro_embeds_files() {
    let conn = SqurustConnection::open_memory().await.unwrap();
    // migrate! reads ./migrations relative to CARGO_MANIFEST_DIR at compile time.
    conn.migrate(migrate!("migrations")).await.unwrap();

    let titles: Vec<String> = conn
        .query("SELECT title FROM posts ORDER BY id")
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(titles, vec!["hello".to_string()]);
}

#[test]
fn sql_macro_returns_validated_str() {
    // A valid statement expands to its (validated) string literal.
    let s: &str = sql!("SELECT name FROM users WHERE id = 1");
    assert_eq!(s, "SELECT name FROM users WHERE id = 1");
}
