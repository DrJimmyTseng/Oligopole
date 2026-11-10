/// Canonical 5-table `STRICT` SQLite schema for `.oligopole` save containers.
pub const INIT_SCHEMA_SQL: &str = r"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS world_meta (
    save_id INTEGER PRIMARY KEY CHECK (save_id = 1),
    schema_version INTEGER NOT NULL,
    world_seed BLOB NOT NULL,
    current_tick INTEGER NOT NULL,
    occ_version INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS world_snapshots (
    tick INTEGER PRIMARY KEY,
    occ_version INTEGER NOT NULL,
    blake3_state_hash BLOB NOT NULL,
    arena_postcard_zstd BLOB NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS decision_outbox_journal (
    tick INTEGER NOT NULL,
    seq_no INTEGER NOT NULL,
    source_kind INTEGER NOT NULL,
    command_postcard BLOB NOT NULL,
    probs_blake3_hash BLOB NOT NULL,
    PRIMARY KEY (tick, seq_no)
) STRICT;

CREATE TABLE IF NOT EXISTS market_ohlcv_history (
    ticker TEXT NOT NULL,
    timeframe_days INTEGER NOT NULL,
    bar_tick INTEGER NOT NULL,
    open_be16 BLOB NOT NULL,
    high_be16 BLOB NOT NULL,
    low_be16 BLOB NOT NULL,
    close_be16 BLOB NOT NULL,
    volume INTEGER NOT NULL,
    PRIMARY KEY (ticker, timeframe_days, bar_tick)
) STRICT;

CREATE TABLE IF NOT EXISTS financial_statement_rollups (
    entity_name TEXT NOT NULL,
    tick INTEGER NOT NULL,
    total_assets_be16 BLOB NOT NULL,
    total_liabilities_be16 BLOB NOT NULL,
    total_equity_be16 BLOB NOT NULL,
    revenue_be16 BLOB NOT NULL,
    net_income_be16 BLOB NOT NULL,
    statement_postcard BLOB NOT NULL,
    PRIMARY KEY (entity_name, tick)
) STRICT;
";
