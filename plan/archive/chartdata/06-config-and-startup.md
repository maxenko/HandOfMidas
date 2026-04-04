# 06 - Configuration, Startup, and Graceful Fallback

## Overview

This document specifies how the DuckDB store is configured via TOML, how it
integrates with the existing `AppConfig` system, the complete initialization
sequence in `MidasApp::new()`, memory budget analysis, hot-reload behavior,
and diagnostic tooling.

**Prerequisite reads:**
- [04-dbhandle-api.md](04-dbhandle-api.md) -- `DbHandle` API
- [05-data-flow.md](05-data-flow.md) -- data loading flows

---

## 1. StoreConfig Struct

Add to `midas-core/src/config.rs` alongside the existing `WindowConfig`,
`ThemeConfig`, and `ChartConfig` structs:

```rust
/// Configuration for the DuckDB persistent cache store.
///
/// Serialized as the `[store]` section in `config.toml`. All fields have
/// sensible defaults so existing config files without a `[store]` section
/// continue to work (serde `#[serde(default)]` on the `AppConfig.store` field
/// provides backward compatibility).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    /// Whether the DuckDB cache is enabled.
    ///
    /// When `false`, `MidasApp.store` is `None` and all data loading falls
    /// through to TestDataProvider/CSV/IB directly. The app behaves identically
    /// to the pre-DuckDB era.
    ///
    /// Default: `true`
    #[serde(default = "default_store_enabled")]
    pub enabled: bool,

    /// Path to the DuckDB database file, relative to the data directory.
    ///
    /// The data directory is resolved via `dirs::data_local_dir()` + "HandOfMidas"
    /// (e.g., `C:\Users\max\AppData\Local\HandOfMidas\` on Windows). If the
    /// platform directory cannot be determined, falls back to `./data/`.
    ///
    /// Set to `:memory:` for in-memory mode (useful for testing; data is lost
    /// on exit).
    ///
    /// Default: `"cache.duckdb"`
    #[serde(default = "default_store_path")]
    pub path: String,

    /// Maximum memory DuckDB is allowed to use, in megabytes.
    ///
    /// Applied via `SET memory_limit = '{X}MB'` on connection open.
    /// DuckDB spills to disk when this limit is exceeded, so setting it
    /// lower does not prevent large queries from completing -- it just
    /// forces more disk I/O.
    ///
    /// Recommendation: 64-256MB. The GPU and iced UI are the real memory
    /// consumers. DuckDB should not compete with them.
    ///
    /// Default: `256`
    #[serde(default = "default_memory_limit_mb")]
    pub memory_limit_mb: u32,

    /// Number of threads DuckDB may use for query execution.
    ///
    /// Applied via `SET threads = {N}` on connection open. This controls
    /// DuckDB's internal parallelism, not the number of OS threads the actor
    /// uses (always 1). Setting this to 1 is fine for our workload (queries
    /// are fast and infrequent). Setting to 2-4 helps with larger analytical
    /// queries (cross-symbol scans, aggregations).
    ///
    /// Default: `2`
    #[serde(default = "default_threads")]
    pub threads: u8,

    /// Interval in seconds for batch-flushing streaming data to DuckDB.
    ///
    /// Only relevant when IB streaming is active (Phase 2). During streaming,
    /// closed candles accumulate in memory and are batch-inserted into DuckDB
    /// every `flush_interval_secs`. Lower values = more frequent writes but
    /// lower data loss window on crash. Higher values = better write batching.
    ///
    /// Default: `5`
    #[serde(default = "default_flush_interval_secs")]
    pub flush_interval_secs: u32,
}

fn default_store_enabled() -> bool {
    true
}

fn default_store_path() -> String {
    "cache.duckdb".into()
}

fn default_memory_limit_mb() -> u32 {
    256
}

fn default_threads() -> u8 {
    2
}

