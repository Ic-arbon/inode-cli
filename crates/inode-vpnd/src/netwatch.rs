//! Linux network-change watcher (M3).
//!
//! Spawns `ip monitor address link` and forwards physical-interface events
//! to [`Shared::network_changed`], which debounces and issues OC_CMD_PAUSE
//! so the engine reconnects on the same cookie.

use crate::Shared;
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[cfg(target_os = "linux")]
pub fn spawn(shared: Arc<Shared>) -> JoinHandle<()> {
    thread::Builder::new()
        .name("inode-netwatch".into())
        .spawn(move || loop {
            // Do not monitor `route`: the engine's own inode-routectl adds
            // routes after every connect/reconnect, which would be mistaken
            // for a physical network change and trigger an endless
            // reconnect loop. Address/link events on the physical interface
            // are the reliable signal.
            let mut child = match Command::new("ip")
                .args(["monitor", "address", "link"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(e) => {
                    tracing::warn!("ip monitor spawn failed: {e}");
                    thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };

            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(line) if interesting_event(&line) => {
                            tracing::debug!(line = %line.trim(), "netlink event");
                            shared.network_changed();
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            }
            let _ = child.wait();
            tracing::warn!("ip monitor exited; restarting");
            thread::sleep(std::time::Duration::from_secs(1));
        })
        .expect("spawn netwatch thread")
}

/// True for address/link events on interfaces we care about. `ip monitor`
/// line shapes are `3: wlo1    inet ...` (address) and `3: wlo1: <...>`
/// (link); everything else, including the engine's own `tun0` and `lo`,
/// is ignored.
#[cfg(target_os = "linux")]
fn interesting_event(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Some((_, rest)) = trimmed.split_once(':') else {
        return false;
    };
    let iface = rest
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(':');
    !iface.is_empty() && iface != "lo" && !iface.starts_with("tun")
}

#[cfg(not(target_os = "linux"))]
pub fn spawn(_shared: Arc<Shared>) -> JoinHandle<()> {
    // macOS uses SystemConfiguration in M4.
    thread::Builder::new()
        .name("inode-netwatch".into())
        .spawn(|| {})
        .expect("spawn netwatch thread")
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "linux")]
    fn filters_engine_internal_interfaces() {
        assert!(super::interesting_event("3: wlo1    inet 192.168.0.128/23"));
        assert!(super::interesting_event(
            "3: wlo1: <BROADCAST,MULTICAST,UP,LOWER_UP>"
        ));
        assert!(!super::interesting_event("4: tun0    inet 10.1.1.10/24"));
        assert!(!super::interesting_event("4: tun0: <POINTOPOINT,UP>"));
        assert!(!super::interesting_event("1: lo    inet 127.0.0.1/8"));
        assert!(!super::interesting_event(""));
    }
}
