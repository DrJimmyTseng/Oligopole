use crate::encoding::{
    compress_world_snapshot, compute_world_blake3_hash, decode_money_be16,
    decompress_world_snapshot, to_hex,
};
use crate::error::StoreError;
use crate::outbox::{DecisionOutboxEntry, DecisionSourceKind};
use crate::schema::INIT_SCHEMA_SQL;
use oligopole_core::{PlayerCommand, WorldState};
use oligopole_ledger::{FinancialStatementReport, Money};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::Path;

/// Synchronous bundled `rusqlite` persistence repository for `.oligopole` save containers
/// and `:memory:` headless Monte Carlo / replay verification runs.
pub struct OligopoleStore {
    conn: Connection,
}

impl std::fmt::Debug for OligopoleStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OligopoleStore").finish_non_exhaustive()
    }
}

impl OligopoleStore {
    /// Opens or creates a file-backed `.oligopole` SQLite WAL container.
    pub fn open_file(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn =
            Connection::open(path).map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let store = Self { conn };
        store.initialize_schema()?;
        Ok(store)
    }

    /// Opens an in-memory SQLite database for headless `proptest` and Monte Carlo runs.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn =
            Connection::open_in_memory().map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let store = Self { conn };
        store.initialize_schema()?;
        Ok(store)
    }

    fn initialize_schema(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch(INIT_SCHEMA_SQL)
            .map_err(|e| StoreError::Sqlite(e.to_string()))
    }

    /// Atomically saves a compressed `WorldState` snapshot, appends `outbox_entries` to
    /// `decision_outbox_journal`, and rolls up current OHLCV and Financial Statements using
    /// sortable 16-byte big-endian `i128` BLOBs.
    ///
    /// Returns the 32-byte `blake3` state hash of the saved `WorldState`.
    pub fn save_checkpoint(
        &mut self,
        world: &WorldState,
        outbox_entries: &[DecisionOutboxEntry],
    ) -> Result<[u8; 32], StoreError> {
        let (compressed_snapshot, state_hash) = compress_world_snapshot(world)?;
        let tick_i64 = i64::try_from(world.current_tick.0)
            .map_err(|_| StoreError::Sqlite("tick overflow".to_owned()))?;
        let occ_i64 = i64::try_from(world.occ_version)
            .map_err(|_| StoreError::Sqlite("occ_version overflow".to_owned()))?;

        let tx = self
            .conn
            .transaction()
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;

        tx.execute(
            r"
            INSERT INTO world_meta (save_id, schema_version, world_seed, current_tick, occ_version)
            VALUES (1, 1, ?1, ?2, ?3)
            ON CONFLICT(save_id) DO UPDATE SET
                current_tick = excluded.current_tick,
                occ_version = excluded.occ_version
            ",
            params![world.world_seed.as_slice(), tick_i64, occ_i64],
        )
        .map_err(|e| StoreError::Sqlite(e.to_string()))?;

        tx.execute(
            r"
            INSERT OR REPLACE INTO world_snapshots (tick, occ_version, blake3_state_hash, arena_postcard_zstd)
            VALUES (?1, ?2, ?3, ?4)
            ",
            params![
                tick_i64,
                occ_i64,
                state_hash.as_slice(),
                compressed_snapshot.as_slice()
            ],
        )
        .map_err(|e| StoreError::Sqlite(e.to_string()))?;

        for entry in outbox_entries {
            let cmd_bytes = postcard::to_allocvec(&entry.command)
                .map_err(|e| StoreError::Postcard(e.to_string()))?;
            let entry_tick = i64::try_from(entry.tick)
                .map_err(|_| StoreError::Sqlite("entry tick overflow".to_owned()))?;
            let seq_i64 = i64::from(entry.seq_no);

            tx.execute(
                r"
                INSERT OR REPLACE INTO decision_outbox_journal
                    (tick, seq_no, source_kind, command_postcard, probs_blake3_hash)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ",
                params![
                    entry_tick,
                    seq_i64,
                    entry.source_kind.to_i64(),
                    cmd_bytes.as_slice(),
                    entry.probs_blake3_hash.as_slice()
                ],
            )
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        }

        // Rollup latest OHLCV bars per instrument using sortable 16-byte BE i128 BLOBs
        for (_, book) in &world.order_books {
            if let Some(bar) = book.history_bars.back() {
                let bar_tick = i64::try_from(bar.bar_tick).unwrap_or(0);
                let vol_i64 = i64::try_from(bar.volume).unwrap_or(i64::MAX);
                tx.execute(
                    r"
                    INSERT OR REPLACE INTO market_ohlcv_history
                        (ticker, timeframe_days, bar_tick, open_be16, high_be16, low_be16, close_be16, volume)
                    VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7)
                    ",
                    params![
                        book.ticker,
                        bar_tick,
                        bar.open.to_sortable_be_bytes().as_slice(),
                        bar.high.to_sortable_be_bytes().as_slice(),
                        bar.low.to_sortable_be_bytes().as_slice(),
                        bar.close.to_sortable_be_bytes().as_slice(),
                        vol_i64
                    ],
                )
                .map_err(|e| StoreError::Sqlite(e.to_string()))?;
            }
        }

        // Rollup corporate financial statements using sortable 16-byte BE i128 BLOBs
        for (_, corp) in &world.corporations {
            let report = FinancialStatementReport::generate(
                &world.ledger,
                corp.entity_id,
                world.current_tick.0,
            )
            .map_err(oligopole_core::CoreError::from)?;
            let stmt_bytes =
                postcard::to_allocvec(&report).map_err(|e| StoreError::Postcard(e.to_string()))?;

            tx.execute(
                r"
                INSERT OR REPLACE INTO financial_statement_rollups
                    (entity_name, tick, total_assets_be16, total_liabilities_be16, total_equity_be16, revenue_be16, net_income_be16, statement_postcard)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ",
                params![
                    corp.ticker,
                    tick_i64,
                    report.total_assets.to_sortable_be_bytes().as_slice(),
                    report.total_liabilities.to_sortable_be_bytes().as_slice(),
                    report
                        .total_equity_including_net_income
                        .to_sortable_be_bytes()
                        .as_slice(),
                    report.total_revenue.to_sortable_be_bytes().as_slice(),
                    report.net_income.to_sortable_be_bytes().as_slice(),
                    stmt_bytes.as_slice()
                ],
            )
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        }

        tx.commit()
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        Ok(state_hash)
    }

    /// Loads the most recent `(WorldState, blake3_state_hash)` from `world_snapshots`.
    pub fn load_latest_snapshot(&self) -> Result<(WorldState, [u8; 32]), StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                r"
                SELECT blake3_state_hash, arena_postcard_zstd
                FROM world_snapshots
                ORDER BY tick DESC
                LIMIT 1
                ",
            )
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;

        let mut rows = stmt
            .query([])
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let Some(row) = rows
            .next()
            .map_err(|e| StoreError::Sqlite(e.to_string()))?
        else {
            return Err(StoreError::MissingSnapshot);
        };

        let hash_blob: Vec<u8> = row.get(0).map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let compressed_blob: Vec<u8> =
            row.get(1).map_err(|e| StoreError::Sqlite(e.to_string()))?;

        let expected_hash: [u8; 32] =
            hash_blob
                .as_slice()
                .try_into()
                .map_err(|_| StoreError::InvalidBlobLength {
                    expected: 32,
                    actual: hash_blob.len(),
                })?;

        let world = decompress_world_snapshot(&compressed_blob, &expected_hash)?;
        Ok((world, expected_hash))
    }

    /// Loads all recorded `DecisionOutboxEntry` events ordered by `(tick ASC, seq_no ASC)`.
    pub fn load_all_outbox_entries(&self) -> Result<Vec<DecisionOutboxEntry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                r"
                SELECT tick, seq_no, source_kind, command_postcard, probs_blake3_hash
                FROM decision_outbox_journal
                ORDER BY tick ASC, seq_no ASC
                ",
            )
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;

        let mut rows = stmt
            .query([])
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let mut entries = Vec::new();

        while let Some(row) = rows
            .next()
            .map_err(|e| StoreError::Sqlite(e.to_string()))?
        {
            let tick_i64: i64 = row.get(0).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let seq_i64: i64 = row.get(1).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let kind_i64: i64 = row.get(2).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let cmd_blob: Vec<u8> = row.get(3).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let hash_blob: Vec<u8> = row.get(4).map_err(|e| StoreError::Sqlite(e.to_string()))?;

            let command: PlayerCommand = postcard::from_bytes(&cmd_blob)
                .map_err(|e| StoreError::Postcard(e.to_string()))?;
            let probs_blake3_hash: [u8; 32] =
                hash_blob
                    .as_slice()
                    .try_into()
                    .map_err(|_| StoreError::InvalidBlobLength {
                        expected: 32,
                        actual: hash_blob.len(),
                    })?;

            entries.push(DecisionOutboxEntry {
                tick: u64::try_from(tick_i64).unwrap_or(0),
                seq_no: u32::try_from(seq_i64).unwrap_or(0),
                source_kind: DecisionSourceKind::from_i64(kind_i64),
                command,
                probs_blake3_hash,
            });
        }

        Ok(entries)
    }

    /// Queries corporate net incomes at `tick` ordered descending directly in SQLite using the
    /// sortable 16-byte big-endian `net_income_be16` BLOB column!
    pub fn query_corporations_by_net_income_desc(
        &self,
        tick: u64,
    ) -> Result<Vec<(String, Money)>, StoreError> {
        let tick_i64 = i64::try_from(tick).unwrap_or(0);
        let mut stmt = self
            .conn
            .prepare(
                r"
                SELECT entity_name, net_income_be16
                FROM financial_statement_rollups
                WHERE tick = ?1
                ORDER BY net_income_be16 DESC, entity_name ASC
                ",
            )
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;

        let mut rows = stmt
            .query(params![tick_i64])
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let mut results = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StoreError::Sqlite(e.to_string()))?
        {
            let name: String = row.get(0).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let be16: Vec<u8> = row.get(1).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let net_income = decode_money_be16(&be16)?;
            results.push((name, net_income));
        }
        Ok(results)
    }

    /// Replays `genesis_world` forward up to the highest recorded snapshot tick, injecting all
    /// `decision_outbox_journal` commands at their recorded ticks and verifying that every single
    /// checkpoint's `blake3_state_hash` matches bit-for-bit.
    pub fn verify_deterministic_replay(
        &self,
        mut replay_world: WorldState,
    ) -> Result<usize, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                r"
                SELECT tick, blake3_state_hash
                FROM world_snapshots
                ORDER BY tick ASC
                ",
            )
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;

        let mut rows = stmt
            .query([])
            .map_err(|e| StoreError::Sqlite(e.to_string()))?;
        let mut snapshot_hashes: BTreeMap<u64, [u8; 32]> = BTreeMap::new();
        let mut max_tick = 0u64;

        while let Some(row) = rows
            .next()
            .map_err(|e| StoreError::Sqlite(e.to_string()))?
        {
            let t_i64: i64 = row.get(0).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let h_blob: Vec<u8> = row.get(1).map_err(|e| StoreError::Sqlite(e.to_string()))?;
            let tick = u64::try_from(t_i64).unwrap_or(0);
            let hash: [u8; 32] =
                h_blob
                    .as_slice()
                    .try_into()
                    .map_err(|_| StoreError::InvalidBlobLength {
                        expected: 32,
                        actual: h_blob.len(),
                    })?;
            if tick > max_tick {
                max_tick = tick;
            }
            snapshot_hashes.insert(tick, hash);
        }

        if snapshot_hashes.is_empty() {
            return Err(StoreError::MissingSnapshot);
        }

        // Group outbox commands by tick
        let all_entries = self.load_all_outbox_entries()?;
        let mut commands_by_tick: BTreeMap<u64, Vec<PlayerCommand>> = BTreeMap::new();
        for entry in all_entries {
            commands_by_tick
                .entry(entry.tick)
                .or_default()
                .push(entry.command);
        }

        let mut verified_count = 0usize;
        if let Some(expected_0) = snapshot_hashes.get(&0) {
            let actual_0 = compute_world_blake3_hash(&replay_world)?;
            if &actual_0 != expected_0 {
                return Err(StoreError::ReplayHashMismatch {
                    tick: 0,
                    expected_hex: to_hex(expected_0),
                    actual_hex: to_hex(&actual_0),
                });
            }
            verified_count += 1;
        }

        for target_tick in 1..=max_tick {
            let cmds = commands_by_tick
                .remove(&target_tick)
                .unwrap_or_default();
            replay_world.step_fast_tick(&cmds)?;

            if let Some(expected_hash) = snapshot_hashes.get(&target_tick) {
                let actual_hash = compute_world_blake3_hash(&replay_world)?;
                if &actual_hash != expected_hash {
                    return Err(StoreError::ReplayHashMismatch {
                        tick: target_tick,
                        expected_hex: to_hex(expected_hash),
                        actual_hex: to_hex(&actual_hash),
                    });
                }
                verified_count += 1;
            }
        }

        Ok(verified_count)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oligopole_ai::{
        ActorObservationView, DecisionPolicy, PersonaBiasVector, System0HeuristicPolicy,
    };
    use oligopole_core::build_vertical_slice_0;

    #[test]
    fn test_oligopole_store_snapshot_sortable_be16_and_deterministic_replay() {
        let seed = [19u8; 32];
        let (mut world, handles) = build_vertical_slice_0(seed).unwrap();
        let mut store = OligopoleStore::open_in_memory().unwrap();
        let mut policy = System0HeuristicPolicy::new();
        let persona = PersonaBiasVector::balanced_executive();

        // Save Genesis snapshot at tick 0
        store.save_checkpoint(&world, &[]).unwrap();

        let mut pending_outbox = Vec::new();
        for day in 1..=60 {
            let mut day_cmds = Vec::new();
            if day == 30 || day == 60 {
                let obs = ActorObservationView::build(&world, handles.acme_corp).unwrap();
                let (directive, probs_hash) = policy
                    .evaluate_actor_slow_tick(seed, &obs, &persona)
                    .unwrap();
                let cmd = PlayerCommand::ApplyStrategicDirective(directive);
                day_cmds.push(cmd.clone());
                pending_outbox.push(DecisionOutboxEntry {
                    tick: day,
                    seq_no: 0,
                    source_kind: DecisionSourceKind::System0Heuristic,
                    command: cmd,
                    probs_blake3_hash: probs_hash,
                });
            }
            world.step_fast_tick(&day_cmds).unwrap();

            if day == 30 || day == 60 {
                store.save_checkpoint(&world, &pending_outbox).unwrap();
                pending_outbox.clear();
            }
        }

        // Verify SQLite byte-order sortability on `net_income_be16` at tick 60
        let ranked = store.query_corporations_by_net_income_desc(60).unwrap();
        assert_eq!(ranked.len(), 6);
        for pair in ranked.windows(2) {
            let [(_, inc_a), (_, inc_b)] = pair else {
                continue;
            };
            assert!(inc_a >= inc_b);
        }

        // Verify full zstd(postcard) snapshot decompression roundtrip
        let (loaded_world, loaded_hash) = store.load_latest_snapshot().unwrap();
        assert_eq!(loaded_world.current_tick.0, 60);
        assert_eq!(
            compute_world_blake3_hash(&loaded_world).unwrap(),
            loaded_hash
        );

        // Verify bit-for-bit deterministic replay from a fresh Genesis world
        let (fresh_genesis, _) = build_vertical_slice_0(seed).unwrap();
        let verified_checkpoints = store.verify_deterministic_replay(fresh_genesis).unwrap();
        assert_eq!(verified_checkpoints, 3); // Ticks 0, 30, 60
    }
}