fn default_flush_interval_secs() -> u32 {
    5
}
```

### Default Implementation

```rust
impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            enabled: default_store_enabled(),
            path: default_store_path(),
            memory_limit_mb: default_memory_limit_mb(),
            threads: default_threads(),
            flush_interval_secs: default_flush_interval_secs(),
        }
    }
}
```

### Validation

```rust
impl StoreConfig {
    /// Validate configuration values and clamp to safe ranges.
    ///
    /// Called after deserialization. Logs warnings for clamped values.
    pub fn validate(&mut self) {
        // Memory limit: clamp to 32-1024 MB.
        if self.memory_limit_mb < 32 {
            tracing::warn!(
                "store.memory_limit_mb={} is below minimum, clamping to 32",
                self.memory_limit_mb
            );
            self.memory_limit_mb = 32;
        }
        if self.memory_limit_mb > 1024 {
            tracing::warn!(
                "store.memory_limit_mb={} is above maximum, clamping to 1024",
                self.memory_limit_mb
            );
            self.memory_limit_mb = 1024;
        }

        // Threads: clamp to 1-8.
        if self.threads == 0 {
            tracing::warn!("store.threads=0 is invalid, clamping to 1");
            self.threads = 1;
        }
        if self.threads > 8 {
            tracing::warn!(
                "store.threads={} is above maximum, clamping to 8",
                self.threads
            );
            self.threads = 8;
        }

        // Flush interval: clamp to 1-60 seconds.
        if self.flush_interval_secs == 0 {
            tracing::warn!("store.flush_interval_secs=0 is invalid, clamping to 1");
            self.flush_interval_secs = 1;
        }
        if self.flush_interval_secs > 60 {
            tracing::warn!(
                "store.flush_interval_secs={} is above maximum, clamping to 60",
                self.flush_interval_secs
            );
            self.flush_interval_secs = 60;
        }

        // Path: reject empty strings.
        if self.path.trim().is_empty() {
            tracing::warn!("store.path is empty, using default 'cache.duckdb'");
            self.path = default_store_path();
        }
    }
}
```

---

## 2. config.toml Integration

### Adding the `[store]` Section to AppConfig

Modify the existing `AppConfig` in `midas-core/src/config.rs`:

```rust
/// Root application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Window size settings.
    pub window: WindowConfig,
    /// Theme settings.
    pub theme: ThemeConfig,
    /// Per-chart configuration, persisted across sessions.
    #[serde(default)]
    pub charts: Vec<ChartConfig>,
    /// Per-ticker horizontal levels, keyed by symbol.
    #[serde(default)]
    pub levels: HashMap<String, Vec<LevelConfig>>,
    /// DuckDB persistent cache configuration.
    ///
    /// Defaults to enabled with standard settings when the `[store]` section
    /// is absent from the config file (backward compatibility).
    #[serde(default)]
    pub store: StoreConfig,
}
```

### Backward Compatibility

The `#[serde(default)]` attribute on the `store` field means:

1. **Existing config files without `[store]`** -- `StoreConfig::default()` is used.
   The store is enabled with default settings. First launch with DuckDB
   "just works" without user intervention.

2. **New config files** -- The `[store]` section is serialized on save, so after
   the first config save the section appears in the file for manual editing.

3. **Config with `[store]` section** -- All fields within `[store]` also have
   `#[serde(default = "...")]`, so partial `[store]` sections work:
   ```toml
   [store]
   memory_limit_mb = 128
   # 'enabled', 'path', 'threads', 'flush_interval_secs' all use defaults
   ```

### Example config.toml

```toml
[window]
width = 2560
height = 1440
maximized = true
x = 0
y = 0

[theme]
mode = "dark"

[store]
enabled = true
path = "cache.duckdb"
memory_limit_mb = 256
threads = 2
flush_interval_secs = 5

[[charts]]
symbol = "AAPL"
timeframe = "1D"
collapse_gaps = false

[[charts]]
symbol = "MSFT"
timeframe = "5m"
collapse_gaps = true

[levels.AAPL]
# ... level configs ...
```

### Disabling the Store

Users who do not want DuckDB (e.g., minimal memory footprint, avoiding C++
dependency build issues, or simply preferring the old behavior) set:

```toml
[store]
enabled = false
```

The app starts faster (no DuckDB initialization), uses less memory, and
behaves identically to the pre-DuckDB codebase.

---

## 3. Data Directory Resolution

### Where cache.duckdb Lives

The store path in `StoreConfig` is relative to the application's data
directory. The data directory is determined as follows:

```rust
/// Resolve the absolute path to the DuckDB database file.
///
/// The `store_config.path` is relative to the application data directory:
/// - Windows: `C:\Users\<user>\AppData\Local\HandOfMidas\`
/// - Linux:   `~/.local/share/HandOfMidas/`
/// - macOS:   `~/Library/Application Support/HandOfMidas/`
///
/// Falls back to `./data/` if the platform directory cannot be determined.
///
/// Special case: if `path` is `:memory:`, returns it as-is (DuckDB in-memory
/// mode, no file created).
pub fn resolve_store_path(config: &StoreConfig) -> PathBuf {
    if config.path == ":memory:" {
        return PathBuf::from(":memory:");
    }

    let data_dir = dirs::data_local_dir()
        .map(|d| d.join("HandOfMidas"))
        .unwrap_or_else(|| PathBuf::from("data"));

    data_dir.join(&config.path)
}
```

### Platform-Specific Paths

