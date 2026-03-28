//! Application configuration types and persistence.
//!
//! `AppConfig` is the root configuration struct, serialized as TOML.
//! It stores window geometry, theme preferences, and per-chart state
//! (symbol, timeframe, horizontal levels, camera position, gap collapsing)
//! so the workspace can be restored across sessions.
//!
//! Writes use atomic file replacement via `tempfile::NamedTempFile` to
//! prevent corruption if the process is interrupted mid-save.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Error type ───────────────────────────────────────────────────────

/// Errors that can occur during config load/save operations.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to read or write the config file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse the TOML content.
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    /// Failed to serialize config to TOML.
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

// ── Config structs ───────────────────────────────────────────────────

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
}

/// Window geometry configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Window width in logical pixels.
    pub width: u32,
    /// Window height in logical pixels.
    pub height: u32,
    /// Whether the window is maximized.
    #[serde(default)]
    pub maximized: bool,
}

/// Theme configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Theme mode: `"dark"` or `"light"`.
    pub mode: String,
}

/// Per-chart configuration for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartConfig {
    /// Ticker symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// Timeframe display name (e.g. `"1D"`, `"5m"`).
    pub timeframe: String,
    /// User-defined horizontal price levels on this chart.
    #[serde(default)]
    pub levels: Vec<LevelConfig>,
    /// Camera time-axis start (restored on load so user sees same view).
    #[serde(default)]
    pub camera_time_start: Option<f64>,
    /// Camera time-axis end.
    #[serde(default)]
    pub camera_time_end: Option<f64>,
    /// Camera price-axis low.
    #[serde(default)]
    pub camera_price_low: Option<f64>,
    /// Camera price-axis high.
    #[serde(default)]
    pub camera_price_high: Option<f64>,
    /// Whether session gaps are collapsed (index-based X positioning).
    #[serde(default)]
    pub collapse_gaps: bool,
}

/// Persisted horizontal price level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelConfig {
    /// Price at which the level is drawn.
    pub price: f64,
    /// RGBA color in linear space.
    pub color: [f32; 4],
    /// Line width in logical pixels.
    #[serde(default = "default_line_width")]
    pub line_width: f32,
}

/// Default line width for levels missing the field (backward compat).
fn default_line_width() -> f32 {
    1.0
}

// ── Default implementation ───────────────────────────────────────────

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig {
                width: 1280,
                height: 800,
                maximized: false,
            },
            theme: ThemeConfig {
                mode: "dark".into(),
            },
            charts: Vec::new(),
        }
    }
}

// ── Load / Save ──────────────────────────────────────────────────────

