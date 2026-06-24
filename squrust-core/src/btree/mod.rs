//! A SQLite-compatible table b-tree: maps `i64` rowids to record byte payloads.
//!
//! Pages use the real SQLite on-disk format (see [`node`]). Large records spill
//! into overflow page chains. The root page id is kept stable across root
//! splits, which is what lets the catalog live permanently on page 1.

pub mod cursor;
pub mod node;
pub mod split;

use std::sync::Arc;

use crate::error::{Result, StorageError};
use crate::page::{PageId, RawPage};

pub use cursor::BTreeCursor;
use node::{InteriorNode, LeafCell, Node, USABLE, inline_len_for};

/// Payload bytes carried by each overflow page (4-byte next-page pointer + data).
const OVERFLOW_CAP: usize = USABLE - 4;

/// Read access to pages.
pub trait PageSource {
    fn get_page(&self, id: PageId) -> Result<Arc<RawPage>>;
}

/// Read + write access to pages.
pub trait PageSink: PageSource {
    fn alloc_page(&self) -> Result<PageId>;
    fn put_page(&self, page: RawPage) -> Result<()>;
    fn free_page(&self, id: PageId) -> Result<()>;
}

struct Split {
    sep_key: i64,
    right_page: PageId,
}

/// A handle to a table b-tree rooted at a fixed page.
pub struct BTree {
    pub root: PageId,
}

impl BTree {
    pub fn open(root: PageId) -> Self {
        BTree { root }
    }

    /// Allocate a fresh empty table b-tree, returning its root page id.
    pub fn create<S: PageSink + ?Sized>(sink: &S) -> Result<PageId> {
        let root = sink.alloc_page()?;
        let mut page = RawPage::new(root);
        Node::empty_leaf().serialize_into(&mut page);
        sink.put_page(page)?;
        Ok(root)
    }

    // --- reads ---

    pub fn get<S: PageSource + ?Sized>(&self, src: &S, rowid: i64) -> Result<Option<Vec<u8>>> {
        let mut cur = self.root;
        loop {
            match Node::parse(&*src.get_page(cur)?)? {
                Node::Interior(n) => cur = n.child_for(rowid),
                Node::Leaf(l) => {
                    return match l.search(rowid) {
                        Ok(i) => Ok(Some(read_value(src, &l.cells[i])?)),
                        Err(_) => Ok(None),
                    };
                }
            }
        }
    }