| Platform | `dirs::data_local_dir()` | Full DuckDB Path |
|----------|--------------------------|------------------|
| Windows 11 | `C:\Users\max\AppData\Local` | `C:\Users\max\AppData\Local\HandOfMidas\cache.duckdb` |
| Linux | `~/.local/share` | `~/.local/share/HandOfMidas/cache.duckdb` |
| macOS | `~/Library/Application Support` | `~/Library/Application Support/HandOfMidas/cache.duckdb` |
| Fallback | `./data` | `./data/cache.duckdb` |

### Directory Creation

The `DbHandle::open()` implementation creates parent directories if they do
not exist (matching the existing `AppConfig::save()` behavior):

```rust
pub fn open(path: impl AsRef<Path>, config: &StoreConfig) -> Result<Self, StoreError> {
    let path = path.as_ref();

    // Create parent directory if it does not exist.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    // ... open DuckDB connection ...
}
```

### Config Path vs Store Path

The existing config file lives in the **config** directory:
```
C:\Users\max\AppData\Local\HandOfMidas\config.toml   (via dirs::config_local_dir())
```

The DuckDB store lives in the **data** directory:
```
C:\Users\max\AppData\Local\HandOfMidas\cache.duckdb  (via dirs::data_local_dir())
```

On Windows, `config_local_dir()` and `data_local_dir()` both resolve to
`AppData\Local`, so both files end up in the same directory. On Linux they
differ (`~/.config/` vs `~/.local/share/`). This is the standard platform
convention.

---

## 4. DuckDB Connection Configuration

### SET Statements Applied on Every Open

When the DuckDB connection is opened, the following configuration is applied
immediately before any schema operations. These are **session-scoped** settings
(not persisted in the database file):

```rust
/// Apply runtime configuration to a DuckDB connection.
///
/// Called once when the actor thread opens the connection. These settings
/// are session-scoped and must be re-applied on every open.
fn configure_connection(
    conn: &duckdb::Connection,
    config: &StoreConfig,
) -> Result<(), StoreError> {
    // ── Memory limit ──────────────────────────────────────────────
    // Controls how much memory DuckDB uses for query execution and
    // buffer management. Spills to disk temp files when exceeded.
    conn.execute_batch(&format!(
        "SET memory_limit = '{}MB'",
        config.memory_limit_mb
    ))?;

    // ── Thread count ──────────────────────────────────────────────
    // Controls DuckDB's internal query parallelism. Our queries are
    // simple range scans; 2 threads is sufficient. Higher values may
    // help with cross-symbol analytical queries.
    conn.execute_batch(&format!(
        "SET threads = {}",
        config.threads
    ))?;

    // ── Temp directory ────────────────────────────────────────────
    // When DuckDB spills to disk (exceeding memory_limit), it creates
    // temp files. Direct these to the same directory as the database
    // file to keep everything in one place.
    //
    // On Windows, the default system temp dir is deep in AppData and
    // sometimes causes issues with antivirus scanners. Placing temp
    // files next to the database avoids this.
    //
    // Note: Only set if we have a file-backed database (not :memory:).
    // The temp_directory path should be the parent directory of the
    // database file, not a file path.
    //
    // Implementation: this is handled by the caller based on the
    // resolved path. See open() below.

    // ── Progress bar ──────────────────────────────────────────────
    // DuckDB can print a progress bar to stderr for long-running
    // queries. We never want this in a desktop app.
    conn.execute_batch("SET enable_progress_bar = false")?;

    // ── Object cache ──────────────────────────────────────────────
    // Caches metadata objects (table schemas, prepared statements).
    // Speeds up repeated queries against the same tables, which is
    // exactly our usage pattern (same SELECT against market.candles
    // with different parameter values).
    conn.execute_batch("SET enable_object_cache = true")?;

    // ── Checkpoint threshold ──────────────────────────────────────
    // DuckDB uses WAL mode by default. This controls how often the
    // WAL is checkpointed into the main database file. Default is
    // 16MB. For a desktop app with infrequent writes, a lower
    // threshold keeps the WAL file small.
    conn.execute_batch("SET wal_autocheckpoint = '4MB'")?;

    Ok(())
}
```

### Temp Directory Configuration

```rust
/// Configure the DuckDB temp directory based on the database path.
///
/// Only applicable for file-backed databases (not :memory:).
fn configure_temp_directory(
    conn: &duckdb::Connection,
    db_path: &Path,
) -> Result<(), StoreError> {
    if let Some(parent) = db_path.parent() {
        let temp_dir = parent.join(".duckdb_temp");
        // Create the temp directory if it doesn't exist.
        std::fs::create_dir_all(&temp_dir).map_err(|e| StoreError::Io {
            path: temp_dir.clone(),
            source: e,
        })?;
        conn.execute_batch(&format!(
            "SET temp_directory = '{}'",
            temp_dir.display()
        ))?;
    }
    Ok(())
}
```

