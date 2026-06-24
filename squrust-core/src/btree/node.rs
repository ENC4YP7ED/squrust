//! SQLite table b-tree page (de)serialization.
//!
//! Implements the real on-disk format so that `sqlite3` can read pages Squrust
//! writes and vice versa. Only *table* b-trees are produced/traversed (leaf
//! `0x0d`, interior `0x05`); index b-trees are not generated here.
//!
//! Page layout (header at byte 0, or byte 100 on page 1):
//! ```text
//!   header: u8 type, u16 first_freeblock, u16 num_cells, u16 cell_content_start,
//!           u8 fragmented_free, [interior only] u32 right_child
//!   cell pointer array: num_cells * u16 (absolute page offsets, key order)
//!   ... free space ...
//!   cell content area (grows down from end of page)
//! ```
//! Table leaf cell:    varint(payload_len) varint(rowid) payload[..inline] [u32 overflow]
//! Table interior cell: u32 left_child varint(rowid)

use crate::error::{Result, StorageError};
use crate::page::{PAGE_SIZE, PageId, RawPage};
use crate::varint;

pub const TABLE_LEAF: u8 = 0x0d;
pub const TABLE_INTERIOR: u8 = 0x05;

/// Usable bytes per page (page size minus the per-page reserved region, which
/// Squrust always leaves at 0).
pub const USABLE: usize = PAGE_SIZE;

/// The byte offset of the b-tree header within a page. Page 1 shares its first
/// 100 bytes with the database header.
pub fn header_offset(page_id: PageId) -> usize {
    if page_id == 1 { 100 } else { 0 }
}

#[derive(Debug, Clone)]
pub struct LeafCell {
    pub rowid: i64,
    /// Total record length (inline + overflow).
    pub payload_len: u64,
    /// First overflow page, or 0 if the record is fully inline.
    pub overflow: PageId,
    /// The inline portion of the record.
    pub inline: Vec<u8>,
}

