//! On-disk format constants. The first 100 bytes of the database file follow
//! the SQLite 3 header layout so that tooling expecting that magic string can
//! at least recognise the file. See <https://www.sqlite.org/fileformat.html>.

/// Default page size in bytes. Matches the SQLite default.
pub const PAGE_SIZE: usize = 4096;

/// Length of the file header, in bytes (SQLite uses 100).
pub const HEADER_SIZE: usize = 100;

/// The 16-byte magic string at offset 0, including the trailing NUL.
pub const MAGIC: &[u8; 16] = b"SQLite format 3\0";

// Byte offsets within the 100-byte header.
pub const OFF_MAGIC: usize = 0; // 16 bytes
pub const OFF_PAGE_SIZE: usize = 16; // u16 big-endian (1 means 65536)
pub const OFF_WRITE_VERSION: usize = 18; // u8: 2 == WAL
pub const OFF_READ_VERSION: usize = 19; // u8: 2 == WAL
pub const OFF_RESERVED: usize = 20; // u8 reserved space per page
pub const OFF_FILE_CHANGE_COUNTER: usize = 24; // u32 be
pub const OFF_DB_SIZE_PAGES: usize = 28; // u32 be: size of database in pages
pub const OFF_FREELIST_TRUNK: usize = 32; // u32 be: first freelist trunk page
pub const OFF_FREELIST_COUNT: usize = 36; // u32 be: total freelist pages
pub const OFF_SCHEMA_COOKIE: usize = 40; // u32 be
pub const OFF_SCHEMA_FORMAT: usize = 44; // u32 be
pub const OFF_TEXT_ENCODING: usize = 56; // u32 be: 1 == UTF-8
pub const OFF_VERSION_VALID_FOR: usize = 92; // u32 be: change counter the db size is valid for
pub const OFF_SQLITE_VERSION: usize = 96; // u32 be: SQLITE_VERSION_NUMBER

/// Text encoding marker for UTF-8 (matches SQLite).
pub const TEXT_ENCODING_UTF8: u32 = 1;