### Full Connection Setup Sequence

```rust
fn setup_connection(
    path: &Path,
    config: &StoreConfig,
) -> Result<duckdb::Connection, StoreError> {
    // 1. Open the DuckDB database file (or create it).
    let conn = duckdb::Connection::open(path)?;

    // 2. Apply session configuration.
    configure_connection(&conn, config)?;

    // 3. Set temp directory (file-backed databases only).
    if path.to_str() != Some(":memory:") {
        configure_temp_directory(&conn, path)?;
    }

    // 4. Run schema migrations.
    run_migrations(&conn)?;

    Ok(conn)
}
```

---

## 5. Graceful Fallback

### Design Principle

The DuckDB store is an **optional optimization**. The app must function
identically with or without it. There are no features that **require**
DuckDB to work -- it only accelerates data loading on subsequent launches.

### Fallback Scenarios

| Scenario | Behavior |
|----------|----------|
| `store.enabled = false` | `self.store = None`. No DuckDB. Direct TestDataProvider loads. |
| `DbHandle::open()` fails | `tracing::warn!()`. `self.store = None`. Direct loads. |
| DuckDB query returns error | `tracing::warn!()`. Treated as cache miss. Falls through to TestDataProvider. |
| DuckDB insert fails (write-behind) | `tracing::warn!()`. Chart already rendered. Data re-fetched on next launch. |
| DuckDB file corrupted | Open fails or queries fail. Falls through. User can delete `cache.duckdb` to reset. |
| Actor thread panics | All subsequent `mb.send()` calls fail with `MailboxProcessorError`. Treated as query errors. |

### No User-Visible Errors

DuckDB failures never produce error dialogs, toast notifications, or status
bar error messages. The user should not know or care whether DuckDB is involved.
All failures are logged at `tracing::warn!` level for developer diagnostics.

```rust
// The only places the user sees feedback about data source:
self.status_message = format!(
    "{}: {} candles at {} (cached)",    // cache hit
    symbol, count, tf
);
self.status_message = format!(
    "{}: {} candles at {} (fetched)",   // cache miss / fallback
    symbol, count, tf
);
// "(fetched)" vs "(cached)" is the only visible difference.
```

### Recovery

If DuckDB gets into a bad state (corrupted file, wedged actor), the user can:

1. **Restart the app.** The actor thread is re-created, connection is re-opened.
2. **Delete `cache.duckdb`.** The store is recreated from scratch on next launch.
   No data is lost permanently -- it is re-fetched from TestDataProvider/IB.
3. **Set `store.enabled = false` in config.toml.** Disables DuckDB entirely.

None of these recovery actions lose user state (levels, annotations, chart
layouts). Those are stored in `config.toml` and the per-symbol JSON files
managed by `AnnotationStore`.

---

## 6. Initialization Sequence in MidasApp::new()

### Complete Code

