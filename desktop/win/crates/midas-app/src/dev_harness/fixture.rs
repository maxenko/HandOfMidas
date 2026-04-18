//! Disk I/O wrappers around [`MidasApp::snapshot_to_fixture`] and
//! [`MidasApp::apply_fixture_envelope`].
//!
//! The `app::fixture` module owns the conversion between `MidasApp`
//! and [`FixtureEnvelope`]. This module owns only the `.devloop/fixtures/`
//! file layout and JSON encoding.

use std::path::{Path, PathBuf};

use iced::Task;
use midas_devloop_proto::FixtureEnvelope;

use crate::app::{FixtureError, Message, MidasApp};

/// Fixtures live in a single directory next to the socket state.
pub const FIXTURES_DIR: &str = ".devloop/fixtures";

/// Resolve the on-disk path for a fixture name.
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(FIXTURES_DIR).join(format!("{name}.json"))
}

/// Capture the current app state to `<FIXTURES_DIR>/<name>.json`.
///
/// Returns the path written on success.
pub fn snapshot_to_disk(
    app: &MidasApp,
    name: &str,
    note: Option<String>,
) -> Result<PathBuf, FixtureError> {
    std::fs::create_dir_all(FIXTURES_DIR)?;
    let envelope = app.snapshot_to_fixture(note)?;
    let path = fixture_path(name);
    let text = serde_json::to_string_pretty(&envelope)?;
    std::fs::write(&path, text)?;
    tracing::info!("devloop: snapshotted fixture to {}", path.display());
    Ok(path)
}

/// Load a fixture by name from disk and apply it to `app`.
///
/// Returns the batched data-reload `Task` that should be fed back into
/// iced's update path.
pub fn apply_from_disk(app: &mut MidasApp, name: &str) -> Result<Task<Message>, FixtureError> {
    let path = fixture_path(name);
    if !path.exists() {
        return Err(FixtureError::NotFound(name.to_owned()));
    }
    apply_from_path(app, &path)
}

/// Same as [`apply_from_disk`] but takes an explicit path. Used for
/// boot-time `--fixture` flag where the caller has already resolved it.
pub fn apply_from_path(app: &mut MidasApp, path: &Path) -> Result<Task<Message>, FixtureError> {
    let text = std::fs::read_to_string(path)?;
    let envelope: FixtureEnvelope = serde_json::from_str(&text)?;
    app.apply_fixture_envelope(envelope)
}
