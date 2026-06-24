# Derive Macros (`squrust-macros`)

Proc macros for typed rows and compile-time SQL. The runtime traits live in
`squrust-serde` (re-exported from `squrust-async`); the macros live in
`squrust-macros`.

```toml
[dependencies]
squrust-async  = { git = "https://github.com/ENC4YP7ED/squrust" }
squrust-macros = { git = "https://github.com/ENC4YP7ED/squrust" }
```

## `#[derive(FromRow)]`

Maps result columns to struct fields **by name**.

```rust
use squrust_macros::FromRow;

#[derive(Debug, FromRow)]
struct User {
    id: i64,
    name: String,
    email: Option<String>,        // NULL → None
    #[squrust(rename = "created")] // read from a differently-named column
    created_at: i64,
    #[squrust(skip)]               // not read; uses Default::default()
    cache: Vec<u8>,
}

let users: Vec<User> = conn.query("SELECT id, name, email, created FROM users")
    .fetch_all()
    .await?;
```

Each field type must implement `FromValue` (all the primitives, `Option<T>`,
`Vec<u8>`, `serde_json::Value` do).

## `#[derive(ToParams)]`

Turns a struct into a positional bind list (field order), honoring
`#[squrust(skip)]`.

```rust
use squrust_async::ToParams;            // the trait
use squrust_macros::ToParams;           // the derive macro (different namespace)

#[derive(ToParams)]
struct NewUser { name: String, email: Option<String> }

let u = NewUser { name: "ada".into(), email: None };
conn.execute("INSERT INTO users(name, email) VALUES (?, ?)", u.to_params()).await?;
```

> The trait and the derive macro share the name `ToParams` but live in different
> namespaces, so importing both is fine — `#[derive(ToParams)]` resolves the
> macro; `T: ToParams` resolves the trait.

## `sql!` — compile-time-checked SQL

`sql!("...")` validates the SQL against a schema file **at compile time** and
expands to the validated `&str`. Unknown tables (and, for single-table queries,
unknown columns) become compile errors.

```rust
let users: Vec<User> = conn
    .query(sql!("SELECT id, name, email FROM users ORDER BY id"))
    .fetch_all()
    .await?;
```

The schema is read from `$SQURUST_SCHEMA`, or `squrust.schema` in
`$CARGO_MANIFEST_DIR`. It's just a file of `CREATE TABLE` statements:

```sql
-- squrust.schema
CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, email TEXT);
CREATE TABLE posts(id INTEGER PRIMARY KEY, title TEXT, body TEXT);
```

A typo is caught before runtime:

```rust
let _ = sql!("SELECT nope FROM users");
//      ^ error: unknown column `nope` in table `users`
```

## `migrate!` — embed a migrations directory

`migrate!("path")` reads every `.sql` file in a directory (relative to
`$CARGO_MANIFEST_DIR`) at compile time, orders them by numeric filename prefix,
and expands to a `&[Migration]`. Duplicate version prefixes are a compile error.

```text
migrations/
├── 001_create_posts.sql
└── 002_seed_posts.sql
```

```rust
use squrust_macros::migrate;

conn.migrate(migrate!("migrations")).await?;
```

The filename's leading digits become `Migration::version`; the rest becomes the
description.

See also: [[Async API]] (where `FromRow`/`ToParams`/`migrate` are used),
[[SQL Engine]] (the SQL the macros validate against).