```rust
impl MidasApp {
    /// Create a new application, restoring state from config if available.
    ///
    /// Returns the app state and a `Task` that opens the main OS window
    /// and loads data for all restored charts.
    pub fn new() -> (Self, Task<Message>) {
        let config_path = Self::config_file_path();
        Self::migrate_legacy_config(&config_path);

        // ── 1. Load config ───────────────────────────────────────────
        let config = match AppConfig::load(&config_path) {
            Ok(mut cfg) => {
                tracing::info!("Loaded config from {}", config_path.display());
                // Validate store config (clamp out-of-range values).
                cfg.store.validate();
                cfg
            }
            Err(e) => {
                tracing::warn!("Failed to load config: {e}, using defaults");
                AppConfig::default()
            }
        };

        let now = chrono::Local::now();
        let current_time = now.format("%H:%M:%S").to_string();

        // ── 2. Open main window ──────────────────────────────────────
        let initial_position = Self::validate_saved_position(&config.window);
        let initial_size = (config.window.width, config.window.height);

        let (main_id, open_task) = window::open(window::Settings {
            size: iced::Size::new(
                config.window.width as f32,
                config.window.height as f32,
            ),
            position: initial_position,
            ..window::Settings::default()
        });
        let open_task = open_task.map(Message::MainWindowOpened);

        // ── 3. DuckDB store initialization ───────────────────────────
        // DbHandle::open() is synchronous — it only creates the channel and
        // spawns the actor thread. Connection opens lazily on first command.
        let store = if config.store.enabled {
            let db_config = StoreConfig {
                path: Some(PathBuf::from(&config.store.path)),
                memory_limit_mb: config.store.memory_limit_mb,
                threads: config.store.threads,
                ..Default::default()
            };
            Some(DbHandle::open(db_config))
        } else {
            tracing::info!("DuckDB store disabled in config");
            None
        };

        // Surface connection errors via a health-check task:
        let store_task = if let Some(ref db) = store {
            let db = db.clone();
            Task::perform(
                async move { db.list_cached().await },
                Message::StoreReady,
            )
        } else {
            Task::none()
        };

        // ── 4. Build workspace and charts from config ────────────────
        let (workspace, charts, status_message) = if config.charts.is_empty() {
            let (ws, first_id) = WorkspaceLayout::single();
            let mut charts = HashMap::new();
            charts.insert(first_id, Self::make_empty_panel());
            (ws, charts, "Ready".to_string())
        } else {
            let (mut ws, first_id) = WorkspaceLayout::single();
            let mut charts = HashMap::new();

            let first_cfg = &config.charts[0];
            let panel = Self::restore_panel(first_cfg);
            charts.insert(first_id, panel);

            let first_pane = ws.focus.unwrap();
            for chart_cfg in config.charts.iter().skip(1) {
                let panel = Self::restore_panel(chart_cfg);
                if let Some((new_id, _)) =
                    ws.split(pane_grid::Axis::Vertical, first_pane)
                {
                    charts.insert(new_id, panel);
                }
            }
            ws.set_focus(first_pane);

            let n = charts.len();
            (ws, charts, format!("Restored {n} chart(s) from config"))
        };

        let level_store = LevelStore::from_config(&config.levels);

        // ── 5. Build MidasApp ────────────────────────────────────────
        let mut app = Self {
            charts,
            workspace,
            status_message,
            show_frame_overlay: false,
            config_path,
            config_dirty: false,
            last_config_save: Instant::now(),
            current_time,
            main_window: Some(main_id),
            floating_charts: HashMap::new(),
            window_position: config.window.x.zip(config.window.y),
            window_size: initial_size,
            monitor_size: None,
            level_store,
            level_placing: false,
            placing_preview: None,
            crosshair_sync: None,
            test_data: TestDataProvider::new(),
            store,  // <-- new field: Option<DbHandle>
        };

        // ── 6. Build startup data-loading tasks ──────────────────────
        let load_tasks = app.build_startup_load_tasks();

        // Combine window-open + data-load tasks.
        let startup_tasks = Task::batch(vec![open_task, load_tasks]);

        (app, startup_tasks)
    }
}
```

### Changes from Existing MidasApp::new()

| Step | Before | After |
|------|--------|-------|
| Config loading | Direct use | Plus `cfg.store.validate()` |
| Store init | N/A | `DbHandle::open()` with fallback |
| MidasApp struct | No `store` field | `store: Option<DbHandle>` |
| Data loading | Sync loop calling `load_test_data_for_chart()` | `build_startup_load_tasks()` returns batched async tasks |
| Return | `(app, open_task)` | `(app, Task::batch([open_task, load_tasks]))` |

### MidasApp Struct Addition

```rust
pub struct MidasApp {
    // ... all existing fields unchanged ...

    /// DuckDB persistent cache handle.
    ///
    /// `None` when the store is disabled in config or failed to open.
    /// When `None`, all data loading falls through to `TestDataProvider`
    /// (pre-DuckDB behavior, zero regression).
    ///
    /// `DbHandle::clone()` is cheap (clones the internal mpsc sender).
    store: Option<DbHandle>,
}
```

---

## 7. Memory Budget Analysis

### Component Breakdown (20 Charts, Daily Data)

| Component | Size Estimate | Notes |
|-----------|--------------|-------|
| **mmap'd binary files** | ~3.2 MB | 20 files x ~2500 candles x 64 bytes/record |
| **SoA CandleBuffers** (L1) | ~2.8 MB | 20 buffers x 2500 candles x 56 bytes (6 fields x 8+4+4+4+4+4 bytes avg) |
| **Arc overhead** | negligible | 20 x 16 bytes (strong + weak counts) |
| **ChartState** (per chart) | ~2 KB each | Camera, dirty flags, interaction state |
| **GPU vertex buffers** | ~20-40 MB | Instance data for candles, volume, grid, crosshair, levels. Depends on visible count and number of overlays. |
| **wgpu internal state** | ~10-20 MB | Device, queues, pipeline caches, texture atlases |
| **iced UI framework** | ~30-50 MB | Widget tree, layout cache, text atlas, event queue |
| **DuckDB (L2)** | **64-256 MB** | **Configurable via `memory_limit_mb`** |
| **DuckDB database file** | ~5-50 MB | On disk, not in RSS. Mapped pages may appear in working set. |
| **Rust runtime** | ~5-10 MB | Stack, allocator metadata, thread stacks |
| **OS overhead** | ~20-30 MB | Heap fragmentation, DLL mappings, window surfaces |

