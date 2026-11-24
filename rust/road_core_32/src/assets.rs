//! Asset streaming: resource packs are opened by the 64-bit UI and streamed to
//! the companion in small, CRC-checked chunks carried by the command ring.
//!
//! Chunks are fixed size so the 32-bit side never allocates while receiving.

use crate::util::crc32;

/// Payload bytes per chunk. Fits in a 256-byte [`crate::ipc::layout::Command`]
/// together with its header.
pub const ASSET_CHUNK_DATA: usize = 192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetChunk {
    pub asset_id: u32,
    pub index: u32,
    pub total: u32,
    pub crc: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetError {
    EmptyChunk,
    IndexOutOfRange,
    ChecksumMismatch,
    InconsistentTotal,
    DuplicateChunk,
    TooLarge,
}

/// Maximum size of a single streamed asset (texture atlas / sound bank).
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;

/// Largest chunk count accepted by [`CMD_ASSET_BEGIN`]. Bounds the bookkeeping
/// vector a hostile or confused sender can make the companion allocate.
///
/// [`CMD_ASSET_BEGIN`]: crate::ipc::layout::CMD_ASSET_BEGIN
pub const MAX_ASSET_CHUNKS: u32 = 16 * 1024;

/// Published in the header so the 64-bit UI can poll the stream outcome.
pub const ASSET_STATUS_NONE: u32 = 0;
pub const ASSET_STATUS_PENDING: u32 = 1;
pub const ASSET_STATUS_OK: u32 = 2;
pub const ASSET_STATUS_EMPTY: u32 = 3;
pub const ASSET_STATUS_RANGE: u32 = 4;
pub const ASSET_STATUS_CHECKSUM: u32 = 5;
pub const ASSET_STATUS_TOTAL: u32 = 6;
pub const ASSET_STATUS_DUPLICATE: u32 = 7;
pub const ASSET_STATUS_TOO_LARGE: u32 = 8;
/// The CRC the UI sent at commit did not match the staged bytes.
pub const ASSET_STATUS_CRC_MISMATCH: u32 = 9;
/// The UI committed a different length than the chunks added up to.
pub const ASSET_STATUS_LENGTH_MISMATCH: u32 = 10;
/// A chunk or commit arrived without a `CMD_ASSET_BEGIN`.
pub const ASSET_STATUS_NO_BEGIN: u32 = 11;
/// The stream ended before every chunk arrived.
pub const ASSET_STATUS_INCOMPLETE: u32 = 12;
/// `CMD_ASSET_BEGIN` asked for an impossible chunk count.
pub const ASSET_STATUS_BAD_TOTAL: u32 = 13;

pub fn status_code(error: AssetError) -> u32 {
    match error {
        AssetError::EmptyChunk => ASSET_STATUS_EMPTY,
        AssetError::IndexOutOfRange => ASSET_STATUS_RANGE,
        AssetError::ChecksumMismatch => ASSET_STATUS_CHECKSUM,
        AssetError::InconsistentTotal => ASSET_STATUS_TOTAL,
        AssetError::DuplicateChunk => ASSET_STATUS_DUPLICATE,
        AssetError::TooLarge => ASSET_STATUS_TOO_LARGE,
    }
}

/// Splits `bytes` into chunks. Returns an empty vector for an empty asset.
pub fn chunk_asset(asset_id: u32, bytes: &[u8]) -> Vec<AssetChunk> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let total = bytes.len().div_ceil(ASSET_CHUNK_DATA) as u32;
    bytes
        .chunks(ASSET_CHUNK_DATA)
        .enumerate()
        .map(|(index, data)| AssetChunk {
            asset_id,
            index: index as u32,
            total,
            crc: crc32(data),
            data: data.to_vec(),
        })
        .collect()
}

/// Reassembles a streamed asset, validating every chunk.
#[derive(Debug, Default)]
pub struct AssetAssembler {
    asset_id: u32,
    total: u32,
    received: u32,
    bytes: usize,
    chunks: Vec<Option<Vec<u8>>>,
}

impl AssetAssembler {
    pub fn new(asset_id: u32) -> Self {
        Self { asset_id, ..Default::default() }
    }

    /// Opens an assembler for a known chunk count, so the bookkeeping vector is
    /// allocated once up front instead of on the first chunk.
    pub fn begin(asset_id: u32, total: u32) -> Self {
        Self {
            asset_id,
            total,
            chunks: if total == 0 { Vec::new() } else { vec![None; total as usize] },
            ..Default::default()
        }
    }

