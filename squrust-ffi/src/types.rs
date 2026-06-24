//! Opaque C handle types.

/// Opaque database connection handle (`sqlite3 *`). Backed by a
/// [`crate::state::ConnectionState`].
#[repr(C)]
pub struct sqlite3 {
    _opaque: [u8; 0],
}

/// Opaque prepared-statement handle (`sqlite3_stmt *`). Backed by a
/// [`crate::state::StmtState`].
#[repr(C)]
pub struct sqlite3_stmt {
    _opaque: [u8; 0],
}