### Total Memory Estimates

| Configuration | DuckDB Memory | Total Estimated RSS |
|---------------|--------------|---------------------|
| **Minimal** (store.enabled = false) | 0 MB | ~80-120 MB |
| **Default** (memory_limit_mb = 256) | 64-256 MB | ~150-370 MB |
| **Conservative** (memory_limit_mb = 64) | 32-64 MB | ~120-180 MB |
| **Aggressive** (memory_limit_mb = 512) | 128-512 MB | ~220-630 MB |

### Recommendation

The performance target from `CLAUDE.md` is **< 200 MB** for 20 charts with
1 year daily data each. To stay within this budget:

1. **Set `memory_limit_mb = 128`** as the default (not 256). This gives
   DuckDB enough room for query execution without blowing the budget.
   Change `default_memory_limit_mb()` to return `128` instead of `256`.

2. DuckDB's `memory_limit` controls the **buffer manager** (cached pages,
   hash tables for joins/aggregations). For our workload (simple range scans
   with no joins), DuckDB rarely uses more than 20-40 MB even with a 256 MB
   limit. The limit is a ceiling, not an allocation.

3. The real RSS contribution from DuckDB is typically **30-60% of the configured
   limit** during active queries, dropping to ~10-20 MB at idle (only metadata
   and prepared statement caches remain).

4. With `memory_limit_mb = 128`, the projected total is:
   - CandleBuffers + GPU + iced + DuckDB = 6 + 30 + 40 + 40 = **~116 MB** idle
   - During 20-chart startup query burst: **~160 MB** peak
   - **Within the 200 MB target.**

### Measurement Strategy

After implementation, measure actual RSS with:

```powershell
# Windows: Task Manager or Process Explorer
# Programmatic: in Rust
use sysinfo::{System, SystemExt, ProcessExt};
let mut sys = System::new_all();
sys.refresh_all();
let proc = sys.process(sysinfo::get_current_pid().unwrap()).unwrap();
let rss_mb = proc.memory() / 1024 / 1024;
tracing::info!("RSS: {rss_mb} MB");
```

If RSS exceeds 200 MB with 20 charts, reduce `memory_limit_mb` in config or
profile to find the actual consumer.

---

## 8. Hot Reload

### What Can Be Changed at Runtime

| Setting | Hot-reloadable? | Mechanism |
|---------|----------------|-----------|
| `enabled` | **No** | Requires dropping actor thread and re-creating (or creating from None). App restart required. |
| `path` | **No** | Requires closing current connection and opening new file. App restart required. |
| `memory_limit_mb` | **Yes** | `SET memory_limit = '{X}MB'` is session-scoped and can be re-issued. |
| `threads` | **Yes** | `SET threads = {N}` is session-scoped and can be re-issued. |
| `flush_interval_secs` | **Yes** | Timer interval is configurable in the aggregator. |

### Hot Reload Implementation (Future)

If a settings UI or config file watcher is added:

```rust
impl DbHandle {
    /// Update runtime DuckDB settings without restarting the connection.
    ///
    /// Only `memory_limit` and `threads` can be updated at runtime.
    /// Other settings require a full restart.
    pub async fn update_settings(
        &self,
        memory_limit_mb: u32,
        threads: u8,
    ) -> Result<(), StoreError> {
        self.mb
            .send(DbCommand::UpdateSettings { memory_limit_mb, threads })
            .await
            .map_err(|e| StoreError::ActorDead(e.to_string()))?;
        Ok(())
    }
}
```

The actor thread handler:

```rust
DbCommand::UpdateSettings { memory_limit_mb, threads } => {
    conn.execute_batch(&format!(
        "SET memory_limit = '{}MB'; SET threads = {};",
        memory_limit_mb, threads,
    )).unwrap_or_else(|e| {
        tracing::warn!("Failed to update DuckDB settings: {e}");
    });
    // Reply with success.
    if let Some(ch) = reply_channel {
        ch.blocking_send(DbReply::SettingsUpdated(Ok(()))).ok();
    }
    Some(conn)
}
```

### v1 Recommendation

Do not implement hot reload in v1. Require app restart for config changes.
The settings change infrequently (once during initial tuning, then never).
Hot reload adds complexity (new message variant, actor state management,
edge cases around in-flight queries during settings change) for minimal
user benefit.

---

## 9. Diagnostic Commands

### DuckDB System Tables and PRAGMAs

DuckDB exposes system information through several built-in views and functions.
These are useful for a future debug panel or `--diagnostics` CLI flag.