    pub fn push(&mut self, chunk: &AssetChunk) -> Result<Option<Vec<u8>>, AssetError> {
        if chunk.data.is_empty() {
            return Err(AssetError::EmptyChunk);
        }
        if chunk.asset_id != self.asset_id {
            return Err(AssetError::InconsistentTotal);
        }
        if chunk.total == 0 || chunk.index >= chunk.total {
            return Err(AssetError::IndexOutOfRange);
        }
        if self.total == 0 {
            self.total = chunk.total;
            self.chunks = vec![None; chunk.total as usize];
        } else if self.total != chunk.total {
            return Err(AssetError::InconsistentTotal);
        }
        if crc32(&chunk.data) != chunk.crc {
            return Err(AssetError::ChecksumMismatch);
        }
        let slot = &mut self.chunks[chunk.index as usize];
        if slot.is_some() {
            return Err(AssetError::DuplicateChunk);
        }
        if self.bytes + chunk.data.len() > MAX_ASSET_BYTES {
            return Err(AssetError::TooLarge);
        }
        self.bytes += chunk.data.len();
        *slot = Some(chunk.data.clone());
        self.received += 1;
        if self.received < self.total {
            return Ok(None);
        }
        let mut assembled = Vec::with_capacity(self.bytes);
        for chunk in &self.chunks {
            match chunk {
                Some(data) => assembled.extend_from_slice(data),
                None => return Ok(None),
            }
        }
        Ok(Some(assembled))
    }

    pub fn received(&self) -> u32 {
        self.received
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_round_trip() {
        let payload: Vec<u8> = (0..1000u32).map(|value| (value % 251) as u8).collect();
        let chunks = chunk_asset(7, &payload);
        assert_eq!(chunks.len(), 6);
        assert!(chunks.iter().all(|chunk| chunk.asset_id == 7));
        assert!(chunks.iter().all(|chunk| chunk.total == 6));
        let mut assembler = AssetAssembler::new(7);
        let mut assembled = None;
        for chunk in &chunks {
            assembled = assembler.push(chunk).expect("push");
        }
        assert_eq!(assembled.as_deref(), Some(payload.as_slice()));
        assert_eq!(assembler.received(), 6);
        assert_eq!(assembler.bytes(), 1000);
    }

    #[test]
    fn begin_preallocates_the_expected_chunk_count() {
        let payload: Vec<u8> = (0..400u32).map(|value| value as u8).collect();
        let chunks = chunk_asset(9, &payload);
        let mut assembler = AssetAssembler::begin(9, chunks.len() as u32);
        assert_eq!(assembler.total(), chunks.len() as u32);
        let mut result = None;
        for chunk in &chunks {
            result = assembler.push(chunk).expect("push");
        }
        assert_eq!(result.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn begin_with_the_wrong_chunk_count_is_rejected() {
        let chunks = chunk_asset(9, &[3u8; 400]);
        let mut assembler = AssetAssembler::begin(9, chunks.len() as u32 + 1);
        assert_eq!(assembler.push(&chunks[0]), Err(AssetError::InconsistentTotal));
    }

    #[test]
    fn empty_asset_produces_no_chunks() {
        assert!(chunk_asset(1, &[]).is_empty());
    }

    #[test]
    fn corrupted_chunk_is_rejected() {
        let mut chunks = chunk_asset(1, &[1u8; 500]);
        chunks[0].data[0] ^= 0xFF;
        let mut assembler = AssetAssembler::new(1);
        assert_eq!(assembler.push(&chunks[0]), Err(AssetError::ChecksumMismatch));
    }

    #[test]
    fn out_of_order_chunks_still_assemble() {
        let payload: Vec<u8> = (0..600u32).map(|value| value as u8).collect();
        let mut chunks = chunk_asset(3, &payload);
        chunks.reverse();
        let mut assembler = AssetAssembler::new(3);
        let mut result = None;
        for chunk in &chunks {
            result = assembler.push(chunk).expect("push");
        }
        assert_eq!(result.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn duplicate_and_out_of_range_chunks_are_rejected() {
        let chunks = chunk_asset(4, &[9u8; 300]);
        let mut assembler = AssetAssembler::new(4);
        assembler.push(&chunks[0]).expect("first");
        assert_eq!(assembler.push(&chunks[0]), Err(AssetError::DuplicateChunk));
        let mut bad = chunks[1].clone();
        bad.index = 99;
        assert_eq!(assembler.push(&bad), Err(AssetError::IndexOutOfRange));
    }

    #[test]
    fn mismatched_asset_id_is_rejected() {
        let chunks = chunk_asset(5, &[1u8; 10]);
        let mut assembler = AssetAssembler::new(6);
        assert_eq!(assembler.push(&chunks[0]), Err(AssetError::InconsistentTotal));
    }
}