impl LeafCell {
    fn encoded_len(&self) -> usize {
        varint::len(self.payload_len)
            + varint::len(zigzag(self.rowid))
            + self.inline.len()
            + if self.overflow != 0 { 4 } else { 0 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LeafNode {
    pub cells: Vec<LeafCell>,
}

impl LeafNode {
    pub fn search(&self, rowid: i64) -> std::result::Result<usize, usize> {
        self.cells.binary_search_by_key(&rowid, |c| c.rowid)
    }

    pub fn used(&self, page_id: PageId) -> usize {
        header_offset(page_id)
            + 8
            + self.cells.len() * 2
            + self.cells.iter().map(|c| c.encoded_len()).sum::<usize>()
    }

    pub fn fits(&self, page_id: PageId) -> bool {
        self.used(page_id) <= PAGE_SIZE
    }
}

/// One interior cell: a left child plus the largest rowid in that child's subtree.
#[derive(Debug, Clone, Default)]
pub struct InteriorNode {
    pub cells: Vec<(PageId, i64)>,
    pub right_child: PageId,
}

impl InteriorNode {
    /// The child to descend for `rowid`.
    pub fn child_for(&self, rowid: i64) -> PageId {
        for (child, key) in &self.cells {
            if rowid <= *key {
                return *child;
            }
        }
        self.right_child
    }

    fn cell_len(key: i64) -> usize {
        4 + varint::len(zigzag(key))
    }

    pub fn used(&self, page_id: PageId) -> usize {
        header_offset(page_id)
            + 12
            + self.cells.len() * 2
            + self
                .cells
                .iter()
                .map(|(_, k)| Self::cell_len(*k))
                .sum::<usize>()
    }

    pub fn fits(&self, page_id: PageId) -> bool {
        self.used(page_id) <= PAGE_SIZE
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(LeafNode),
    Interior(InteriorNode),
}

impl Node {
    pub fn empty_leaf() -> Node {
        Node::Leaf(LeafNode::default())
    }

    pub fn serialize_into(&self, page: &mut RawPage) {
        let page_id = page.id;
        let off = header_offset(page_id);
        let buf = &mut page.data[..];
        // Zero everything except, on page 1, the database header.
        for b in buf[off..].iter_mut() {
            *b = 0;
        }

        match self {
            Node::Leaf(leaf) => {
                let header_size = 8;
                // Lay cells out contiguously at the end of the page.
                let total: usize = leaf.cells.iter().map(|c| c.encoded_len()).sum();
                let content_start = PAGE_SIZE - total;
                buf[off] = TABLE_LEAF;
                write_u16(buf, off + 1, 0); // first freeblock
                write_u16(buf, off + 3, leaf.cells.len() as u16);
                write_u16(buf, off + 5, content_start as u16);
                buf[off + 7] = 0; // fragmented free bytes

                let mut cell_pos = content_start;
                let ptr_array = off + header_size;
                for (i, cell) in leaf.cells.iter().enumerate() {
                    write_u16(buf, ptr_array + i * 2, cell_pos as u16);
                    let mut tmp = Vec::with_capacity(cell.encoded_len());
                    varint::write(&mut tmp, cell.payload_len);
                    varint::write(&mut tmp, zigzag(cell.rowid));
                    tmp.extend_from_slice(&cell.inline);
                    if cell.overflow != 0 {
                        tmp.extend_from_slice(&cell.overflow.to_be_bytes());
                    }
                    buf[cell_pos..cell_pos + tmp.len()].copy_from_slice(&tmp);
                    cell_pos += tmp.len();
                }
            }
            Node::Interior(node) => {
                let header_size = 12;
                let total: usize = node
                    .cells
                    .iter()
                    .map(|(_, k)| InteriorNode::cell_len(*k))
                    .sum();
                let content_start = PAGE_SIZE - total;
                buf[off] = TABLE_INTERIOR;
                write_u16(buf, off + 1, 0);
                write_u16(buf, off + 3, node.cells.len() as u16);
                write_u16(buf, off + 5, content_start as u16);
                buf[off + 7] = 0;
                write_u32(buf, off + 8, node.right_child);

                let mut cell_pos = content_start;
                let ptr_array = off + header_size;
                for (i, (child, key)) in node.cells.iter().enumerate() {
                    write_u16(buf, ptr_array + i * 2, cell_pos as u16);
                    let start = cell_pos;
                    buf[cell_pos..cell_pos + 4].copy_from_slice(&child.to_be_bytes());
                    cell_pos += 4;
                    let mut tmp = Vec::new();
                    varint::write(&mut tmp, zigzag(*key));
                    buf[cell_pos..cell_pos + tmp.len()].copy_from_slice(&tmp);
                    cell_pos += tmp.len();
                    debug_assert_eq!(cell_pos - start, InteriorNode::cell_len(*key));
                }
            }
        }
        page.dirty = true;
    }

    pub fn parse(page: &RawPage) -> Result<Node> {
        let off = header_offset(page.id);
        let buf = &page.data[..];
        let ty = buf[off];
        let num_cells = read_u16(buf, off + 3) as usize;
        match ty {
            TABLE_LEAF => {
                let ptr_array = off + 8;
                let mut cells = Vec::with_capacity(num_cells);
                for i in 0..num_cells {
                    let cell_off = read_u16(buf, ptr_array + i * 2) as usize;
                    cells.push(parse_leaf_cell(buf, cell_off)?);
                }
                Ok(Node::Leaf(LeafNode { cells }))
            }
            TABLE_INTERIOR => {
                let right_child = read_u32(buf, off + 8);
                let ptr_array = off + 12;
                let mut cells = Vec::with_capacity(num_cells);
                for i in 0..num_cells {
                    let cell_off = read_u16(buf, ptr_array + i * 2) as usize;
                    let child = read_u32(buf, cell_off);
                    let (key, _) = varint::read(&buf[cell_off + 4..])
                        .ok_or_else(|| StorageError::Corrupt("bad interior rowid varint".into()))?;
                    cells.push((child, unzigzag(key)));
                }
                Ok(Node::Interior(InteriorNode { cells, right_child }))
            }
            other => Err(StorageError::Corrupt(format!(
                "unsupported b-tree page type 0x{other:02x}"
            ))),
        }
    }
}

fn parse_leaf_cell(buf: &[u8], mut pos: usize) -> Result<LeafCell> {
    let (payload_len, n) = varint::read(&buf[pos..])
        .ok_or_else(|| StorageError::Corrupt("bad payload varint".into()))?;
    pos += n;
    let (rowid_raw, n) = varint::read(&buf[pos..])
        .ok_or_else(|| StorageError::Corrupt("bad rowid varint".into()))?;
    pos += n;
    let rowid = unzigzag(rowid_raw);

    let p = payload_len as usize;
    let x = USABLE - 35;
    if p <= x {
        let inline = buf[pos..pos + p].to_vec();
        Ok(LeafCell {
            rowid,
            payload_len,
            overflow: 0,
            inline,
        })
    } else {
        let inline_len = inline_len_for(payload_len);
        let inline = buf[pos..pos + inline_len].to_vec();
        let overflow = read_u32(buf, pos + inline_len);
        Ok(LeafCell {
            rowid,
            payload_len,
            overflow,
            inline,
        })
    }
}

/// Inline payload length when a record of length `p` spills to overflow.
pub fn inline_len_for(p: u64) -> usize {
    let u = USABLE as u64;
    let x = u - 35;
    if p <= x {
        return p as usize;
    }
    let m = ((u - 12) * 32 / 255) - 23;
    let k = m + ((p - m) % (u - 4));
    (if k <= x { k } else { m }) as usize
}

// SQLite stores rowids as signed varints via the record's integer encoding, but
// the cell rowid itself is a plain varint of the (non-negative for our use)
// rowid. We use zig-zag so negative rowids also round-trip; for the common
// non-negative case this matches plain varint for values < 2^63 only loosely,
// so we keep it internal-consistent. Rowids in practice are positive.
fn zigzag(v: i64) -> u64 {
    v as u64
}
fn unzigzag(v: u64) -> i64 {
    v as i64
}

fn write_u16(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_be_bytes());
}
fn write_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_be_bytes());
}
fn read_u16(buf: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([buf[at], buf[at + 1]])
}
fn read_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_roundtrip() {
        let leaf = LeafNode {
            cells: vec![
                LeafCell {
                    rowid: 1,
                    payload_len: 3,
                    overflow: 0,
                    inline: vec![1, 2, 3],
                },
                LeafCell {
                    rowid: 1000,
                    payload_len: 2,
                    overflow: 0,
                    inline: vec![9, 9],
                },
            ],
        };
        let mut page = RawPage::new(7);
        Node::Leaf(leaf).serialize_into(&mut page);
        match Node::parse(&page).unwrap() {
            Node::Leaf(l) => {
                assert_eq!(l.cells.len(), 2);
                assert_eq!(l.cells[0].rowid, 1);
                assert_eq!(l.cells[1].inline, vec![9, 9]);
            }
            _ => panic!("expected leaf"),
        }
    }