```rust
/// Diagnostic information about the DuckDB store.
#[derive(Debug, Clone)]
pub struct StoreDiagnostics {
    /// DuckDB version string.
    pub duckdb_version: String,
    /// Configured memory limit in bytes.
    pub memory_limit: u64,
    /// Current memory usage in bytes.
    pub memory_usage: u64,
    /// Number of tables in the database.
    pub table_count: usize,
    /// Per-table row counts and sizes.
    pub table_stats: Vec<TableStats>,
    /// Total database file size in bytes.
    pub file_size: u64,
    /// WAL file size in bytes.
    pub wal_size: u64,
}

#[derive(Debug, Clone)]
pub struct TableStats {
    pub schema_name: String,
    pub table_name: String,
    pub estimated_row_count: u64,
    pub estimated_size_bytes: u64,
}
```

### Queries for Diagnostic Data

```sql
-- DuckDB version
SELECT version();

-- Current memory usage
SELECT * FROM duckdb_memory();

-- Current settings
SELECT name, value FROM duckdb_settings()
WHERE name IN ('memory_limit', 'threads', 'temp_directory', 'wal_autocheckpoint');

-- Table sizes
SELECT
    table_schema,
    table_name,
    estimated_size,
    column_count,
    index_count
FROM duckdb_tables();

-- Cache inventory summary
SELECT
    symbol,
    timeframe_secs,
    candle_count,
    first_ts,
    last_ts,
    source,
    updated_at
FROM meta.data_ranges
ORDER BY symbol, timeframe_secs;

-- Total candle count
SELECT COUNT(*) AS total_candles FROM market.candles;

-- Per-symbol candle counts
SELECT symbol, COUNT(*) AS count
FROM market.candles
GROUP BY symbol
ORDER BY count DESC;

-- Database file size (from Rust, not SQL)
-- std::fs::metadata(db_path).map(|m| m.len())
```

### Future: Debug Panel Integration

The diagnostics can be exposed through a `Message::RequestStoreDiagnostics`
variant that queries the actor and returns results to a debug overlay panel:

```rust
// New message variants:
Message::RequestStoreDiagnostics => {
    if let Some(ref store) = self.store {
        let store = store.clone();
        Task::perform(
            async move { store.diagnostics().await },
            Message::StoreDiagnosticsReceived,
        )
    } else {
        Task::none()
    }
}

Message::StoreDiagnosticsReceived(diag) => {
    // Display in debug overlay panel.
    self.store_diagnostics = Some(diag);
    Task::none()
}
```

### CLI Flag (Future)

```rust
// In main.rs, before starting iced:
if std::env::args().any(|a| a == "--store-info") {
    let config = AppConfig::load(&MidasApp::config_file_path()).unwrap_or_default();
    let store_path = resolve_store_path(&config.store);
    println!("Store path: {}", store_path.display());
    println!("Enabled: {}", config.store.enabled);
    println!("Memory limit: {} MB", config.store.memory_limit_mb);
    println!("Threads: {}", config.store.threads);

    if store_path.exists() {
        let meta = std::fs::metadata(&store_path).unwrap();
        println!("File size: {:.2} MB", meta.len() as f64 / 1_048_576.0);

        // Open read-only and print stats.
        let conn = duckdb::Connection::open_with_flags(
            &store_path,
            duckdb::Config::default(),
        ).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM market.candles", [], |r| r.get(0))
            .unwrap_or(0);
        println!("Total candles: {count}");

        let symbols: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT symbol FROM market.candles ORDER BY symbol"
            ).unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        println!("Symbols: {}", symbols.join(", "));
    } else {
        println!("Database file does not exist yet.");
    }

    std::process::exit(0);
}
```

---

## 10. Test Support

### Test Config Helpers

```rust
#[cfg(test)]
impl StoreConfig {
    /// Create a test config with in-memory DuckDB.
    pub fn test_memory() -> Self {
        Self {
            enabled: true,
            path: ":memory:".into(),
            memory_limit_mb: 64,
            threads: 1,
            flush_interval_secs: 1,
        }
    }

    /// Create a test config pointing to a temp directory.
    pub fn test_file(dir: &Path) -> Self {
        Self {
            enabled: true,
            path: dir.join("test.duckdb").to_string_lossy().into_owned(),
            memory_limit_mb: 64,
            threads: 1,
            flush_interval_secs: 1,
        }
    }

    /// Create a disabled config (for fallback tests).
    pub fn test_disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}
```

### Config Roundtrip Tests

