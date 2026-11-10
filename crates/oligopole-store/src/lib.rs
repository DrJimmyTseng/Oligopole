//! `oligopole-store`: Synchronous bundled `rusqlite` WAL `.oligopole` container,
//! 5-table `STRICT` schema, sortable 16-byte big-endian `i128` BLOBs, `zstd(postcard)` snapshots,
//! and `blake3` deterministic replay verification.

pub mod encoding;
pub mod error;
pub mod outbox;
pub mod repository;
pub mod schema;

pub use encoding::{
    compress_world_snapshot, compute_world_blake3_hash, decode_money_be16,
    decompress_world_snapshot, to_hex,
};
pub use error::StoreError;
pub use outbox::{DecisionOutboxEntry, DecisionSourceKind};
pub use repository::OligopoleStore;
pub use schema::INIT_SCHEMA_SQL;
