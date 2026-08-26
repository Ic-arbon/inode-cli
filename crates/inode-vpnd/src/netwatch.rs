//! Linux network-change watcher (M3).
//!
//! Spawns `ip monitor address link route` and forwards bursts to
//! [`Shared::network_changed`], which debounces and issues OC_CMD_PAUSE so
//! the engine reconnects on the same cookie.

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
            let mut child = match Command::new("ip")
                .args(["monitor", "address", "link", "route"])
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
                        Ok(line) if !line.trim().is_empty() => {
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

#[cfg(not(target_os = "linux"))]
pub fn spawn(_shared: Arc<Shared>) -> JoinHandle<()> {
    // macOS uses SystemConfiguration in M4.
    thread::Builder::new()
        .name("inode-netwatch".into())
        .spawn(|| {})
        .expect("spawn netwatch thread")
}
