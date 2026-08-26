//! System service installation helpers (Linux: systemd, M3).

use std::path::{Path, PathBuf};
use std::process::Command;

pub const UNIT_NAME: &str = "inode-vpnd@";

pub fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

pub fn unit_name_for(uid: u32) -> String {
    format!("{UNIT_NAME}{uid}.service")
}

pub fn stable_exec_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("INODE_SERVICE_EXEC") {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "cannot locate executable directory".to_string())
}

pub fn unit_text(uid: u32, exec: &Path) -> String {
    format!(
        r#"[Unit]
Description=inode-vpn daemon (user {uid})
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=root
ExecStart={exec} --uid {uid}
RuntimeDirectory=inode-vpn
RuntimeDirectoryMode=0750
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
"#,
        exec = exec.display()
    )
}

fn sudo(args: &[&str]) -> Result<(), String> {
    let status = Command::new("sudo")
        .args(args)
        .status()
        .map_err(|e| format!("failed to run sudo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("sudo {:?} exited {status}", args))
    }
}

/// Create `/var/lib/inode-vpn/current -> <dir containing inode-vpnd>` so the
/// unit never references a GC-able /nix/store path.
pub fn install_stable_link(exec: &Path) -> Result<(), String> {
    let bin_dir = exec.to_path_buf();
    sudo(&["mkdir", "-p", "/var/lib/inode-vpn"])?;
    sudo(&[
        "ln",
        "-sfn",
        bin_dir.to_str().ok_or("non-UTF8 exec path")?,
        "/var/lib/inode-vpn/current",
    ])
}

pub fn enable(uid: u32, now: bool) -> Result<(), String> {
    let bin_dir = stable_exec_path()?;
    install_stable_link(&bin_dir)?;
    let exec = PathBuf::from("/var/lib/inode-vpn/current/bin/inode-vpnd");
    let unit = unit_text(uid, &exec);

    let tmp = std::env::temp_dir().join(unit_name_for(uid));
    std::fs::write(&tmp, unit).map_err(|e| e.to_string())?;
    let unit_path = format!("/etc/systemd/system/{}", unit_name_for(uid));
    sudo(&[
        "install",
        "-m",
        "0644",
        tmp.to_str().ok_or("bad tmp path")?,
        &unit_path,
    ])?;
    let _ = std::fs::remove_file(tmp);
    sudo(&["systemctl", "daemon-reload"])?;
    if now {
        sudo(&["systemctl", "enable", "--now", &unit_name_for(uid)])?;
    } else {
        sudo(&["systemctl", "enable", &unit_name_for(uid)])?;
    }
    Ok(())
}

pub fn disable(uid: u32, now: bool) -> Result<(), String> {
    let unit = unit_name_for(uid);
    if now {
        sudo(&["systemctl", "disable", "--now", &unit])?;
    } else {
        sudo(&["systemctl", "disable", &unit])?;
    }
    sudo(&["rm", "-f", &format!("/etc/systemd/system/{unit}")])?;
    sudo(&["systemctl", "daemon-reload"])?;
    Ok(())
}

pub fn logs(uid: u32, follow: bool) -> Result<(), String> {
    let unit = unit_name_for(uid);
    let mut cmd = Command::new("journalctl");
    cmd.arg("-u").arg(&unit);
    if follow {
        cmd.arg("-f");
    }
    let status = cmd.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("journalctl exited {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_text_contains_stable_contract() {
        let text = unit_text(501, Path::new("/var/lib/inode-vpn/current/bin/inode-vpnd"));
        assert!(text.contains("Description=inode-vpn daemon (user 501)"));
        assert!(text.contains("ExecStart=/var/lib/inode-vpn/current/bin/inode-vpnd --uid 501"));
        assert!(text.contains("Restart=always"));
        assert!(text.contains("RuntimeDirectory=inode-vpn"));
        assert!(text.contains("WantedBy=multi-user.target"));
    }
}
