//! daemon session state machine.
//!
//! See `docs/architecture.md` §5 for the canonical state diagram and
//! `status --json` schema. This enum must stay in sync with that document.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// No active session and no automatic-reconnect target.
    Stopped,
    /// Obtaining the `svpnginfo` cookie (TLS + login flow).
    Authenticating,
    /// `NET_EXTEND` in progress; tunnel parameters not confirmed yet.
    Connecting,
    /// Tunnel, tun device, routes and DNS are fully configured.
    Connected,
    /// Link lost; attempting same-cookie reconnect.
    Reconnecting,
    /// Authentication failed, timeout expired, or reconnect gave up.
    Failed,
}

impl SessionState {
    /// CLI process exit codes defined in the architecture:
    /// `Connected = 0`, `Failed = 1`, everything else = 3.
    pub fn exit_code(self) -> i32 {
        match self {
            SessionState::Connected => 0,
            SessionState::Failed => 1,
            _ => 3,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, SessionState::Stopped | SessionState::Failed)
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SessionState::Stopped => "Stopped",
            SessionState::Authenticating => "Authenticating",
            SessionState::Connecting => "Connecting",
            SessionState::Connected => "Connected",
            SessionState::Reconnecting => "Reconnecting",
            SessionState::Failed => "Failed",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_architecture() {
        assert_eq!(SessionState::Connected.exit_code(), 0);
        assert_eq!(SessionState::Failed.exit_code(), 1);
        assert_eq!(SessionState::Stopped.exit_code(), 3);
        assert_eq!(SessionState::Connecting.exit_code(), 3);
    }

    #[test]
    fn serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(SessionState::Reconnecting).unwrap(),
            serde_json::json!("reconnecting")
        );
    }
}
