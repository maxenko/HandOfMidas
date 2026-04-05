use super::*;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

/// Monotonic counter to ensure each test gets a unique temp directory.
static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Helper to create a unique temp directory for each test.
fn temp_dir() -> std::path::PathBuf {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("midas_config_test_{}_{id}", std::process::id()));
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

    let mut msft_levels = HashMap::new();
    msft_levels.insert(
        "MSFT".into(),
        vec![
            LevelConfig {
                price: 420.50,
                color: [1.0, 0.0, 0.0, 1.0],
                line_width: 2.0,
                label: None,
                icon: "none".into(),
                locked: false,
            },
            LevelConfig {
                price: 380.25,
                color: [0.0, 1.0, 0.5, 0.8],
                line_width: 1.5,
                label: None,
                icon: "none".into(),
                locked: false,
            },
        ],
    );
    let config = AppConfig {
        window: WindowConfig {
            width: 1920,
            height: 1080,
            maximized: true,
            ..Default::default()
        },
        theme: ThemeConfig {
            mode: "light".into(),
        },
        charts: vec![ChartConfig {
            symbol: "MSFT".into(),
            timeframe: "4H".into(),
            levels: vec![],
            camera_time_start: Some(1_000_000.0),
            camera_time_end: Some(2_000_000.0),
            camera_price_low: Some(350.0),
            camera_price_high: Some(450.0),
            collapse_gaps: true,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
        }],
        levels: msft_levels,
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
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
    // Levels are in the top-level map, not per-chart.
    assert_eq!(loaded.levels["MSFT"].len(), 2);
    assert!((loaded.levels["MSFT"][0].price - 420.50).abs() < f64::EPSILON);
    assert_eq!(loaded.levels["MSFT"][0].color, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(loaded.levels["MSFT"][0].line_width, 2.0);
    assert!((loaded.levels["MSFT"][1].price - 380.25).abs() < f64::EPSILON);
    assert_eq!(loaded.levels["MSFT"][1].color, [0.0, 1.0, 0.5, 0.8]);
    assert_eq!(loaded.levels["MSFT"][1].line_width, 1.5);
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

    let mut aapl_levels = HashMap::new();
    aapl_levels.insert(
        "AAPL".into(),
        vec![
            LevelConfig {
                price: 150.0,
                color: [1.0, 0.843, 0.0, 1.0],
                line_width: 1.0,
                label: None,
                icon: "none".into(),
                locked: false,
            },
            LevelConfig {
                price: 175.50,
                color: [0.0, 1.0, 0.0, 1.0],
                line_width: 3.0,
                label: None,
                icon: "none".into(),
                locked: false,
            },
        ],
    );
    let config = AppConfig {
        window: WindowConfig {
            width: 1280,
            height: 800,
            ..Default::default()
        },
        theme: ThemeConfig {
            mode: "dark".into(),
        },
        charts: vec![
            ChartConfig {
                symbol: "AAPL".into(),
                timeframe: "1D".into(),
                levels: vec![],
                camera_time_start: None,
                camera_time_end: None,
                camera_price_low: None,
                camera_price_high: None,
                collapse_gaps: false,
                timeline_border_ratio: 0.20,
                volume_scale: 1.0,
                show_volume_profile: false,
                show_levels: true,
                viewport_width: None,
                viewport_height: None,
                symbol_link: LinkMode::default(),
                timeframe_link: LinkMode::default(),
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
                timeline_border_ratio: 0.20,
                volume_scale: 1.0,
                show_volume_profile: false,
                show_levels: true,
                viewport_width: None,
                viewport_height: None,
                symbol_link: LinkMode::default(),
                timeframe_link: LinkMode::default(),
            },
        ],
        levels: aapl_levels,
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
    };

    config.save(&path).expect("save config");
    let loaded = AppConfig::load(&path).expect("load config");

    assert_eq!(loaded.charts.len(), 2);

    // First chart (no per-chart levels anymore).
    assert_eq!(loaded.charts[0].symbol, "AAPL");
    assert_eq!(loaded.charts[0].timeframe, "1D");
    assert!(loaded.charts[0].levels.is_empty());
    assert!(!loaded.charts[0].collapse_gaps);
    assert_eq!(loaded.charts[0].camera_time_start, None);

    // Levels are in the top-level map.
    assert_eq!(loaded.levels["AAPL"].len(), 2);
    assert!((loaded.levels["AAPL"][0].price - 150.0).abs() < f64::EPSILON);
    assert_eq!(loaded.levels["AAPL"][0].line_width, 1.0);
    assert!((loaded.levels["AAPL"][1].price - 175.50).abs() < f64::EPSILON);
    assert_eq!(loaded.levels["AAPL"][1].line_width, 3.0);

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
    // Link modes default to Unlinked.
    assert_eq!(config.charts[0].symbol_link, LinkMode::Unlinked);
    assert_eq!(config.charts[0].timeframe_link, LinkMode::Unlinked);

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
            ..Default::default()
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
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
        }],
        levels: HashMap::new(),
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
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
        temp_files.iter().map(|e| e.file_name()).collect::<Vec<_>>()
    );

    cleanup(&dir);
}

