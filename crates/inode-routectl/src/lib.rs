//! vpnc-script replacement.
//!
//! `inode-routectl <reason>` is exec'd by libopenconnect with the vpnc-script
//! environment (`INTERNAL_IP4_*`, `CISCO_SPLIT_INC_*`, `TUNDEV`, ...).
//! Pure parsing lives in [`plan`]; Linux command execution in [`linux`].

pub mod linux;
pub mod plan;

use inode_core::{Error, Result};
use std::env;

pub const SUPPORTED_REASONS: &[&str] = &["pre-init", "connect", "reconnect", "disconnect"];

/// Parse the vpnc-script environment and run one phase.
pub fn run(reason: &str) -> Result<()> {
    if !SUPPORTED_REASONS.contains(&reason) {
        return Err(Error::Route(format!(
            "unsupported vpnc-script reason: {reason}"
        )));
    }
    let dry_run = env::var("INODE_ROUTECTL_DRY_RUN").is_ok_and(|v| v == "1");
    let plan = plan::RoutePlan::from_env(reason)?;
    tracing::info!(reason, tun = %plan.tun_iface, dry_run, "inode-routectl phase");
    if dry_run {
        println!("{}", plan.describe());
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    linux::apply(&plan)?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = &plan;
        return Err(Error::Route(
            "platform implementation not available yet (macOS lands in M4)".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_reason() {
        assert!(matches!(run("explode"), Err(Error::Route(_))));
    }

    #[test]
    fn accepts_vpnc_script_phases() {
        for reason in SUPPORTED_REASONS {
            let result = run(reason);
            // On non-Linux test hosts a missing platform implementation is
            // fine; the reason itself must be accepted.
            if let Err(Error::Route(msg)) = result {
                assert!(!msg.starts_with("unsupported vpnc-script reason"));
            }
        }
    }
}
