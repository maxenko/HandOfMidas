use std::fmt;

/// Connection state machine for the IB TWS / Gateway link.
///
/// ```text
/// Disconnected ──> Connecting ──> Connected ──> Ready
///       ^                              │
///       │                              v
///       └──────── Reconnecting <───────┘
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// No connection has been established or the connection was cleanly closed.
    Disconnected,
    /// TCP handshake and IB API negotiation in progress.
    Connecting,
    /// TCP connected and API version negotiated; waiting for account readiness.
    Connected { server_version: i32 },
    /// Fully operational: account data received, ready to accept commands.
    Ready,
    /// Connection was lost; attempting automatic recovery.
    Reconnecting { attempt: u32 },
}

impl ConnectionState {
    /// Returns `true` when the engine has at least a TCP connection
    /// (either `Connected` or `Ready`).
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::Ready)
    }

    /// Returns `true` only when the engine is fully operational.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected { server_version } => {
                write!(f, "Connected (server v{server_version})")
            }
            Self::Ready => write!(f, "Ready"),
            Self::Reconnecting { attempt } => {
                write!(f, "Reconnecting (attempt {attempt})")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state_transitions() {
        let disconnected = ConnectionState::Disconnected;
        assert!(!disconnected.is_connected());
        assert!(!disconnected.is_ready());

        let connecting = ConnectionState::Connecting;
        assert!(!connecting.is_connected());
        assert!(!connecting.is_ready());

        let connected = ConnectionState::Connected {
            server_version: 176,
        };
        assert!(connected.is_connected());
        assert!(!connected.is_ready());

        let ready = ConnectionState::Ready;
        assert!(ready.is_connected());
        assert!(ready.is_ready());

        let reconnecting = ConnectionState::Reconnecting { attempt: 3 };
        assert!(!reconnecting.is_connected());
        assert!(!reconnecting.is_ready());
    }

    #[test]
    fn test_connection_state_display() {
        assert_eq!(ConnectionState::Disconnected.to_string(), "Disconnected");
        assert_eq!(ConnectionState::Connecting.to_string(), "Connecting");
        assert_eq!(
            ConnectionState::Connected {
                server_version: 176
            }
            .to_string(),
            "Connected (server v176)"
        );
        assert_eq!(ConnectionState::Ready.to_string(), "Ready");
        assert_eq!(
            ConnectionState::Reconnecting { attempt: 2 }.to_string(),
            "Reconnecting (attempt 2)"
        );
    }

    #[test]
    fn test_connection_state_equality() {
        assert_eq!(ConnectionState::Ready, ConnectionState::Ready);
        assert_ne!(ConnectionState::Ready, ConnectionState::Disconnected);
        assert_eq!(
            ConnectionState::Connected {
                server_version: 176
            },
            ConnectionState::Connected {
                server_version: 176
            },
        );
        assert_ne!(
            ConnectionState::Connected {
                server_version: 176
            },
            ConnectionState::Connected {
                server_version: 177
            },
        );
    }

    #[test]
    fn test_connection_state_clone() {
        let state = ConnectionState::Connected {
            server_version: 176,
        };
        let cloned = state.clone();
        assert_eq!(state, cloned);
    }
}
