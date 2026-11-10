use crate::error::StoreError;
use oligopole_core::WorldState;
use oligopole_ledger::Money;

/// Computes the canonical 32-byte `blake3` hash of `WorldState` serialized via `postcard`.
///
/// Determinism is guaranteed because `std::collections::HashMap` and `HashSet` are forbidden
/// across the workspace via `clippy.toml` (`disallowed-types`).
pub fn compute_world_blake3_hash(world: &WorldState) -> Result<[u8; 32], StoreError> {
    let raw_bytes =
        postcard::to_allocvec(world).map_err(|e| StoreError::Postcard(e.to_string()))?;
    Ok(*blake3::hash(&raw_bytes).as_bytes())
}

/// Serializes `WorldState` via `postcard`, computes its 32-byte `blake3` digest,
/// and compresses the bytes using `zstd` (level 3).
pub fn compress_world_snapshot(world: &WorldState) -> Result<(Vec<u8>, [u8; 32]), StoreError> {
    let raw_bytes =
        postcard::to_allocvec(world).map_err(|e| StoreError::Postcard(e.to_string()))?;
    let hash = *blake3::hash(&raw_bytes).as_bytes();
    let compressed =
        zstd::encode_all(raw_bytes.as_slice(), 3).map_err(|e| StoreError::Zstd(e.to_string()))?;
    Ok((compressed, hash))
}

/// Decompresses a `zstd(postcard)` snapshot blob, verifies its `blake3` hash against `expected_hash`,
/// and deserializes `WorldState`.
pub fn decompress_world_snapshot(
    compressed: &[u8],
    expected_hash: &[u8; 32],
) -> Result<WorldState, StoreError> {
    let raw_bytes =
        zstd::decode_all(compressed).map_err(|e| StoreError::Zstd(e.to_string()))?;
    let actual_hash = *blake3::hash(&raw_bytes).as_bytes();
    if &actual_hash != expected_hash {
        return Err(StoreError::ReplayHashMismatch {
            tick: 0,
            expected_hex: to_hex(expected_hash),
            actual_hex: to_hex(&actual_hash),
        });
    }
    postcard::from_bytes(&raw_bytes).map_err(|e| StoreError::Postcard(e.to_string()))
}

/// Decodes a 16-byte sortable big-endian SQLite `BLOB` slice into `Money(i128)`.
pub fn decode_money_be16(blob: &[u8]) -> Result<Money, StoreError> {
    let arr: [u8; 16] = blob
        .try_into()
        .map_err(|_| StoreError::InvalidBlobLength {
            expected: 16,
            actual: blob.len(),
        })?;
    Ok(Money::from_sortable_be_bytes(arr))
}

/// Formats a 32-byte hash as a lowercase hexadecimal string.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}