    pub fn cursor<'a, S: PageSource + ?Sized>(&self, src: &'a S) -> Result<BTreeCursor<'a, S>> {
        BTreeCursor::seek_first(src, self.root)
    }

    pub fn cursor_from<'a, S: PageSource + ?Sized>(
        &self,
        src: &'a S,
        rowid: i64,
    ) -> Result<BTreeCursor<'a, S>> {
        BTreeCursor::seek(src, self.root, rowid)
    }

    /// The largest rowid in the tree (rightmost descent), or `None` if empty.
    pub fn last_key<S: PageSource + ?Sized>(&self, src: &S) -> Result<Option<i64>> {
        let mut cur = self.root;
        loop {
            match Node::parse(&*src.get_page(cur)?)? {
                Node::Interior(n) => cur = n.right_child,
                Node::Leaf(l) => return Ok(l.cells.last().map(|c| c.rowid)),
            }
        }
    }

    // --- writes ---

    pub fn insert<S: PageSink + ?Sized>(&self, sink: &S, rowid: i64, record: &[u8]) -> Result<()> {
        let cell = self.make_cell(sink, rowid, record)?;
        if let Some(split) = self.insert_rec(sink, self.root, cell)? {
            self.grow_root(sink, split)?;
        }
        Ok(())
    }

    fn make_cell<S: PageSink + ?Sized>(
        &self,
        sink: &S,
        rowid: i64,
        record: &[u8],
    ) -> Result<LeafCell> {
        let payload_len = record.len() as u64;
        let x = USABLE - 35;
        if record.len() <= x {
            Ok(LeafCell {
                rowid,
                payload_len,
                overflow: 0,
                inline: record.to_vec(),
            })
        } else {
            let inline_len = inline_len_for(payload_len);
            let inline = record[..inline_len].to_vec();
            let overflow = write_overflow(sink, &record[inline_len..])?;
            Ok(LeafCell {
                rowid,
                payload_len,
                overflow,
                inline,
            })
        }
    }

    fn insert_rec<S: PageSink + ?Sized>(
        &self,
        sink: &S,
        page_id: PageId,
        cell: LeafCell,
    ) -> Result<Option<Split>> {
        match Node::parse(&*sink.get_page(page_id)?)? {
            Node::Leaf(mut leaf) => {
                let mut freed = 0;
                match leaf.search(cell.rowid) {
                    Ok(i) => {
                        freed = leaf.cells[i].overflow;
                        leaf.cells[i] = cell;
                    }
                    Err(i) => leaf.cells.insert(i, cell),
                }
                let result = if leaf.fits(page_id) {
                    self.write_node(sink, page_id, &Node::Leaf(leaf))?;
                    None
                } else {
                    let (left, right, sep) = split::split_leaf(leaf);
                    let right_page = sink.alloc_page()?;
                    self.write_node(sink, page_id, &Node::Leaf(left))?;
                    self.write_node(sink, right_page, &Node::Leaf(right))?;
                    Some(Split {
                        sep_key: sep,
                        right_page,
                    })
                };
                if freed != 0 {
                    free_overflow_chain(sink, freed)?;
                }
                Ok(result)
            }
            Node::Interior(mut node) => {
                let pos = node.cells.iter().position(|(_, k)| cell.rowid <= *k);
                let child = match pos {
                    Some(i) => node.cells[i].0,
                    None => node.right_child,
                };
                match self.insert_rec(sink, child, cell)? {
                    None => Ok(None),
                    Some(split) => {
                        match pos {
                            Some(i) => {
                                let old_key = node.cells[i].1;
                                node.cells[i] = (child, split.sep_key);
                                node.cells.insert(i + 1, (split.right_page, old_key));
                            }
                            None => {
                                node.cells.push((child, split.sep_key));
                                node.right_child = split.right_page;
                            }
                        }
                        if node.fits(page_id) {
                            self.write_node(sink, page_id, &Node::Interior(node))?;
                            Ok(None)
                        } else {
                            let (left, right, sep) = split::split_interior(node);
                            let right_page = sink.alloc_page()?;
                            self.write_node(sink, page_id, &Node::Interior(left))?;
                            self.write_node(sink, right_page, &Node::Interior(right))?;
                            Ok(Some(Split {
                                sep_key: sep,
                                right_page,
                            }))
                        }
                    }
                }
            }
        }
    }

    fn grow_root<S: PageSink + ?Sized>(&self, sink: &S, split: Split) -> Result<()> {
        let left_node = Node::parse(&*sink.get_page(self.root)?)?;
        let left_page = sink.alloc_page()?;
        self.write_node(sink, left_page, &left_node)?;
        let new_root = InteriorNode {
            cells: vec![(left_page, split.sep_key)],
            right_child: split.right_page,
        };
        self.write_node(sink, self.root, &Node::Interior(new_root))
    }

    pub fn delete<S: PageSink + ?Sized>(&self, sink: &S, rowid: i64) -> Result<bool> {
        self.delete_rec(sink, self.root, rowid)
    }

    fn delete_rec<S: PageSink + ?Sized>(
        &self,
        sink: &S,
        page_id: PageId,
        rowid: i64,
    ) -> Result<bool> {
        match Node::parse(&*sink.get_page(page_id)?)? {
            Node::Interior(n) => self.delete_rec(sink, n.child_for(rowid), rowid),
            Node::Leaf(mut leaf) => match leaf.search(rowid) {
                Ok(i) => {
                    let removed = leaf.cells.remove(i);
                    self.write_node(sink, page_id, &Node::Leaf(leaf))?;
                    if removed.overflow != 0 {
                        free_overflow_chain(sink, removed.overflow)?;
                    }
                    Ok(true)
                }
                Err(_) => Ok(false),
            },
        }
    }

    fn write_node<S: PageSink + ?Sized>(
        &self,
        sink: &S,
        page_id: PageId,
        node: &Node,
    ) -> Result<()> {
        let mut page = RawPage::new(page_id);
        node.serialize_into(&mut page);
        sink.put_page(page)
    }
}

/// Write the spilled tail of a record into a chain of overflow pages.
fn write_overflow<S: PageSink + ?Sized>(sink: &S, tail: &[u8]) -> Result<PageId> {
    let chunks: Vec<&[u8]> = tail.chunks(OVERFLOW_CAP).collect();
    let mut ids = Vec::with_capacity(chunks.len());
    for _ in &chunks {
        ids.push(sink.alloc_page()?);
    }
    for (i, chunk) in chunks.iter().enumerate() {
        let next = ids.get(i + 1).copied().unwrap_or(0);
        let mut page = RawPage::new(ids[i]);
        page.data[0..4].copy_from_slice(&next.to_be_bytes());
        page.data[4..4 + chunk.len()].copy_from_slice(chunk);
        page.dirty = true;
        sink.put_page(page)?;
    }
    Ok(ids[0])
}

fn read_value<S: PageSource + ?Sized>(src: &S, cell: &LeafCell) -> Result<Vec<u8>> {
    if cell.overflow == 0 {
        return Ok(cell.inline.clone());
    }
    let mut out = Vec::with_capacity(cell.payload_len as usize);
    out.extend_from_slice(&cell.inline);
    let mut next = cell.overflow;
    while next != 0 {
        let page = src.get_page(next)?;
        let np = u32::from_be_bytes(page.data[0..4].try_into().unwrap());
        let remaining = cell.payload_len as usize - out.len();
        let take = remaining.min(OVERFLOW_CAP);
        out.extend_from_slice(&page.data[4..4 + take]);
        next = np;
    }
    if out.len() != cell.payload_len as usize {
        return Err(StorageError::Corrupt(format!(
            "overflow length mismatch: {} vs {}",
            out.len(),
            cell.payload_len
        )));
    }
    Ok(out)
}