#[test]
fn roundtrip_with_camera_and_collapse_gaps_and_line_width() {
    let dir = temp_dir();
    let path = dir.join("full_roundtrip.toml");

    let mut spy_levels = HashMap::new();
    spy_levels.insert(
        "SPY".into(),
        vec![
            LevelConfig {
                price: 500.0,
                color: [1.0, 0.0, 0.0, 1.0],
                line_width: 2.5,
                label: None,
                icon: "none".into(),
                locked: false,
            },
            LevelConfig {
                price: 480.0,
                color: [0.0, 1.0, 0.0, 0.7],
                line_width: 0.5,
                label: None,
                icon: "none".into(),
                locked: false,
            },
        ],
    );
    let config = AppConfig {
        window: WindowConfig {
            width: 2560,
            height: 1440,
            maximized: true,
            ..Default::default()
        },
        theme: ThemeConfig {
            mode: "dark".into(),
        },
        charts: vec![
            ChartConfig {
                symbol: "SPY".into(),
                timeframe: "5m".into(),
                levels: vec![],
                camera_time_start: Some(1_700_000_000.0),
                camera_time_end: Some(1_700_100_000.0),
                camera_price_low: Some(470.0),
                camera_price_high: Some(510.0),
                collapse_gaps: true,
                timeline_border_ratio: 0.20,
                volume_scale: 1.0,
                show_volume_profile: false,
                show_levels: true,
                viewport_width: None,
                viewport_height: None,
                symbol_link: LinkMode::default(),
                timeframe_link: LinkMode::default(),
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
                timeline_border_ratio: 0.20,
                volume_scale: 1.0,
                show_volume_profile: false,
                show_levels: true,
                viewport_width: None,
                viewport_height: None,
                symbol_link: LinkMode::default(),
                timeframe_link: LinkMode::default(),
            },
        ],
        levels: spy_levels,
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
    };

    config.save(&path).expect("save full config");
    let loaded = AppConfig::load(&path).expect("load full config");

    // Window
    assert_eq!(loaded.window.width, 2560);
    assert_eq!(loaded.window.height, 1440);
    assert!(loaded.window.maximized);

    // First chart: camera fields populated
    let c0 = &loaded.charts[0];
    assert_eq!(c0.symbol, "SPY");
    assert_eq!(c0.timeframe, "5m");
    assert!(c0.collapse_gaps);
    assert_eq!(c0.camera_time_start, Some(1_700_000_000.0));
    assert_eq!(c0.camera_time_end, Some(1_700_100_000.0));
    assert_eq!(c0.camera_price_low, Some(470.0));
    assert_eq!(c0.camera_price_high, Some(510.0));
    // Levels are in top-level map, not per-chart.
    assert_eq!(loaded.levels["SPY"].len(), 2);
    assert_eq!(loaded.levels["SPY"][0].line_width, 2.5);
    assert_eq!(loaded.levels["SPY"][1].line_width, 0.5);

    // Second chart: no camera, no levels
    let c1 = &loaded.charts[1];
    assert_eq!(c1.symbol, "QQQ");
    assert!(!c1.collapse_gaps);
    assert_eq!(c1.camera_time_start, None);
    assert!(!loaded.levels.contains_key("QQQ"));

    cleanup(&dir);
}