```rust
#[cfg(test)]
mod store_config_tests {
    use super::*;

    #[test]
    fn default_store_config_has_expected_values() {
        let cfg = StoreConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.path, "cache.duckdb");
        assert_eq!(cfg.memory_limit_mb, 256);
        assert_eq!(cfg.threads, 2);
        assert_eq!(cfg.flush_interval_secs, 5);
    }

    #[test]
    fn store_config_roundtrip_through_toml() {
        let cfg = StoreConfig {
            enabled: true,
            path: "custom.duckdb".into(),
            memory_limit_mb: 128,
            threads: 4,
            flush_interval_secs: 10,
        };
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let parsed: StoreConfig = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed.enabled, cfg.enabled);
        assert_eq!(parsed.path, cfg.path);
        assert_eq!(parsed.memory_limit_mb, cfg.memory_limit_mb);
        assert_eq!(parsed.threads, cfg.threads);
        assert_eq!(parsed.flush_interval_secs, cfg.flush_interval_secs);
    }

    #[test]
    fn app_config_without_store_section_uses_defaults() {
        let toml_str = r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"
"#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("parse");
        assert!(cfg.store.enabled);
        assert_eq!(cfg.store.path, "cache.duckdb");
        assert_eq!(cfg.store.memory_limit_mb, 256);
    }

    #[test]
    fn app_config_with_partial_store_section() {
        let toml_str = r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"

[store]
memory_limit_mb = 64
"#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("parse");
        assert!(cfg.store.enabled);  // default
        assert_eq!(cfg.store.path, "cache.duckdb");  // default
        assert_eq!(cfg.store.memory_limit_mb, 64);  // overridden
        assert_eq!(cfg.store.threads, 2);  // default
    }

    #[test]
    fn store_disabled_in_config() {
        let toml_str = r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"

[store]
enabled = false
"#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("parse");
        assert!(!cfg.store.enabled);
    }

    #[test]
    fn validate_clamps_memory_limit() {
        let mut cfg = StoreConfig {
            memory_limit_mb: 10,  // below 32 minimum
            ..Default::default()
        };
        cfg.validate();
        assert_eq!(cfg.memory_limit_mb, 32);

        cfg.memory_limit_mb = 2000;  // above 1024 maximum
        cfg.validate();
        assert_eq!(cfg.memory_limit_mb, 1024);
    }

    #[test]
    fn validate_clamps_threads() {
        let mut cfg = StoreConfig {
            threads: 0,
            ..Default::default()
        };
        cfg.validate();
        assert_eq!(cfg.threads, 1);

        cfg.threads = 16;
        cfg.validate();
        assert_eq!(cfg.threads, 8);
    }

    #[test]
    fn validate_clamps_flush_interval() {
        let mut cfg = StoreConfig {
            flush_interval_secs: 0,
            ..Default::default()
        };
        cfg.validate();
        assert_eq!(cfg.flush_interval_secs, 1);

        cfg.flush_interval_secs = 120;
        cfg.validate();
        assert_eq!(cfg.flush_interval_secs, 60);
    }

    #[test]
    fn validate_rejects_empty_path() {
        let mut cfg = StoreConfig {
            path: "   ".into(),
            ..Default::default()
        };
        cfg.validate();
        assert_eq!(cfg.path, "cache.duckdb");
    }

    #[test]
    fn full_app_config_roundtrip_with_store() {
        let cfg = AppConfig {
            store: StoreConfig {
                enabled: true,
                path: "my_cache.duckdb".into(),
                memory_limit_mb: 128,
                threads: 4,
                flush_interval_secs: 10,
            },
            ..Default::default()
        };
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let parsed: AppConfig = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed.store.path, "my_cache.duckdb");
        assert_eq!(parsed.store.memory_limit_mb, 128);
        assert_eq!(parsed.store.threads, 4);
        assert_eq!(parsed.store.flush_interval_secs, 10);
    }
}
```

---

## 11. Summary: Config-to-Runtime Flow

```
config.toml                          MidasApp::new()                   Runtime
+-----------+                        +------------------+              +----------+
| [store]   |  --deserialize-->      | StoreConfig      |              |          |
| enabled   |                        |   .validate()    |              |          |
| path      |                        |                  |              |          |
| memory_mb |                        | resolve_store_   |              |          |
| threads   |                        |   path()         |              |          |
| flush_s   |                        |        |         |              |          |
+-----------+                        |        v         |              |          |
                                     | DbHandle::open() |              |          |
                                     |   |              |              |          |
                                     |   +-- Ok ------->| store = Some |          |
                                     |   |              |              |          |
                                     |   +-- Err ------>| store = None |          |
                                     |                  |  warn!()     |          |
                                     +------------------+              |          |
                                                                       |          |
  On symbol change:                                                    |          |
    store.is_some()?                                                   |          |
      YES -> query DuckDB -> hit/miss -> DataLoaded / DataCacheMiss   |          |
      NO  -> load_test_data_for_chart() (sync)                        |          |
                                                                       +----------+
```