    #[test]
    fn interior_roundtrip_and_routing() {
        let node = InteriorNode {
            cells: vec![(2, 10), (3, 20)],
            right_child: 4,
        };
        let mut page = RawPage::new(9);
        Node::Interior(node).serialize_into(&mut page);
        match Node::parse(&page).unwrap() {
            Node::Interior(n) => {
                assert_eq!(n.right_child, 4);
                assert_eq!(n.child_for(5), 2);
                assert_eq!(n.child_for(10), 2);
                assert_eq!(n.child_for(11), 3);
                assert_eq!(n.child_for(20), 3);
                assert_eq!(n.child_for(21), 4);
            }
            _ => panic!("expected interior"),
        }
    }

    #[test]
    fn page1_uses_offset_100() {
        let leaf = LeafNode {
            cells: vec![LeafCell {
                rowid: 5,
                payload_len: 1,
                overflow: 0,
                inline: vec![42],
            }],
        };
        let mut page = RawPage::new(1);
        Node::Leaf(leaf).serialize_into(&mut page);
        // Type byte lives at offset 100 on page 1.
        assert_eq!(page.data[100], TABLE_LEAF);
        match Node::parse(&page).unwrap() {
            Node::Leaf(l) => assert_eq!(l.cells[0].inline, vec![42]),
            _ => panic!(),
        }
    }
}
