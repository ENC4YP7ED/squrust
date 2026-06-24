//! WAL frame format: a 32-byte header followed by a full page of data.
//!
//! Layout (big-endian):
//! ```text
//!   0  u32  page number
//!   4  u64  commit version
//!  12  u32  db size in pages after commit (non-zero only on the commit frame)
//!  16  u32  salt
//!  20  u64  checksum over header[0..20] ++ page data
//!  28  u32  reserved (zero)
//!  32 ..    page data (PAGE_SIZE bytes)
//! ```

use crate::error::{Result, StorageError};
use crate::page::{PAGE_SIZE, PageId};

pub const FRAME_HEADER_SIZE: usize = 32;
pub const FRAME_SIZE: usize = FRAME_HEADER_SIZE + PAGE_SIZE;

#[derive(Debug, Clone)]
pub struct WalFrame {
    pub page_id: PageId,
    pub commit_version: u64,
    /// Database size in pages after this commit; `0` for non-commit frames.
    pub db_size_after: u32,
    pub salt: u32,
    pub data: Box<[u8; PAGE_SIZE]>,
}

impl WalFrame {
    pub fn is_commit(&self) -> bool {
        self.db_size_after != 0
    }

    /// A deterministic checksum over the header prefix and the page body.
    fn checksum(header_prefix: &[u8], data: &[u8]) -> u64 {
        // FNV-1a over the meaningful header bytes and the page data.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in header_prefix.iter().chain(data.iter()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = vec![0u8; FRAME_SIZE];
        buf[0..4].copy_from_slice(&self.page_id.to_be_bytes());
        buf[4..12].copy_from_slice(&self.commit_version.to_be_bytes());
        buf[12..16].copy_from_slice(&self.db_size_after.to_be_bytes());
        buf[16..20].copy_from_slice(&self.salt.to_be_bytes());
        // checksum (bytes 20..28) computed below
        // 28..32 reserved zero
        buf[FRAME_HEADER_SIZE..].copy_from_slice(&self.data[..]);
        let ck = Self::checksum(&buf[0..20], &self.data[..]);
        buf[20..28].copy_from_slice(&ck.to_be_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Result<WalFrame> {
        if buf.len() < FRAME_SIZE {
            return Err(StorageError::CorruptWal(format!(
                "short frame: {} bytes",
                buf.len()
            )));
        }
        let page_id = u32::from_be_bytes(buf[0..4].try_into().unwrap());
        let commit_version = u64::from_be_bytes(buf[4..12].try_into().unwrap());
        let db_size_after = u32::from_be_bytes(buf[12..16].try_into().unwrap());
        let salt = u32::from_be_bytes(buf[16..20].try_into().unwrap());
        let stored_ck = u64::from_be_bytes(buf[20..28].try_into().unwrap());
        let mut data = Box::new([0u8; PAGE_SIZE]);
        data.copy_from_slice(&buf[FRAME_HEADER_SIZE..FRAME_SIZE]);
        let ck = Self::checksum(&buf[0..20], &data[..]);
        if ck != stored_ck {
            return Err(StorageError::CorruptWal("checksum mismatch".into()));
        }
        Ok(WalFrame {
            page_id,
            commit_version,
            db_size_after,
            salt,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let mut data = Box::new([0u8; PAGE_SIZE]);
        data[0] = 1;
        data[100] = 99;
        let frame = WalFrame {
            page_id: 12,
            commit_version: 7,
            db_size_after: 20,
            salt: 0xDEADBEEF,
            data,
        };
        let encoded = frame.encode();
        assert_eq!(encoded.len(), FRAME_SIZE);
        let back = WalFrame::decode(&encoded).unwrap();
        assert_eq!(back.page_id, 12);
        assert_eq!(back.commit_version, 7);
        assert_eq!(back.db_size_after, 20);
        assert!(back.is_commit());
        assert_eq!(back.data[100], 99);
    }

    #[test]
    fn detects_corruption() {
        let frame = WalFrame {
            page_id: 1,
            commit_version: 1,
            db_size_after: 1,
            salt: 0,
            data: Box::new([0u8; PAGE_SIZE]),
        };
        let mut encoded = frame.encode();
        encoded[FRAME_HEADER_SIZE + 10] ^= 0xFF; // flip a data bit
        assert!(matches!(
            WalFrame::decode(&encoded),
            Err(StorageError::CorruptWal(_))
        ));
    }
}
