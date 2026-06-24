# CLI — `sq` (`squrust-cli`)

`sq` is a `sqlite3`-style command-line shell built on the [[Async API]].

## Install

```console
$ cargo install --git https://github.com/ENC4YP7ED/squrust squrust-cli
# or, from a checkout:
$ cargo build --release -p squrust-cli   # -> target/release/sq
```

## Usage

```
sq [DATABASE] [SQL] [--mode <table|csv|json|line>]
```

- `DATABASE` defaults to `:memory:`.
- If `SQL` is given, it runs and exits.
- If stdin is piped, SQL is read from it.
- Otherwise an interactive REPL starts.

### One-shot

```console
$ sq mydata.db "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT);
                INSERT INTO t(name) VALUES('alice'),('bob');
                SELECT * FROM t ORDER BY id"
┌────┬───────┐
│ id ┆ name  │
╞════╪═══════╡
│ 1  ┆ alice │
├╌╌╌╌┼╌╌╌╌╌╌╌┤
│ 2  ┆ bob   │
└────┴───────┘
```

A file-backed database is **checkpointed on exit**, so the resulting `.db` is a
complete, stock-`sqlite3`-readable file (see [[SQLite Compatibility]]).

### Output modes

```console
$ sq :memory: --mode json "SELECT 1 AS a, 'x' AS b"
[ { "a": 1, "b": "x" } ]

$ sq :memory: --mode csv  "SELECT 1 AS a, 'x' AS b"
a,b
1,x

$ sq :memory: --mode line "SELECT 1 AS a, 'x' AS b"
a = 1
b = x
```

### Piped stdin

```console
$ echo "SELECT 2 + 3 AS answer" | sq :memory:
```

### Interactive REPL

```console
$ sq mydata.db
squrust shell — enter SQL terminated by ';', or .help for commands
sq> SELECT count(*) FROM t;
...
sq> .tables
t
sq> .schema t
CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);
sq> .mode csv
sq> .import people.csv people
imported 100 rows into people
sq> .quit
```

Multi-line input is buffered until a `;`. History is kept in `~/.sq_history`.

### Dot-commands

| Command | Effect |
|---------|--------|
| `.tables` | List tables |
| `.schema [TABLE]` | Show `CREATE` statements |
| `.mode MODE` | Switch output mode (`table`/`csv`/`json`/`line`) |
| `.import FILE TABLE` | Bulk-insert a CSV file into a table |
| `.help` | List commands |
| `.exit` / `.quit` | Leave |

See also: [[Async API]], [[SQLite Compatibility]].
