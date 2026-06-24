/*
 * squrust.h — drop-in subset of sqlite3.h provided by libsqurust.
 *
 * Link against libsqurust.so / libsqurust.a, or LD_PRELOAD libsqurust.so to
 * substitute it for the system libsqlite3 for the supported API surface.
 */
#ifndef SQURUST_H
#define SQURUST_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;
typedef long long sqlite3_int64;

/* Result codes */
#define SQLITE_OK           0
#define SQLITE_ERROR        1
#define SQLITE_BUSY         5
#define SQLITE_CANTOPEN    14
#define SQLITE_MISUSE      21
#define SQLITE_RANGE       25
#define SQLITE_ROW        100
#define SQLITE_DONE       101

/* Fundamental datatypes */
#define SQLITE_INTEGER 1
#define SQLITE_FLOAT   2
#define SQLITE_TEXT    3
#define SQLITE_BLOB    4
#define SQLITE_NULL    5

/* bind/text destructor sentinels */
#define SQLITE_STATIC      ((void(*)(void*))0)
#define SQLITE_TRANSIENT   ((void(*)(void*))-1)

/* Connection lifecycle */
int sqlite3_open(const char *filename, sqlite3 **ppDb);
int sqlite3_open_v2(const char *filename, sqlite3 **ppDb, int flags, const char *zVfs);
int sqlite3_close(sqlite3 *db);
int sqlite3_close_v2(sqlite3 *db);

/* One-shot exec */
int sqlite3_exec(sqlite3 *db, const char *sql,
                 int (*callback)(void *, int, char **, char **),
                 void *arg, char **errmsg);

/* Prepared statements */
int sqlite3_prepare_v2(sqlite3 *db, const char *sql, int nByte,
                       sqlite3_stmt **ppStmt, const char **pzTail);
int sqlite3_finalize(sqlite3_stmt *stmt);
const char *sqlite3_sql(sqlite3_stmt *stmt);
int sqlite3_stmt_readonly(sqlite3_stmt *stmt);
int sqlite3_step(sqlite3_stmt *stmt);
int sqlite3_reset(sqlite3_stmt *stmt);
int sqlite3_clear_bindings(sqlite3_stmt *stmt);

/* Binding */
int sqlite3_bind_int(sqlite3_stmt *, int, int);
int sqlite3_bind_int64(sqlite3_stmt *, int, sqlite3_int64);
int sqlite3_bind_double(sqlite3_stmt *, int, double);
int sqlite3_bind_text(sqlite3_stmt *, int, const char *, int, void (*)(void *));
int sqlite3_bind_blob(sqlite3_stmt *, int, const void *, int, void (*)(void *));
int sqlite3_bind_null(sqlite3_stmt *, int);
int sqlite3_bind_zeroblob(sqlite3_stmt *, int, int);
int sqlite3_bind_parameter_count(sqlite3_stmt *);
const char *sqlite3_bind_parameter_name(sqlite3_stmt *, int);

/* Result columns */
int sqlite3_column_count(sqlite3_stmt *);
int sqlite3_data_count(sqlite3_stmt *);
int sqlite3_column_type(sqlite3_stmt *, int iCol);
const char *sqlite3_column_name(sqlite3_stmt *, int iCol);
int sqlite3_column_int(sqlite3_stmt *, int iCol);
sqlite3_int64 sqlite3_column_int64(sqlite3_stmt *, int iCol);
double sqlite3_column_double(sqlite3_stmt *, int iCol);
const unsigned char *sqlite3_column_text(sqlite3_stmt *, int iCol);
const void *sqlite3_column_blob(sqlite3_stmt *, int iCol);
int sqlite3_column_bytes(sqlite3_stmt *, int iCol);
const char *sqlite3_column_decltype(sqlite3_stmt *, int iCol);

/* Errors and memory */
int sqlite3_errcode(sqlite3 *db);
int sqlite3_extended_errcode(sqlite3 *db);
const char *sqlite3_errmsg(sqlite3 *db);
const char *sqlite3_errstr(int code);
void sqlite3_free(void *p);
void *sqlite3_malloc(int n);

/* Metadata */
const char *sqlite3_libversion(void);
int sqlite3_libversion_number(void);
const char *sqlite3_sourceid(void);
int sqlite3_threadsafe(void);
int sqlite3_changes(sqlite3 *db);
sqlite3_int64 sqlite3_changes64(sqlite3 *db);
sqlite3_int64 sqlite3_last_insert_rowid(sqlite3 *db);
void sqlite3_interrupt(sqlite3 *db);
int sqlite3_get_autocommit(sqlite3 *db);
int sqlite3_busy_timeout(sqlite3 *db, int ms);
int sqlite3_complete(const char *sql);
sqlite3 *sqlite3_db_handle(sqlite3_stmt *stmt);

#ifdef __cplusplus
}
#endif
#endif /* SQURUST_H */
