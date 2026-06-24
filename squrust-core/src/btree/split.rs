//! Splitting table b-tree nodes, SQLite-style. Splits are chosen by cumulative
//! cell size so both halves fit.

use super::node::{InteriorNode, LeafCell, LeafNode};
use crate::varint;

fn leaf_cell_len(c: &LeafCell) -> usize {
    varint::len(c.payload_len) + varint::len(c.rowid as u64) + c.inline.len()
        + if c.overflow != 0 { 4 } else { 0 }
}

/// Split a full leaf. Returns `(left, right, separator)` where `separator` is
/// the largest rowid that remains in `left`.
pub fn split_leaf(mut leaf: LeafNode) -> (LeafNode, LeafNode, i64) {
    let total: usize = leaf.cells.iter().map(leaf_cell_len).sum();
    let mut acc = 0usize;
    let mut idx = 0usize;
    for (i, c) in leaf.cells.iter().enumerate() {
        acc += leaf_cell_len(c);
        if acc * 2 >= total {
            idx = i + 1;
            break;
        }
    }
    idx = idx.clamp(1, leaf.cells.len() - 1);
    let right_cells = leaf.cells.split_off(idx);
    let sep = leaf.cells.last().unwrap().rowid;
    (leaf, LeafNode { cells: right_cells }, sep)
}

/// Split a full interior node. The middle cell's key is promoted as the
/// separator and its child becomes the left node's right-child pointer.
pub fn split_interior(node: InteriorNode) -> (InteriorNode, InteriorNode, i64) {
    let n = node.cells.len();
    let m = (n / 2).clamp(0, n - 1);

    let left_cells = node.cells[..m].to_vec();
    let (mid_child, sep) = node.cells[m];
    let right_cells = node.cells[m + 1..].to_vec();

    let left = InteriorNode {
        cells: left_cells,
        right_child: mid_child,
    };
    let right = InteriorNode {
        cells: right_cells,
        right_child: node.right_child,
    };
    (left, right, sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(rowid: i64) -> LeafCell {
        LeafCell {
            rowid,
            payload_len: 4,
            overflow: 0,
            inline: vec![0; 4],
        }
    }

    #[test]
    fn leaf_split_balances() {
        let leaf = LeafNode {
            cells: (0..10).map(cell).collect(),
        };
        let (l, r, sep) = split_leaf(leaf);
        assert!(!l.cells.is_empty() && !r.cells.is_empty());
        assert_eq!(sep, l.cells.last().unwrap().rowid);
        assert!(r.cells[0].rowid > sep);
    }

    #[test]
    fn interior_split_promotes_middle() {
        let node = InteriorNode {
            cells: vec![(10, 10), (20, 20), (30, 30), (40, 40)],
            right_child: 50,
        };
        let (l, r, sep) = split_interior(node);
        assert_eq!(sep, 30);
        assert_eq!(l.right_child, 30);
        assert_eq!(l.cells, vec![(10, 10), (20, 20)]);
        assert_eq!(r.cells, vec![(40, 40)]);
        assert_eq!(r.right_child, 50);
    }
}
