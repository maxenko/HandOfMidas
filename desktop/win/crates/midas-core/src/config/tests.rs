use super::*;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

/// Monotonic counter to ensure each test gets a unique temp directory.
static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Helper to create a unique temp directory for each test.
fn temp_dir() -> std::path::PathBuf {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("midas_config_test_{}_{id}", std::process::id()));
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
    let _parsed: AppConfig = toml::from_str(&toml_str).expect("parse serialized default config");
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
fn default_broker_backend_is_sim() {
    // The whole point of the sim-default experience: a fresh install
    // (no config.toml) must boot into Sim so `cargo run -p midas-app`
    // works without editing any files.
    let config = AppConfig::default();
    assert_eq!(config.broker.backend, BrokerBackend::Sim);
    assert_eq!(config.broker.host, "127.0.0.1");
    assert_eq!(config.broker.port, 7498);
    assert!(!config.broker.allow_live);
}

#[test]
fn broker_backend_serde_roundtrip() {
    // Each variant's TOML tag is part of the stable config schema —
    // renaming them breaks user configs. Pin them explicitly.
    //
    // TOML can't serialize a bare enum value at the top level, so
    // exercise the tag through a wrapper struct (the same shape as
    // it appears inside `BrokerConnectionConfig`).
    #[derive(Serialize, Deserialize)]
    struct Wrapper {
        backend: BrokerBackend,
    }
    for (backend, tag_line) in [
        (BrokerBackend::Sim, "backend = \"sim\""),
        (BrokerBackend::LivePaper, "backend = \"live_paper\""),
        (BrokerBackend::Live, "backend = \"live\""),
    ] {
        let ser = toml::to_string(&Wrapper { backend }).unwrap();
        assert!(
            ser.trim() == tag_line,
            "expected {tag_line} for {backend:?}, got {ser:?}"
        );
        let de: Wrapper = toml::from_str(&ser).unwrap();
        assert_eq!(de.backend, backend, "roundtrip for {backend:?}");
    }
}

