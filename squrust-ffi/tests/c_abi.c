/* Exercises the Squrust C ABI exactly as a libsqlite3 client would. */
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;

#define SQLITE_OK 0
#define SQLITE_ROW 100
#define SQLITE_DONE 101

extern int sqlite3_open(const char *, sqlite3 **);
extern int sqlite3_close(sqlite3 *);
extern int sqlite3_exec(sqlite3 *, const char *, int (*)(void *, int, char **, char **), void *, char **);
extern int sqlite3_prepare_v2(sqlite3 *, const char *, int, sqlite3_stmt **, const char **);
extern int sqlite3_step(sqlite3_stmt *);
extern int sqlite3_finalize(sqlite3_stmt *);
extern int sqlite3_bind_int64(sqlite3_stmt *, int, long long);
extern int sqlite3_bind_text(sqlite3_stmt *, int, const char *, int, void *);
extern int sqlite3_column_count(sqlite3_stmt *);
extern long long sqlite3_column_int64(sqlite3_stmt *, int);
extern const unsigned char *sqlite3_column_text(sqlite3_stmt *, int);
extern const char *sqlite3_column_name(sqlite3_stmt *, int);
extern const char *sqlite3_libversion(void);
extern const char *sqlite3_errmsg(sqlite3 *);
extern long long sqlite3_last_insert_rowid(sqlite3 *);

static int failures = 0;
#define CHECK(cond, msg) do { if (!(cond)) { printf("FAIL: %s\n", msg); failures++; } } while (0)

static int exec_rows = 0;
static int count_cb(void *arg, int ncol, char **vals, char **names) {
    (void)arg; (void)ncol; (void)names; (void)vals;
    exec_rows++;
    return 0;
}

int main(void) {
    printf("squrust libversion: %s\n", sqlite3_libversion());

    sqlite3 *db = NULL;
    CHECK(sqlite3_open(":memory:", &db) == SQLITE_OK, "open :memory:");
    CHECK(db != NULL, "db handle not null");

    char *err = NULL;
    int rc = sqlite3_exec(db,
        "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, score INTEGER)",
        NULL, NULL, &err);
    CHECK(rc == SQLITE_OK, "create table");

    /* Prepared INSERT with bound params. */
    sqlite3_stmt *ins = NULL;
    rc = sqlite3_prepare_v2(db, "INSERT INTO t(name, score) VALUES (?, ?)", -1, &ins, NULL);
    CHECK(rc == SQLITE_OK, "prepare insert");
    sqlite3_bind_text(ins, 1, "alice", -1, NULL);
    sqlite3_bind_int64(ins, 2, 90);
    CHECK(sqlite3_step(ins) == SQLITE_DONE, "step insert 1");
    sqlite3_finalize(ins);

    /* Plain exec inserts. */
    rc = sqlite3_exec(db, "INSERT INTO t(name, score) VALUES ('bob', 75), ('carol', 88)",
                      NULL, NULL, &err);
    CHECK(rc == SQLITE_OK, "exec inserts");

    long long last = sqlite3_last_insert_rowid(db);
    CHECK(last == 3, "last_insert_rowid == 3");

    /* Prepared SELECT, iterate rows. */
    sqlite3_stmt *sel = NULL;
    rc = sqlite3_prepare_v2(db, "SELECT name, score FROM t WHERE score >= ? ORDER BY score DESC",
                            -1, &sel, NULL);
    CHECK(rc == SQLITE_OK, "prepare select");
    sqlite3_bind_int64(sel, 1, 80);
    CHECK(sqlite3_column_count(sel) == 2, "column count 2");

    int n = 0;
    long long prev = 1000000;
    while (sqlite3_step(sel) == SQLITE_ROW) {
        const unsigned char *name = sqlite3_column_text(sel, 0);
        long long score = sqlite3_column_int64(sel, 1);
        printf("  row: %s = %lld\n", name ? (const char *)name : "(null)", score);
        CHECK(score <= prev, "rows ordered DESC");
        prev = score;
        n++;
    }
    CHECK(n == 2, "two rows with score >= 80");

    const char *cn = sqlite3_column_name(sel, 0);
    CHECK(cn && strcmp(cn, "name") == 0, "column name 0 == name");
    sqlite3_finalize(sel);

    /* exec callback row counting. */
    exec_rows = 0;
    rc = sqlite3_exec(db, "SELECT * FROM t", count_cb, NULL, &err);
    CHECK(rc == SQLITE_OK, "exec select with callback");
    CHECK(exec_rows == 3, "callback saw 3 rows");

    sqlite3_close(db);

    if (failures == 0) {
        printf("ALL C-ABI CHECKS PASSED\n");
        return 0;
    }
    printf("%d FAILURE(S)\n", failures);
    return 1;
}
