# Quickstart: 数据源管理

## Prerequisites

- Rust toolchain (stable, as specified in `rust-toolchain.toml`)
- A running MySQL 5.7+ instance for testing
- HiveClaw service running (`cargo run -p hiveclaw`)
- HiveGUI running (`cargo run -p hivegui`)

## Setup

### 1. Add dependencies

Add the following to `crates/hiveclaw/Cargo.toml`:

```toml
[dependencies]
mysql_async = "0.34"
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "sqlite", "chrono"] }
chacha20poly1305 = "0.10"
zeroize = "1"
argon2 = "0.5"
rand = "0.8"
```

### 2. Initialize SQLite storage

On first run, HiveClaw will automatically create the SQLite database and `data_sources` table.

Default SQLite path: `~/.hiveclaw/datasources.db` (configurable via environment variable `HIVECLAW_DATASOURCES_DB`)

### 3. Run HiveClaw

```bash
cargo run -p hiveclaw
```

The service starts on `127.0.0.1:8686` with the new `/api/v1/datasources` endpoints.

### 4. Run HiveGUI

```bash
cargo run -p hivegui
```

The data source management UI will be accessible from the main window.

## Using the Feature

### Add a Data Source

1. Click "Add Data Source" in the left panel
2. Fill in:
   - **Name** (custom, pre-filled with `mysql@host:port`)
   - **Host** (IP or domain)
   - **Port** (default: 3306)
   - **Username**
   - **Password**
3. Click "Test Connection" to verify
4. Click "Save" to persist

### Browse Databases and Tables

1. Select a data source from the left panel
2. The middle panel shows the tree: expand to see databases → tables
3. Click a table to view its details in the right panel

### View Table Details

Three tabs in the right panel:
- **列**: Column names, types, nullable, defaults, keys, comments
- **DDL**: Full CREATE TABLE statement with syntax highlighting
- **数据**: Preview of table data (default 100 rows, paginated)
  - Optional: enter WHERE clause and ORDER BY for filtered queries

### Edit/Delete a Data Source

1. Select a data source in the left panel
2. Click "Edit" to modify connection details (requires re-test)
3. Click "Delete" to remove (confirmation required)

## Testing

```bash
# Run all tests
cargo test --workspace

# Run contract tests for datasource API
cargo test -p hiveclaw --test contract_datasource_api

# Run contract tests for datasource UI
cargo test -p hivegui --test contract_datasource_ui
```

## Configuration

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `HIVECLAW_DATASOURCES_DB` | `~/.hiveclaw/datasources.db` | SQLite database path |
| `HIVECLAW_CRYPTO_KEY` | (auto-generated) | Encryption key for password storage (hex-encoded) |
| `HIVECLAW_CONNECTION_TIMEOUT` | `10` | MySQL connection timeout (seconds) |