#[test]
fn migrate_levels_from_old_per_chart_format() {
    let dir = temp_dir();
    let path = dir.join("old_levels.toml");

    // Old format: levels are inside [[charts]].
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
price = 185.50
color = [0.0, 0.8, 0.0, 1.0]
line_width = 1.0
icon = "arrow_up"
locked = false

[[charts]]
symbol = "AAPL"
timeframe = "5m"

[[charts.levels]]
price = 185.50
color = [0.0, 0.8, 0.0, 1.0]
line_width = 1.0
icon = "arrow_up"
locked = false

[[charts.levels]]
price = 192.30
color = [0.85, 0.85, 0.85, 0.8]
line_width = 1.0
icon = "none"
locked = false
"#,
    )
    .expect("write old format");

    let config = AppConfig::load(&path).expect("load old format");

    // Levels should have been migrated to top-level map.
    assert!(config.levels.contains_key("AAPL"));
    let aapl = &config.levels["AAPL"];
    // 185.50 appears in both charts but should be deduplicated.
    assert_eq!(aapl.len(), 2); // 185.50 + 192.30
    assert!((aapl[0].price - 185.50).abs() < 0.001);
    assert!((aapl[1].price - 192.30).abs() < 0.001);

    cleanup(&dir);
}

#[test]
fn new_format_levels_round_trip() {
    let dir = temp_dir();
    let path = dir.join("new_levels.toml");

    let mut levels = HashMap::new();
    levels.insert(
        "AAPL".into(),
        vec![LevelConfig {
            price: 185.50,
            color: [0.0, 0.8, 0.0, 1.0],
            line_width: 1.0,
            label: Some("Support".into()),
            icon: "arrow_up".into(),
            locked: true,
        }],
    );

    let config = AppConfig {
        levels,
        ..Default::default()
    };
    config.save(&path).expect("save new format");

    let loaded = AppConfig::load(&path).expect("load new format");
    assert_eq!(loaded.levels.len(), 1);
    assert!(loaded.levels.contains_key("AAPL"));
    let aapl = &loaded.levels["AAPL"];
    assert_eq!(aapl.len(), 1);
    assert!((aapl[0].price - 185.50).abs() < f64::EPSILON);
    assert_eq!(aapl[0].label.as_deref(), Some("Support"));
    assert_eq!(aapl[0].icon, "arrow_up");
    assert!(aapl[0].locked);

    cleanup(&dir);
}

#[test]
fn migration_skipped_when_new_format_already_has_levels() {
    let dir = temp_dir();
    let path = dir.join("already_migrated.toml");

    // Config with both old per-chart levels and new top-level levels.
    // Migration should be skipped because top-level levels exist.
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
price = 999.0
color = [1.0, 0.0, 0.0, 1.0]
line_width = 1.0
icon = "none"
locked = false

[[levels.AAPL]]
price = 185.50
color = [0.0, 0.8, 0.0, 1.0]
line_width = 1.0
icon = "arrow_up"
locked = false
"#,
    )
    .expect("write mixed format");

    let config = AppConfig::load(&path).expect("load mixed format");
    // Top-level levels should be preserved (not overwritten by migration).
    assert_eq!(config.levels["AAPL"].len(), 1);
    assert!((config.levels["AAPL"][0].price - 185.50).abs() < 0.001);

    cleanup(&dir);
}

