//! Forward cursor for in-order traversal of a table b-tree.

use super::PageSource;
use super::node::{InteriorNode, LeafNode, Node};
use super::read_value;
use crate::error::Result;
use crate::page::PageId;

/// A cursor yielding `(rowid, record)` pairs in ascending rowid order.
pub struct BTreeCursor<'a, S: PageSource + ?Sized> {
    src: &'a S,
    /// Interior nodes from the root, each with the child index we descended.
    /// Index `cells.len()` denotes the right-child pointer.
    stack: Vec<(InteriorNode, usize)>,
    leaf: Option<(LeafNode, usize)>,
}

fn child_at(node: &InteriorNode, idx: usize) -> PageId {
    if idx < node.cells.len() {
        node.cells[idx].0
    } else {
        node.right_child
    }
}

impl<'a, S: PageSource + ?Sized> BTreeCursor<'a, S> {
    pub fn seek(src: &'a S, root: PageId, rowid: i64) -> Result<Self> {
        let mut cursor = BTreeCursor {
            src,
            stack: Vec::new(),
            leaf: None,
        };
        let mut cur = root;
        loop {
            match Node::parse(&*src.get_page(cur)?)? {
                Node::Interior(n) => {
                    let idx = n
                        .cells
                        .iter()
                        .position(|(_, k)| rowid <= *k)
                        .unwrap_or(n.cells.len());
                    let next = child_at(&n, idx);
                    cursor.stack.push((n, idx));
                    cur = next;
                }
                Node::Leaf(l) => {
                    let i = match l.search(rowid) {
                        Ok(i) => i,
                        Err(i) => i,
                    };
                    cursor.leaf = Some((l, i));
                    break;
                }
            }
        }
        Ok(cursor)
    }

    pub fn seek_first(src: &'a S, root: PageId) -> Result<Self> {
        Self::seek(src, root, i64::MIN)
    }

    fn descend_leftmost(&mut self, mut cur: PageId) -> Result<()> {
        loop {
            match Node::parse(&*self.src.get_page(cur)?)? {
                Node::Interior(n) => {
                    let next = child_at(&n, 0);
                    self.stack.push((n, 0));
                    cur = next;
                }
                Node::Leaf(l) => {
                    self.leaf = Some((l, 0));
                    return Ok(());
                }
            }
        }
    }

    /// Yield the next `(rowid, record)`, or `None` at the end.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<(i64, Vec<u8>)>> {
        loop {
            if let Some((leaf, i)) = self.leaf.as_ref() {
                if *i < leaf.cells.len() {
                    let cell = &leaf.cells[*i];
                    let rowid = cell.rowid;
                    let value = read_value(self.src, cell)?;
                    if let Some((_, idx)) = self.leaf.as_mut() {
                        *idx += 1;
                    }
                    return Ok(Some((rowid, value)));
                }
            }
            self.leaf = None;
            let next_child = loop {
                let len = self.stack.len();
                if len == 0 {
                    return Ok(None);
                }
                let (node, idx) = &mut self.stack[len - 1];
                *idx += 1;
                // Children are indices 0..=cells.len() (last == right_child).
                if *idx <= node.cells.len() {
                    break child_at(node, *idx);
                }
                self.stack.pop();
            };
            self.descend_leftmost(next_child)?;
        }
    }
}
