//! # squrust-core
//!
//! The storage engine underpinning Squrust: fixed-size pages, an LRU page
//! cache, a write-ahead log, MVCC snapshot isolation, and a table b-tree, tied
//! together by [`StorageEngine`] with read and write transactions.
//!
//! This crate uses no `unsafe`: file I/O is done with positioned
//! reads/writes rather than `mmap`.

#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod btree;
pub mod error;
pub mod mvcc;
pub mod page;
pub mod storage;
pub mod varint;
pub mod wal;

pub use btree::{BTree, BTreeCursor, PageSink, PageSource};
pub use error::{Result, StorageError};
pub use mvcc::{MvccManager, Snapshot, TransactionId};
pub use page::cache::PageCache;
pub use page::{PAGE_SIZE, PageId, PageType, RawPage};
pub use storage::{ReadTx, StorageEngine, WriteTx};
