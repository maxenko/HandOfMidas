//! User-named window identifier.
//!
//! `WindowKey` is the user-visible name and the persistence key. Stored
//! as `Arc<str>` so clones (used in many message-construction closures)
//! are cheap. Renaming a window is a remove-and-reinsert in the
//! `BTreeMap<WindowKey, WindowState>` that owns it.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The default name for the main window on a fresh install.
pub const MAIN_DEFAULT: &str = "Main";

/// Maximum byte length for a window name after trimming.
pub const MAX_LEN: usize = 64;

/// User-supplied window name. Acts as the BTreeMap key in
/// `MidasApp::windows` and on-disk under `[windows.<name>]`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WindowKey(Arc<str>);

impl Serialize for WindowKey {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WindowKey {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Ok(Self(Arc::from(s.as_str())))
    }
}

/// Reasons a candidate window name fails normalisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// Trim left an empty string.
    Empty,
    /// Trimmed length exceeds [`MAX_LEN`].
    TooLong,
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("window name is empty or whitespace"),
            Self::TooLong => write!(f, "window name exceeds {MAX_LEN} bytes"),
        }
    }
}

impl std::error::Error for NameError {}

impl WindowKey {
    /// Default name for the main window on a fresh install. Mirrors
    /// the module-level [`MAIN_DEFAULT`] constant; the associated form
    /// is the call site preferred by `WindowConfig`'s [`BTreeMap`]
    /// keying so most lookups read `windows[WindowKey::MAIN_DEFAULT]`.
    pub const MAIN_DEFAULT: &'static str = MAIN_DEFAULT;

    /// Construct without normalisation. Caller-asserts the input is
    /// already trimmed and within bounds. Use [`Self::normalize`] for
    /// untrusted input.
    pub fn new(s: impl Into<Arc<str>>) -> Self {
        Self(s.into())
    }

    /// The default key for a freshly installed app.
    pub fn main_default() -> Self {
        Self(Arc::from(MAIN_DEFAULT))
    }

    /// Borrow the inner name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Trim whitespace, reject empty / overlong names.
    ///
    /// Case is preserved for display; uniqueness checks at higher
    /// layers compare case-insensitively.
    pub fn normalize(s: &str) -> Result<Self, NameError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(NameError::Empty);
        }
        if trimmed.len() > MAX_LEN {
            return Err(NameError::TooLong);
        }
        Ok(Self(Arc::from(trimmed)))
    }
}

impl std::fmt::Display for WindowKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for WindowKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for WindowKey {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for WindowKey {
    fn from(s: String) -> Self {
        Self(Arc::from(s.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_whitespace() {
        let k = WindowKey::normalize("  Trading  ").unwrap();
        assert_eq!(k.as_str(), "Trading");
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(matches!(WindowKey::normalize(""), Err(NameError::Empty)));
        assert!(matches!(WindowKey::normalize("   "), Err(NameError::Empty)));
    }

    #[test]
    fn normalize_rejects_overlong() {
        let too_long = "x".repeat(MAX_LEN + 1);
        assert!(matches!(
            WindowKey::normalize(&too_long),
            Err(NameError::TooLong)
        ));
    }

    #[test]
    fn normalize_accepts_max_length() {
        let max = "x".repeat(MAX_LEN);
        let k = WindowKey::normalize(&max).unwrap();
        assert_eq!(k.as_str().len(), MAX_LEN);
    }

    #[test]
    fn main_default_matches_const() {
        assert_eq!(WindowKey::main_default().as_str(), MAIN_DEFAULT);
    }

    #[test]
    fn ord_is_alphabetical() {
        let mut keys = [
            WindowKey::new("Zeta"),
            WindowKey::new("Alpha"),
            WindowKey::new("Mike"),
        ];
        keys.sort();
        assert_eq!(keys[0].as_str(), "Alpha");
        assert_eq!(keys[1].as_str(), "Mike");
        assert_eq!(keys[2].as_str(), "Zeta");
    }

    #[test]
    fn serde_round_trip_is_transparent_string() {
        let k = WindowKey::new("My Layout");
        let json = serde_json::to_string(&k).unwrap();
        assert_eq!(json, "\"My Layout\"");
        let back: WindowKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn clone_shares_arc() {
        let a = WindowKey::new("Shared");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.as_str().as_ptr(), b.as_str().as_ptr());
    }
}