#[test]
fn missing_broker_table_defaults_to_sim() {
    // A config written by an older midas-app (before the Sim-default
    // work) has no `[broker]` section. Loading it must still yield
    // `backend = Sim` so existing users get the default experience
    // without editing their config.
    let toml_str = r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"
"#;
    let config: AppConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.broker.backend, BrokerBackend::Sim);
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
        version: crate::config::CURRENT_CONFIG_VERSION,
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
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        }],
        levels: msft_levels,
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        order_blotters: Vec::new(),
        account_panels: Vec::new(),
        recent_symbols: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
        broker: BrokerConnectionConfig::default(),
        chart_view_store_schema: 0,
        experimental: ExperimentalFlags::default(),
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
        version: crate::config::CURRENT_CONFIG_VERSION,
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
                bound_symbol: None,
                backend: None,
                show_extended_hours: true,
                show_extended_hours_bands: true,
                volume_profile: VolumeProfileSettings::default(),
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
                bound_symbol: None,
                backend: None,
                show_extended_hours: true,
                show_extended_hours_bands: true,
                volume_profile: VolumeProfileSettings::default(),
            },
        ],
        levels: aapl_levels,
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        order_blotters: Vec::new(),
        account_panels: Vec::new(),
        recent_symbols: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
        broker: BrokerConnectionConfig::default(),
        chart_view_store_schema: 0,
        experimental: ExperimentalFlags::default(),
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
        version: crate::config::CURRENT_CONFIG_VERSION,
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
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        }],
        levels: HashMap::new(),
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        order_blotters: Vec::new(),
        account_panels: Vec::new(),
        recent_symbols: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
        broker: BrokerConnectionConfig::default(),
        chart_view_store_schema: 0,
        experimental: ExperimentalFlags::default(),
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
        version: crate::config::CURRENT_CONFIG_VERSION,
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
                bound_symbol: None,
                backend: None,
                show_extended_hours: true,
                show_extended_hours_bands: true,
                volume_profile: VolumeProfileSettings::default(),
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
                bound_symbol: None,
                backend: None,
                show_extended_hours: true,
                show_extended_hours_bands: true,
                volume_profile: VolumeProfileSettings::default(),
            },
        ],
        levels: spy_levels,
        watchlists: Vec::new(),
        order_panels: Vec::new(),
        order_blotters: Vec::new(),
        account_panels: Vec::new(),
        recent_symbols: Vec::new(),
        panel_order: Vec::new(),
        layout_tree: Vec::new(),
        store: StoreConfig::default(),
        providers: None,
        broker: BrokerConnectionConfig::default(),
        chart_view_store_schema: 0,
        experimental: ExperimentalFlags::default(),
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
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
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
                    favorite: 1,
                },
                WatchlistTickerConfig {
                    symbol: "MSFT".into(),
                    favorite: 0,
                },
            ],
            symbol_link: LinkMode::default(),
            column_widths: vec![],
        }],
        panel_order: vec![
            PanelSlot::Chart { chart_index: 0 },
            PanelSlot::Watchlist { watchlist_index: 0 },
        ],
        ..Default::default()
    };

    config.save(&path).expect("save watchlist config");
    let loaded = AppConfig::load(&path).expect("load watchlist config");

    assert_eq!(loaded.watchlists.len(), 1);
    assert_eq!(loaded.watchlists[0].name, "Main");
    assert_eq!(loaded.watchlists[0].tickers.len(), 2);
    assert_eq!(loaded.watchlists[0].tickers[0].symbol, "AAPL");
    assert_eq!(loaded.watchlists[0].tickers[0].favorite, 1);
    assert_eq!(loaded.watchlists[0].tickers[1].symbol, "MSFT");
    assert_eq!(loaded.watchlists[0].tickers[1].favorite, 0);

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
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        }],
        ..Default::default()
    };

    config.save(&path).expect("save");
    let loaded = AppConfig::load(&path).expect("load");

    assert_eq!(
        loaded.charts[0].symbol_link,
        LinkMode::Color(LinkColor::Blue)
    );
    assert_eq!(loaded.charts[0].timeframe_link, LinkMode::ListenAll);

    // Verify the TOML contains the flat string representation.
    let raw = std::fs::read_to_string(&path).expect("read");
    assert!(
        raw.contains("symbol_link = \"blue\""),
        "expected flat string, got:\n{raw}"
    );
    assert!(
        raw.contains("timeframe_link = \"listen_all\""),
        "expected flat string, got:\n{raw}"
    );

    cleanup(&dir);
}

