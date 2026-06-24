//! # squrust-serde
//!
//! Traits for mapping query rows to typed Rust values ([`FromRow`]) and Rust
//! values to bind parameters ([`ToParams`]).

#![forbid(unsafe_code)]

pub mod from_row;
pub mod to_params;

pub use from_row::{FromRow, FromValue, RowAccess};
pub use to_params::ToParams;
