//! # squrust (C ABI)
//!
//! A drop-in `libsqlite3` replacement backed by Squrust. Builds to both a
//! `cdylib` (`libsqurust.so`, usable via `LD_PRELOAD`) and a `staticlib`.
//!
//! All `unsafe` in the workspace that is not the storage mmap lives here, at
//! the C boundary. Each connection owns a private current-thread Tokio runtime,
//! so C callers need no knowledge of async or Tokio.

#![allow(non_camel_case_types)]
// The C ABI surface is inherently unsafe; safety is documented per function.
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod constants;
mod mem;
mod state;
mod types;

pub mod bind;
pub mod column;
pub mod errmsg;
pub mod exec;
pub mod meta;
pub mod open;
pub mod prepare;
pub mod step;
pub mod stubs;

pub use constants::*;
pub use types::{sqlite3, sqlite3_stmt};