#[test]
fn order_panel_bracket_active_roundtrip() {
    let dir = temp_dir();
    let path = dir.join("op_bracket.toml");

    let config = AppConfig {
        order_panels: vec![
            OrderPanelConfig {
                symbol: "AAPL".into(),
                bracket_active: Some("BUY".into()),
                ..Default::default()
            },
            OrderPanelConfig {
                symbol: "MSFT".into(),
                bracket_active: Some("SELL".into()),
                ..Default::default()
            },
            OrderPanelConfig {
                symbol: "TSLA".into(),
                bracket_active: None,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    config.save(&path).expect("save");
    let loaded = AppConfig::load(&path).expect("load");

    assert_eq!(loaded.order_panels.len(), 3);
    assert_eq!(loaded.order_panels[0].bracket_active, Some("BUY".into()),);
    assert_eq!(loaded.order_panels[1].bracket_active, Some("SELL".into()),);
    assert_eq!(loaded.order_panels[2].bracket_active, None);

    cleanup(&dir);
}

#[test]
fn old_config_without_bracket_active_loads_with_none() {
    let dir = temp_dir();
    let path = dir.join("op_no_bracket.toml");

    // Simulate old config that predates bracket_active field.
    std::fs::write(
        &path,
        r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"

[[order_panels]]
symbol = "AAPL"
side = "BUY"
quantity = "100"
symbol_link = "unlinked"
"#,
    )
    .expect("write old order panel config");

    let config = AppConfig::load(&path).expect("load");
    assert_eq!(config.order_panels.len(), 1);
    assert_eq!(config.order_panels[0].symbol, "AAPL");
    assert_eq!(config.order_panels[0].bracket_active, None);

    cleanup(&dir);
}

#[test]
fn load_migrates_order_blotters_and_writes_backup() {
    let dir = temp_dir();
    let path = dir.join("config.toml");

    let mut file = std::fs::File::create(&path).expect("create toml");
    file.write_all(
        br#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"

[[order_blotters]]
name = "Orders"
column_widths = [80.0, 60.0]
symbol_link = "unlinked"
hidden_columns = ["tp"]

[[panel_order]]
type = "order_blotter"
order_blotter_index = 0

[[layout_tree]]
type = "order_blotter"
order_blotter_index = 0
"#,
    )
    .expect("write legacy config");

    let config = AppConfig::load(&path).expect("load migrates");
    assert!(config.order_blotters.is_empty(), "legacy vec drained");
    assert_eq!(config.account_panels.len(), 1, "one account panel");
    assert_eq!(config.account_panels[0].active_tab, AccountTab::Orders);
    assert_eq!(
        config.account_panels[0].orders.column_widths,
        vec![80.0, 60.0]
    );
    assert_eq!(
        config.account_panels[0].orders.hidden_columns,
        vec!["tp".to_string()]
    );
    // Panel-slot / layout-tree rewritten.
    assert!(matches!(
        config.panel_order[0],
        PanelSlot::Account {
            account_panel_index: 0
        }
    ));
    assert!(matches!(
        config.layout_tree[0],
        LayoutNode::Account {
            account_panel_index: 0
        }
    ));

    // Backup exists and holds the pre-migration bytes. Filename is
    // `<name>.bak-v<initial>-to-v<current>` — the legacy file has
    // no `version` field so it deserializes as v1 and the framework
    // walks it to the current version.
    let backup = dir.join(format!(
        "config.toml.bak-v1-to-v{}",
        crate::config::CURRENT_CONFIG_VERSION
    ));
    assert!(backup.exists(), "backup file written");
    let backup_contents = std::fs::read_to_string(&backup).expect("read backup");
    assert!(
        backup_contents.contains("[[order_blotters]]"),
        "backup retains legacy section"
    );
    assert!(
        !backup_contents.contains("[[account_panels]]"),
        "backup is pre-migration"
    );

    cleanup(&dir);
}

#[test]
fn recent_symbols_roundtrip_preserves_order() {
    let dir = temp_dir();
    let path = dir.join("recents.toml");

    let config = AppConfig {
        recent_symbols: vec!["AAPL".into(), "TSLA".into(), "MSFT".into()],
        ..Default::default()
    };

    config.save(&path).expect("save recents");
    let loaded = AppConfig::load(&path).expect("load recents");
    assert_eq!(
        loaded.recent_symbols,
        vec!["AAPL".to_string(), "TSLA".into(), "MSFT".into()],
    );

    // Loading a config written before this field existed must still work.
    let legacy_path = dir.join("legacy.toml");
    std::fs::write(
        &legacy_path,
        r#"
[window]
width = 1280
height = 800

[theme]
mode = "dark"
"#,
    )
    .expect("write legacy");
    let legacy = AppConfig::load(&legacy_path).expect("load legacy");
    assert!(legacy.recent_symbols.is_empty(), "missing key => empty vec");

    cleanup(&dir);
}

// ── Chart-transition slice 9a: `ChartBackend` persistence tests ──────

/// Default for `ChartBackend` is `Legacy` (slice 9a). Slice 9b flips
/// this to `New`.
#[test]
fn chart_backend_default_is_legacy() {
    assert_eq!(ChartBackend::default(), ChartBackend::Legacy);
}

/// Legacy enum variant serializes to the lowercase string `"legacy"`
/// inside a TOML table (TOML can't serialize a bare enum, so we wrap
/// in a `ChartConfig` context).
#[test]
fn chart_backend_serializes_lowercase() {
    let cfg = ChartConfig {
        symbol: "X".into(),
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
        bound_symbol: None,
        backend: Some(ChartBackend::Legacy),
        show_extended_hours: true,
        show_extended_hours_bands: true,
        volume_profile: VolumeProfileSettings::default(),
    };
    let toml_str = toml::to_string_pretty(&cfg).expect("serialize legacy");
    assert!(toml_str.contains("backend = \"legacy\""), "got: {toml_str}");
    let cfg_new = ChartConfig {
        backend: Some(ChartBackend::New),
        ..cfg
    };
    let toml_str = toml::to_string_pretty(&cfg_new).expect("serialize new");
    assert!(toml_str.contains("backend = \"new\""), "got: {toml_str}");
}

/// Deserializing `backend = "new"` yields [`ChartBackend::New`].
/// Critically, this works regardless of the `session_chart` feature
/// flag — the enum parse is feature-independent (plan Scenario 9).
#[test]
fn chart_backend_new_deserializes_without_feature() {
    let cfg: ChartConfig = toml::from_str(
        r#"
            symbol = "AAPL"
            timeframe = "1D"
            backend = "new"
        "#,
    )
    .expect("deserialize");
    assert_eq!(cfg.backend, Some(ChartBackend::New));
}

/// Deserializing `backend = "legacy"` yields [`ChartBackend::Legacy`].
#[test]
fn chart_backend_legacy_deserializes() {
    let cfg: ChartConfig = toml::from_str(
        r#"
            symbol = "AAPL"
            timeframe = "1D"
            backend = "legacy"
        "#,
    )
    .expect("deserialize");
    assert_eq!(cfg.backend, Some(ChartBackend::Legacy));
}

/// Backward compat: a config without the `backend` field loads
/// cleanly, with `backend == None`. The app resolves `None` to
/// the current default (`Legacy` in slice 9a, `New` after 9b).
#[test]
fn chart_backend_absent_defaults_to_none() {
    let cfg: ChartConfig = toml::from_str(
        r#"
            symbol = "AAPL"
            timeframe = "1D"
        "#,
    )
    .expect("deserialize");
    assert!(cfg.backend.is_none(), "absent field loads as None");
}

/// Full round-trip: `backend = Some(New)` survives save+load.
#[test]
fn chart_backend_roundtrip_new() {
    let cfg = ChartConfig {
        symbol: "MSFT".to_string(),
        timeframe: "5m".to_string(),
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
        bound_symbol: None,
        backend: Some(ChartBackend::New),
        show_extended_hours: true,
        show_extended_hours_bands: true,
        volume_profile: VolumeProfileSettings::default(),
    };
    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let restored: ChartConfig = toml::from_str(&toml_str).expect("deserialize");
    assert_eq!(restored.backend, Some(ChartBackend::New));
}

// ── VP-anchored Slice 1: schema + kill-switch tests ──────────────────

/// Test #1 — every new `volume_profile` field round-trips through
/// save → load. Models the existing
/// `save_load_roundtrip_preserves_all_fields` style: per-field hand
/// asserts, no `..Default::default()`, so a future field rename
/// surfaces as a compile error rather than a silent miss.
#[test]
fn vp_settings_roundtrip_preserves_all_fields() {
    let dir = temp_dir();
    let path = dir.join("vp_roundtrip.toml");

    let cfg = AppConfig {
        charts: vec![ChartConfig {
            symbol: "AAPL".into(),
            timeframe: "5m".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: true,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings {
                anchor: VolumeProfileAnchor::Daily,
                width_fraction: 0.65,
                show_value_area: true,
                value_area_pct: 0.68,
            },
        }],
        ..Default::default()
    };
    cfg.save(&path).expect("save");
    let loaded = AppConfig::load(&path).expect("load");

    let vp = &loaded.charts[0].volume_profile;
    assert_eq!(vp.anchor, VolumeProfileAnchor::Daily);
    assert!((vp.width_fraction - 0.65).abs() < f32::EPSILON);
    assert!(vp.show_value_area);
    assert!((vp.value_area_pct - 0.68).abs() < f32::EPSILON);

    cleanup(&dir);
}

/// Test #2 — pre-feature TOML (no `[volume_profile]` table) loads
/// with default settings. Critical regression guard: the
/// serde-defaults pipe is the only thing keeping users on existing
/// configs from crashing.
#[test]
fn vp_settings_legacy_config_loads_with_defaults() {
    let dir = temp_dir();
    let path = dir.join("vp_legacy.toml");
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
show_volume_profile = true
"#,
    )
    .expect("write legacy config");
    let cfg = AppConfig::load(&path).expect("load legacy");
    assert_eq!(cfg.charts.len(), 1);
    let vp = &cfg.charts[0].volume_profile;
    assert_eq!(vp, &VolumeProfileSettings::default());
    assert_eq!(vp.anchor, VolumeProfileAnchor::Viewport);
    cleanup(&dir);
}

/// Test #3 — a config with a future-version anchor (e.g. `Quarterly`)
/// loads cleanly and falls back to `Unknown`. Render code in
/// S2/S3 must treat `Unknown` exactly like `Viewport` so a
/// downgraded binary never crashes on an upgraded config.
#[test]
fn vp_settings_unknown_anchor_falls_back() {
    let dir = temp_dir();
    let path = dir.join("vp_unknown.toml");
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
show_volume_profile = true

[charts.volume_profile]
anchor = "Quarterly"
width_fraction = 0.5
"#,
    )
    .expect("write future config");
    let cfg = AppConfig::load(&path).expect("load future");
    assert_eq!(
        cfg.charts[0].volume_profile.anchor,
        VolumeProfileAnchor::Unknown,
        "future-version anchor should downgrade to Unknown",
    );
    cleanup(&dir);
}

/// Test #4 — `width_fraction` is clamped to `[0.05, 1.0]` and
/// `value_area_pct` to `[0.10, 0.95]` on save. Belt & braces: the
/// in-memory value can be temporarily out-of-range (e.g. mid-drag),
/// but a single save→load cycle normalises it. `restore_panel`
/// also calls `sanitized()` on read for the symmetric guarantee.
#[test]
fn vp_settings_clamps_out_of_range() {
    let s = VolumeProfileSettings {
        anchor: VolumeProfileAnchor::Daily,
        width_fraction: 5.0, // way over
        show_value_area: false,
        value_area_pct: 0.05, // under floor
    };
    let s2 = s.sanitized();
    assert!((s2.width_fraction - 1.0).abs() < f32::EPSILON);
    assert!((s2.value_area_pct - 0.10).abs() < f32::EPSILON);

    let s = VolumeProfileSettings {
        anchor: VolumeProfileAnchor::Daily,
        width_fraction: -1.0, // way under
        show_value_area: false,
        value_area_pct: 1.5, // over ceiling
    };
    let s2 = s.sanitized();
    assert!((s2.width_fraction - 0.05).abs() < f32::EPSILON);
    assert!((s2.value_area_pct - 0.95).abs() < f32::EPSILON);
}

/// Test #5a (kill-switch) — `AppConfig::default()` ships with
/// `experimental.disable_anchored_vp == false`. A config without an
/// `[experimental]` table loads cleanly with the same default.
#[test]
fn kill_switch_default_off() {
    let cfg = AppConfig::default();
    assert!(!cfg.experimental.disable_anchored_vp);

    let dir = temp_dir();
    let path = dir.join("no_experimental.toml");
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
    .expect("write");
    let cfg = AppConfig::load(&path).expect("load");
    assert!(!cfg.experimental.disable_anchored_vp);
    cleanup(&dir);
}

/// Test — kill-switch flag round-trips through save → load.
#[test]
fn kill_switch_roundtrips() {
    let dir = temp_dir();
    let path = dir.join("kill_switch.toml");
    let mut cfg = AppConfig::default();
    cfg.experimental.disable_anchored_vp = true;
    cfg.save(&path).expect("save");
    let loaded = AppConfig::load(&path).expect("load");
    assert!(loaded.experimental.disable_anchored_vp);
    cleanup(&dir);
}

/// `backend: None` is skipped during serialization so existing
/// configs remain byte-identical after a save that only flipped
/// unrelated fields (no stray `backend = "legacy"` pollution).
#[test]
fn chart_backend_none_skips_serialization() {
    let cfg = ChartConfig {
        symbol: "TSLA".to_string(),
        timeframe: "1D".to_string(),
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
        bound_symbol: None,
        backend: None,
        show_extended_hours: true,
        show_extended_hours_bands: true,
        volume_profile: VolumeProfileSettings::default(),
    };
    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    assert!(
        !toml_str.contains("backend"),
        "backend: None must not serialize. got: {toml_str}"
    );
}

// ─── S3: ETH knobs (show_extended_hours{,_bands}) ────────────────────────────

/// Old configs saved before the ETH-shading slice landed are missing
/// both `show_extended_hours` and `show_extended_hours_bands`. They
/// must load with both knobs defaulting to `true` — TradingView-parity
/// behavior for new sessions.
#[test]
fn eth_knobs_absent_default_to_true() {
    let cfg: ChartConfig = toml::from_str(
        r#"
            symbol = "AAPL"
            timeframe = "1D"
        "#,
    )
    .expect("deserialize");
    assert!(
        cfg.show_extended_hours,
        "absent show_extended_hours must default to true"
    );
    assert!(
        cfg.show_extended_hours_bands,
        "absent show_extended_hours_bands must default to true"
    );
}

/// User explicitly opting out of ETH on disk must round-trip — a
/// `false` value cannot be silently overwritten by the default.
#[test]
fn eth_knobs_explicit_false_round_trips() {
    let cfg: ChartConfig = toml::from_str(
        r#"
            symbol = "AAPL"
            timeframe = "1D"
            show_extended_hours = false
            show_extended_hours_bands = false
        "#,
    )
    .expect("deserialize");
    assert!(!cfg.show_extended_hours);
    assert!(!cfg.show_extended_hours_bands);

    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let restored: ChartConfig = toml::from_str(&toml_str).expect("deserialize");
    assert!(!restored.show_extended_hours);
    assert!(!restored.show_extended_hours_bands);
}

/// Mixed-state round-trip: data on, bands off (a plausible user
/// preference — keep ETH bars visible but hide the tint overlay).
#[test]
fn eth_knobs_mixed_state_round_trips() {
    let cfg = ChartConfig {
        symbol: "MSFT".to_string(),
        timeframe: "1m".to_string(),
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
        bound_symbol: None,
        backend: None,
        show_extended_hours: true,
        show_extended_hours_bands: false,
        volume_profile: VolumeProfileSettings::default(),
    };
    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let restored: ChartConfig = toml::from_str(&toml_str).expect("deserialize");
    assert!(restored.show_extended_hours);
    assert!(!restored.show_extended_hours_bands);
}

/// Default values are not skipped on serialization — we want the
/// generated TOML to be self-documenting so a user inspecting the
/// file sees both knobs spelled out, not just inheriting from
/// some implicit default.
#[test]
fn eth_knobs_default_true_serializes_explicitly() {
    let cfg = ChartConfig {
        symbol: "AAPL".to_string(),
        timeframe: "1D".to_string(),
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
        bound_symbol: None,
        backend: None,
        show_extended_hours: true,
        show_extended_hours_bands: true,
        volume_profile: VolumeProfileSettings::default(),
    };
    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    assert!(
        toml_str.contains("show_extended_hours"),
        "show_extended_hours = true must serialize explicitly. got: {toml_str}"
    );
    assert!(
        toml_str.contains("show_extended_hours_bands"),
        "show_extended_hours_bands = true must serialize explicitly. got: {toml_str}"
    );
}