impl AppConfig {
    /// Load configuration from a TOML file at `path`.
    ///
    /// Returns `AppConfig::default()` if the file does not exist or is empty.
    /// Returns an error only for genuine I/O or parse failures (e.g. permission
    /// denied, malformed TOML).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("Config file not found at {}, using defaults", path.display());
                return Ok(Self::default());
            }
            Err(e) => return Err(ConfigError::Io(e)),
        };

        if content.trim().is_empty() {
            tracing::info!("Config file at {} is empty, using defaults", path.display());
            return Ok(Self::default());
        }

        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Serialize this configuration and atomically write it to `path`.
    ///
    /// Creates parent directories if they do not exist. Uses a temporary
    /// file in the same directory followed by an atomic rename so that a
    /// crash mid-write never leaves a truncated config file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        let parent = path.parent().unwrap_or(std::path::Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(&mut tmp, content.as_bytes())?;
        tmp.persist(path).map_err(std::io::Error::from)?;
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Monotonic counter to ensure each test gets a unique temp directory.
    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Helper to create a unique temp directory for each test.
    fn temp_dir() -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "midas_config_test_{}_{id}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Clean up a temp directory after a test.
    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn default_produces_valid_toml() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).expect("serialize default config");
        assert!(!toml_str.is_empty());
        // Round-trip through TOML parser.
        let _parsed: AppConfig =
            toml::from_str(&toml_str).expect("parse serialized default config");
    }

    #[test]
    fn default_has_expected_values() {
        let config = AppConfig::default();
        assert_eq!(config.window.width, 1280);
        assert_eq!(config.window.height, 800);
        assert!(!config.window.maximized);
        assert_eq!(config.theme.mode, "dark");
        assert!(config.charts.is_empty());
    }

    #[test]
    fn save_load_roundtrip_preserves_all_fields() {
        let dir = temp_dir();
        let path = dir.join("roundtrip.toml");

        let config = AppConfig {
            window: WindowConfig {
                width: 1920,
                height: 1080,
                maximized: true,
            },
            theme: ThemeConfig {
                mode: "light".into(),
            },
            charts: vec![ChartConfig {
                symbol: "MSFT".into(),
                timeframe: "4H".into(),
                levels: vec![
                    LevelConfig {
                        price: 420.50,
                        color: [1.0, 0.0, 0.0, 1.0],
                        line_width: 2.0,
                    },
                    LevelConfig {
                        price: 380.25,
                        color: [0.0, 1.0, 0.5, 0.8],
                        line_width: 1.5,
                    },
                ],
                camera_time_start: Some(1_000_000.0),
                camera_time_end: Some(2_000_000.0),
                camera_price_low: Some(350.0),
                camera_price_high: Some(450.0),
                collapse_gaps: true,
            }],
        };

        config.save(&path).expect("save config");
        let loaded = AppConfig::load(&path).expect("load config");

        assert_eq!(loaded.window.width, 1920);
        assert_eq!(loaded.window.height, 1080);
        assert!(loaded.window.maximized);
        assert_eq!(loaded.theme.mode, "light");
        assert_eq!(loaded.charts.len(), 1);
        assert_eq!(loaded.charts[0].symbol, "MSFT");
        assert_eq!(loaded.charts[0].timeframe, "4H");
        assert_eq!(loaded.charts[0].levels.len(), 2);
        assert!((loaded.charts[0].levels[0].price - 420.50).abs() < f64::EPSILON);
        assert_eq!(loaded.charts[0].levels[0].color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(loaded.charts[0].levels[0].line_width, 2.0);
        assert!((loaded.charts[0].levels[1].price - 380.25).abs() < f64::EPSILON);
        assert_eq!(loaded.charts[0].levels[1].color, [0.0, 1.0, 0.5, 0.8]);
        assert_eq!(loaded.charts[0].levels[1].line_width, 1.5);
        // Camera fields
        assert_eq!(loaded.charts[0].camera_time_start, Some(1_000_000.0));
        assert_eq!(loaded.charts[0].camera_time_end, Some(2_000_000.0));
        assert_eq!(loaded.charts[0].camera_price_low, Some(350.0));
        assert_eq!(loaded.charts[0].camera_price_high, Some(450.0));
        assert!(loaded.charts[0].collapse_gaps);

        cleanup(&dir);
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let dir = temp_dir();
        let path = dir.join("nonexistent.toml");

        let config = AppConfig::load(&path).expect("load from missing file");
        assert_eq!(config.window.width, 1280);
        assert_eq!(config.window.height, 800);
        assert_eq!(config.theme.mode, "dark");
        assert!(config.charts.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn load_empty_file_returns_defaults() {
        let dir = temp_dir();
        let path = dir.join("empty.toml");

        // Create an empty file.
        std::fs::File::create(&path).expect("create empty file");

        let config = AppConfig::load(&path).expect("load from empty file");
        assert_eq!(config.window.width, 1280);
        assert_eq!(config.window.height, 800);
        assert_eq!(config.theme.mode, "dark");
        assert!(config.charts.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn load_whitespace_only_file_returns_defaults() {
        let dir = temp_dir();
        let path = dir.join("whitespace.toml");

        let mut f = std::fs::File::create(&path).expect("create file");
        f.write_all(b"   \n  \n  ").expect("write whitespace");

        let config = AppConfig::load(&path).expect("load from whitespace file");
        assert_eq!(config.window.width, 1280);
        assert!(config.charts.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn chart_config_with_levels_survives_roundtrip() {
        let dir = temp_dir();
        let path = dir.join("levels.toml");

        let config = AppConfig {
            window: WindowConfig {
                width: 1280,
                height: 800,
                maximized: false,
            },
            theme: ThemeConfig {
                mode: "dark".into(),
            },
            charts: vec![
                ChartConfig {
                    symbol: "AAPL".into(),
                    timeframe: "1D".into(),
                    levels: vec![
                        LevelConfig {
                            price: 150.0,
                            color: [1.0, 0.843, 0.0, 1.0],
                            line_width: 1.0,
                        },
                        LevelConfig {
                            price: 175.50,
                            color: [0.0, 1.0, 0.0, 1.0],
                            line_width: 3.0,
                        },
                    ],
                    camera_time_start: None,
                    camera_time_end: None,
                    camera_price_low: None,
                    camera_price_high: None,
                    collapse_gaps: false,
                },
                ChartConfig {
                    symbol: "TSLA".into(),
                    timeframe: "5m".into(),
                    levels: vec![],
                    camera_time_start: Some(500.0),
                    camera_time_end: Some(1500.0),
                    camera_price_low: Some(100.0),
                    camera_price_high: Some(300.0),
                    collapse_gaps: true,
                },
            ],
        };

        config.save(&path).expect("save config");
        let loaded = AppConfig::load(&path).expect("load config");

        assert_eq!(loaded.charts.len(), 2);

        // First chart with levels.
        assert_eq!(loaded.charts[0].symbol, "AAPL");
        assert_eq!(loaded.charts[0].timeframe, "1D");
        assert_eq!(loaded.charts[0].levels.len(), 2);
        assert!((loaded.charts[0].levels[0].price - 150.0).abs() < f64::EPSILON);
        assert_eq!(loaded.charts[0].levels[0].line_width, 1.0);
        assert!((loaded.charts[0].levels[1].price - 175.50).abs() < f64::EPSILON);
        assert_eq!(loaded.charts[0].levels[1].line_width, 3.0);
        assert!(!loaded.charts[0].collapse_gaps);
        assert_eq!(loaded.charts[0].camera_time_start, None);

        // Second chart without levels, with camera.
        assert_eq!(loaded.charts[1].symbol, "TSLA");
        assert_eq!(loaded.charts[1].timeframe, "5m");
        assert!(loaded.charts[1].levels.is_empty());
        assert!(loaded.charts[1].collapse_gaps);
        assert_eq!(loaded.charts[1].camera_time_start, Some(500.0));
        assert_eq!(loaded.charts[1].camera_time_end, Some(1500.0));
        assert_eq!(loaded.charts[1].camera_price_low, Some(100.0));
        assert_eq!(loaded.charts[1].camera_price_high, Some(300.0));

        cleanup(&dir);
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = temp_dir();
        let path = dir.join("nested").join("deep").join("config.toml");

        let config = AppConfig::default();
        config.save(&path).expect("save with nested dirs");
        assert!(path.exists());

        cleanup(&dir);
    }

    #[test]
    fn load_malformed_toml_returns_error() {
        let dir = temp_dir();
        let path = dir.join("malformed.toml");

        std::fs::write(&path, "this is not [valid toml = ").expect("write malformed");

        let result = AppConfig::load(&path);
        assert!(result.is_err());

        cleanup(&dir);
    }

    #[test]
    fn partial_toml_uses_defaults_for_missing_fields() {
        let dir = temp_dir();
        let path = dir.join("partial.toml");

        // Only specify window and theme, no charts.
        std::fs::write(
            &path,
            r#"
[window]
width = 800
height = 600

[theme]
mode = "light"
"#,
        )
        .expect("write partial config");

        let config = AppConfig::load(&path).expect("load partial config");
        assert_eq!(config.window.width, 800);
        assert_eq!(config.window.height, 600);
        assert!(!config.window.maximized);
        assert_eq!(config.theme.mode, "light");
        // charts has #[serde(default)], so missing = empty vec.
        assert!(config.charts.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn backward_compat_old_config_without_new_fields_loads_with_defaults() {
        let dir = temp_dir();
        let path = dir.join("old_format.toml");

        // Simulate an old config without camera, collapse_gaps, line_width,
        // or maximized fields.
        std::fs::write(
            &path,
            r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"

[[charts]]
symbol = "AAPL"
timeframe = "1D"

[[charts.levels]]
price = 150.0
color = [1.0, 0.843, 0.0, 1.0]
"#,
        )
        .expect("write old-format config");

        let config = AppConfig::load(&path).expect("load old-format config");
        assert_eq!(config.window.width, 1280);
        assert!(!config.window.maximized); // default false
        assert_eq!(config.charts.len(), 1);
        assert_eq!(config.charts[0].symbol, "AAPL");
        // New camera fields default to None.
        assert_eq!(config.charts[0].camera_time_start, None);
        assert_eq!(config.charts[0].camera_time_end, None);
        assert_eq!(config.charts[0].camera_price_low, None);
        assert_eq!(config.charts[0].camera_price_high, None);
        // collapse_gaps defaults to false.
        assert!(!config.charts[0].collapse_gaps);
        // line_width defaults to 1.0.
        assert_eq!(config.charts[0].levels[0].line_width, 1.0);

        cleanup(&dir);
    }

    #[test]
    fn atomic_write_does_not_corrupt_on_success() {
        let dir = temp_dir();
        let path = dir.join("atomic.toml");

        let config = AppConfig {
            window: WindowConfig {
                width: 1600,
                height: 900,
                maximized: false,
            },
            theme: ThemeConfig {
                mode: "dark".into(),
            },
            charts: vec![ChartConfig {
                symbol: "GOOG".into(),
                timeframe: "1H".into(),
                levels: vec![],
                camera_time_start: Some(100.0),
                camera_time_end: Some(200.0),
                camera_price_low: Some(50.0),
                camera_price_high: Some(150.0),
                collapse_gaps: false,
            }],
        };

        // Write multiple times to ensure atomic replacement works.
        for _ in 0..5 {
            config.save(&path).expect("atomic save");
        }

        // Verify the file is valid after repeated writes.
        let loaded = AppConfig::load(&path).expect("load after atomic writes");
        assert_eq!(loaded.window.width, 1600);
        assert_eq!(loaded.charts.len(), 1);
        assert_eq!(loaded.charts[0].symbol, "GOOG");
        assert_eq!(loaded.charts[0].camera_time_start, Some(100.0));

        // Verify no stale temp files remain.
        let temp_files: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name != "atomic.toml"
            })
            .collect();
        assert!(
            temp_files.is_empty(),
            "stale temp files remain: {:?}",
            temp_files
                .iter()
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        );

        cleanup(&dir);
    }

    #[test]
    fn roundtrip_with_camera_and_collapse_gaps_and_line_width() {
        let dir = temp_dir();
        let path = dir.join("full_roundtrip.toml");

        let config = AppConfig {
            window: WindowConfig {
                width: 2560,
                height: 1440,
                maximized: true,
            },
            theme: ThemeConfig {
                mode: "dark".into(),
            },
            charts: vec![
                ChartConfig {
                    symbol: "SPY".into(),
                    timeframe: "5m".into(),
                    levels: vec![
                        LevelConfig {
                            price: 500.0,
                            color: [1.0, 0.0, 0.0, 1.0],
                            line_width: 2.5,
                        },
                        LevelConfig {
                            price: 480.0,
                            color: [0.0, 1.0, 0.0, 0.7],
                            line_width: 0.5,
                        },
                    ],
                    camera_time_start: Some(1_700_000_000.0),
                    camera_time_end: Some(1_700_100_000.0),
                    camera_price_low: Some(470.0),
                    camera_price_high: Some(510.0),
                    collapse_gaps: true,
                },
                ChartConfig {
                    symbol: "QQQ".into(),
                    timeframe: "1D".into(),
                    levels: vec![],
                    camera_time_start: None,
                    camera_time_end: None,
                    camera_price_low: None,
                    camera_price_high: None,
                    collapse_gaps: false,
                },
            ],
        };

        config.save(&path).expect("save full config");
        let loaded = AppConfig::load(&path).expect("load full config");

        // Window
        assert_eq!(loaded.window.width, 2560);
        assert_eq!(loaded.window.height, 1440);
        assert!(loaded.window.maximized);

        // First chart: all fields populated
        let c0 = &loaded.charts[0];
        assert_eq!(c0.symbol, "SPY");
        assert_eq!(c0.timeframe, "5m");
        assert!(c0.collapse_gaps);
        assert_eq!(c0.camera_time_start, Some(1_700_000_000.0));
        assert_eq!(c0.camera_time_end, Some(1_700_100_000.0));
        assert_eq!(c0.camera_price_low, Some(470.0));
        assert_eq!(c0.camera_price_high, Some(510.0));
        assert_eq!(c0.levels.len(), 2);
        assert_eq!(c0.levels[0].line_width, 2.5);
        assert_eq!(c0.levels[1].line_width, 0.5);

        // Second chart: no camera, no levels
        let c1 = &loaded.charts[1];
        assert_eq!(c1.symbol, "QQQ");
        assert!(!c1.collapse_gaps);
        assert_eq!(c1.camera_time_start, None);
        assert!(c1.levels.is_empty());

        cleanup(&dir);
    }
}
