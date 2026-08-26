//! vpnc-script replacement.
//!
//! M0 skeleton: parse `reason` and dispatch to platform modules. Platform
//! implementations arrive in M3 (Linux) and M4 (macOS).

use inode_core::{Error, Result};

pub const SUPPORTED_REASONS: &[&str] = &["pre-init", "connect", "reconnect", "disconnect"];

/// Run one vpnc-script phase. openconnect passes tunnel parameters through
/// the process environment (`INTERNAL_IP4_ADDRESS`, `CISCO_SPLIT_INC_*`, ...).
pub fn run(reason: &str) -> Result<()> {
    if !SUPPORTED_REASONS.contains(&reason) {
        return Err(Error::Route(format!(
            "unsupported vpnc-script reason: {reason}"
        )));
    }
    tracing::info!(reason, "inode-routectl phase (platform impl pending)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_reason() {
        assert!(run("explode").is_err());
    }

    #[test]
    fn accepts_vpnc_script_phases() {
        for reason in SUPPORTED_REASONS {
            assert!(run(reason).is_ok(), "reason {reason} should be accepted");
        }
    }
}
