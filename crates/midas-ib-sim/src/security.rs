//! Security model — bearer token generation, file permissions,
//! external-bind gate.
//!
//! The sim refuses to bind a non-loopback address unless both
//! `--listen-external` *and* `external_bind_acknowledged = true` are set
//! (mirrors the `allow_live` pattern in `midas-core::config`). This file owns
//! both sides of the gate.

use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use rand::RngCore;

/// Bearer token used to authenticate control-plane requests. Token bytes are
/// deliberately not logged anywhere — formatting impls are redacted.
#[derive(Clone)]
pub struct ControlToken {
    secret: String,
}

impl std::fmt::Debug for ControlToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlToken")
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl ControlToken {
    /// Generate a fresh random token (32 bytes of OS entropy, hex-encoded).
    pub fn generate() -> Self {
        let mut buf = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        Self {
            secret: hex_encode(&buf),
        }
    }

    /// Constant-time equality check against a presented token value.
    pub fn matches(&self, presented: &str) -> bool {
        constant_time_eq(self.secret.as_bytes(), presented.as_bytes())
    }

    /// Borrow the raw secret bytes (only used by `write_to_disk`).
    pub fn as_str(&self) -> &str {
        &self.secret
    }
}

/// Default location for the control-plane token. Consumers of the token
/// (devloop / tests) read it from here.
pub fn default_token_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("midas-ib-sim").join("control.token"))
}

/// Write the token to `path` with 0600 permissions (Unix) / owner-only ACL
/// (Windows). Creates parent directories as needed.
pub fn write_token_to_disk(token: &ControlToken, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Remove any stale token first so we don't inherit old permissions.
    let _ = fs::remove_file(path);

    // Create with owner-only permissions on Unix.
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
    };

    // On Windows, the default ACL for files inside the user's data-local dir
    // already restricts to the user; we document the expectation here.
    #[cfg(not(unix))]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;

    file.write_all(token.as_str().as_bytes())?;
    file.flush()?;
    Ok(())
}

/// Thrown when `--listen-external` is requested without the
/// `external_bind_acknowledged` safety flag.
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    #[error(
        "--listen-external requires external_bind_acknowledged=true in config (refusing to bind non-loopback without explicit dual-flag consent)"
    )]
    ExternalBindNotAcknowledged,
    #[error("invalid bind address: {0}")]
    InvalidAddress(String),
}

/// Validate a bind decision. Returns the IP the sim should bind, or an error
/// describing why the requested configuration is refused.
///
/// - By default (no `--listen-external`), returns `127.0.0.1` unconditionally.
/// - With `--listen-external` + `acknowledged = false`, returns
///   `ExternalBindNotAcknowledged`.
/// - With `--listen-external` + `acknowledged = true`, returns `0.0.0.0`.
pub fn resolve_bind_address(
    listen_external: bool,
    acknowledged: bool,
) -> Result<IpAddr, BindError> {
    match (listen_external, acknowledged) {
        (false, _) => Ok(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        (true, false) => Err(BindError::ExternalBindNotAcknowledged),
        (true, true) => Ok(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
    }
}

/// Returns `true` if `addr` is a loopback IP.
pub fn is_loopback(addr: IpAddr) -> bool {
    addr.is_loopback()
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_is_random_and_hex() {
        let a = ControlToken::generate();
        let b = ControlToken::generate();
        assert_eq!(a.secret.len(), 64, "32 bytes hex-encoded = 64 chars");
        assert_ne!(a.secret, b.secret, "tokens must not collide");
        assert!(a.secret.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_matches_constant_time() {
        let t = ControlToken::generate();
        assert!(t.matches(&t.secret.clone()));
        assert!(!t.matches(""));
        assert!(!t.matches("deadbeef"));
    }

    #[test]
    fn token_debug_redacts() {
        let t = ControlToken::generate();
        let dbg = format!("{t:?}");
        assert!(dbg.contains("redacted"), "token Debug must redact: {dbg}");
        assert!(!dbg.contains(&t.secret), "token Debug must not leak secret");
    }

    #[test]
    fn default_listen_is_loopback() {
        let ip = resolve_bind_address(false, false).unwrap();
        assert!(is_loopback(ip));
    }

    #[test]
    fn external_bind_needs_ack() {
        let err = resolve_bind_address(true, false).unwrap_err();
        matches!(err, BindError::ExternalBindNotAcknowledged);
    }

    #[test]
    fn external_bind_with_ack_unspecified() {
        let ip = resolve_bind_address(true, true).unwrap();
        assert!(!is_loopback(ip));
    }

    #[test]
    fn write_token_creates_file_and_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("control.token");
        let token = ControlToken::generate();
        write_token_to_disk(&token, &path).unwrap();
        let s = std::fs::read_to_string(&path).unwrap();
        assert_eq!(s, token.secret);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "token file must be 0600 on unix");
        }
    }
}