#[test]
fn chart_config_levels_not_serialized() {
    let dir = temp_dir();
    let path = dir.join("no_chart_levels.toml");

    let config = AppConfig {
        charts: vec![ChartConfig {
            symbol: "AAPL".into(),
            timeframe: "1D".into(),
            levels: vec![LevelConfig {
                price: 100.0,
                color: [1.0, 0.0, 0.0, 1.0],
                line_width: 1.0,
                label: None,
                icon: "none".into(),
                locked: false,
            }],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
        }],
        ..Default::default()
    };

    config.save(&path).expect("save");
    // Read raw TOML to verify no per-chart levels serialized.
    let raw = std::fs::read_to_string(&path).expect("read");
    assert!(
        !raw.contains("charts.levels"),
        "per-chart levels should not be serialized"
    );

    cleanup(&dir);
}

#[test]
fn watchlist_config_roundtrip() {
    let dir = temp_dir();
    let path = dir.join("watchlist.toml");

    let config = AppConfig {
        watchlists: vec![WatchlistConfig {
            name: "Main".into(),
            tickers: vec![
                WatchlistTickerConfig {
                    symbol: "AAPL".into(),
                    favorite: true,
                },
                WatchlistTickerConfig {
                    symbol: "MSFT".into(),
                    favorite: false,
                },
            ],
            symbol_link: LinkMode::default(),
            column_widths: vec![],
        }],
        panel_order: vec![
            PanelSlot::Chart { chart_index: 0 },
            PanelSlot::Watchlist {
                watchlist_index: 0,
            },
        ],
        ..Default::default()
    };

    config.save(&path).expect("save watchlist config");
    let loaded = AppConfig::load(&path).expect("load watchlist config");

    assert_eq!(loaded.watchlists.len(), 1);
    assert_eq!(loaded.watchlists[0].name, "Main");
    assert_eq!(loaded.watchlists[0].tickers.len(), 2);
    assert_eq!(loaded.watchlists[0].tickers[0].symbol, "AAPL");
    assert!(loaded.watchlists[0].tickers[0].favorite);
    assert_eq!(loaded.watchlists[0].tickers[1].symbol, "MSFT");
    assert!(!loaded.watchlists[0].tickers[1].favorite);

    assert_eq!(loaded.panel_order.len(), 2);
    match &loaded.panel_order[0] {
        PanelSlot::Chart { chart_index } => assert_eq!(*chart_index, 0),
        _ => panic!("expected Chart"),
    }
    match &loaded.panel_order[1] {
        PanelSlot::Watchlist { watchlist_index } => assert_eq!(*watchlist_index, 0),
        _ => panic!("expected Watchlist"),
    }

    cleanup(&dir);
}

#[test]
fn old_config_without_watchlist_fields_loads_with_defaults() {
    let dir = temp_dir();
    let path = dir.join("no_watchlists.toml");

    std::fs::write(
        &path,
        r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"
"#,
    )
    .expect("write config without watchlist fields");

    let config = AppConfig::load(&path).expect("load old config");
    assert!(config.watchlists.is_empty());
    assert!(config.panel_order.is_empty());

    cleanup(&dir);
}

#[test]
fn non_default_link_modes_roundtrip() {
    use crate::link::LinkColor;

    let dir = temp_dir();
    let path = dir.join("link_modes.toml");

    let config = AppConfig {
        charts: vec![ChartConfig {
            symbol: "AAPL".into(),
            timeframe: "D1".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::Color(LinkColor::Blue),
            timeframe_link: LinkMode::ListenAll,
        }],
        ..Default::default()
    };

    config.save(&path).expect("save");
    let loaded = AppConfig::load(&path).expect("load");

    assert_eq!(loaded.charts[0].symbol_link, LinkMode::Color(LinkColor::Blue));
    assert_eq!(loaded.charts[0].timeframe_link, LinkMode::ListenAll);

    // Verify the TOML contains the flat string representation.
    let raw = std::fs::read_to_string(&path).expect("read");
    assert!(raw.contains("symbol_link = \"blue\""), "expected flat string, got:\n{raw}");
    assert!(
        raw.contains("timeframe_link = \"listen_all\""),
        "expected flat string, got:\n{raw}"
    );

    cleanup(&dir);
}