fn free_overflow_chain<S: PageSink + ?Sized>(sink: &S, first: PageId) -> Result<()> {
    let mut next = first;
    while next != 0 {
        let page = sink.get_page(next)?;
        let np = u32::from_be_bytes(page.data[0..4].try_into().unwrap());
        sink.free_page(next)?;
        next = np;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemStore {
        pages: Mutex<HashMap<PageId, Arc<RawPage>>>,
        next: Mutex<PageId>,
        free: Mutex<Vec<PageId>>,
    }

    impl MemStore {
        fn new() -> Self {
            MemStore {
                pages: Mutex::new(HashMap::new()),
                next: Mutex::new(2),
                free: Mutex::new(Vec::new()),
            }
        }
    }

    impl PageSource for MemStore {
        fn get_page(&self, id: PageId) -> Result<Arc<RawPage>> {
            self.pages
                .lock()
                .get(&id)
                .cloned()
                .ok_or(StorageError::PageOutOfRange(id))
        }
    }

    impl PageSink for MemStore {
        fn alloc_page(&self) -> Result<PageId> {
            if let Some(id) = self.free.lock().pop() {
                return Ok(id);
            }
            let mut next = self.next.lock();
            let id = *next;
            *next += 1;
            Ok(id)
        }
        fn put_page(&self, page: RawPage) -> Result<()> {
            self.pages.lock().insert(page.id, Arc::new(page));
            Ok(())
        }
        fn free_page(&self, id: PageId) -> Result<()> {
            self.pages.lock().remove(&id);
            self.free.lock().push(id);
            Ok(())
        }
    }

    fn collect(tree: &BTree, store: &MemStore) -> Vec<(i64, Vec<u8>)> {
        let mut cursor = tree.cursor(store).unwrap();
        let mut out = Vec::new();
        while let Some(kv) = cursor.next().unwrap() {
            out.push(kv);
        }
        out
    }

    #[test]
    fn insert_get_delete() {
        let store = MemStore::new();
        let root = BTree::create(&store).unwrap();
        let tree = BTree::open(root);
        tree.insert(&store, 5, b"five").unwrap();
        tree.insert(&store, 1, b"one").unwrap();
        tree.insert(&store, 3, b"three").unwrap();
        assert_eq!(tree.get(&store, 1).unwrap().unwrap(), b"one");
        assert_eq!(tree.get(&store, 99).unwrap(), None);
        let all = collect(&tree, &store);
        assert_eq!(all.iter().map(|(k, _)| *k).collect::<Vec<_>>(), vec![1, 3, 5]);
        tree.insert(&store, 3, b"THREE").unwrap();
        assert_eq!(tree.get(&store, 3).unwrap().unwrap(), b"THREE");
        assert!(tree.delete(&store, 3).unwrap());
        assert_eq!(tree.get(&store, 3).unwrap(), None);
    }

    #[test]
    fn many_inserts_force_splits() {
        let store = MemStore::new();
        let root = BTree::create(&store).unwrap();
        let tree = BTree::open(root);
        let n = 5000i64;
        for i in 0..n {
            let k = (i * 2_654_435_761i64).rem_euclid(n);
            tree.insert(&store, k, format!("val-{k}").as_bytes()).unwrap();
        }
        let all = collect(&tree, &store);
        assert!(all.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(!all.is_empty());
        assert_eq!(tree.last_key(&store).unwrap(), all.last().map(|(k, _)| *k));
    }

    #[test]
    fn sequential_insert_and_scan() {
        let store = MemStore::new();
        let root = BTree::create(&store).unwrap();
        let tree = BTree::open(root);
        for i in 0..2000 {
            tree.insert(&store, i, format!("row{i}").as_bytes()).unwrap();
        }
        let all = collect(&tree, &store);
        assert_eq!(all.len(), 2000);
        for (i, (k, v)) in all.iter().enumerate() {
            assert_eq!(*k, i as i64);
            assert_eq!(v, format!("row{i}").as_bytes());
        }
    }

    #[test]
    fn large_value_overflow() {
        let store = MemStore::new();
        let root = BTree::create(&store).unwrap();
        let tree = BTree::open(root);
        let big = vec![0xABu8; 50_000];
        tree.insert(&store, 1, &big).unwrap();
        tree.insert(&store, 2, b"small").unwrap();
        assert_eq!(tree.get(&store, 1).unwrap().unwrap(), big);
        assert_eq!(tree.get(&store, 2).unwrap().unwrap(), b"small");
        tree.insert(&store, 1, b"tiny").unwrap();
        assert_eq!(tree.get(&store, 1).unwrap().unwrap(), b"tiny");
    }

    #[test]
    fn cursor_seek() {
        let store = MemStore::new();
        let root = BTree::create(&store).unwrap();
        let tree = BTree::open(root);
        for i in (0..100).step_by(10) {
            tree.insert(&store, i, b"x").unwrap();
        }
        let mut cursor = tree.cursor_from(&store, 25).unwrap();
        let (k, _) = cursor.next().unwrap().unwrap();
        assert_eq!(k, 30);
    }
}
