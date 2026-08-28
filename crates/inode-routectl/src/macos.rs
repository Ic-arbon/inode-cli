//! macOS route executor for inode-routectl.
//!
//! Uses `ifconfig`/`route`/`scutil` equivalents, mirroring what the legacy
//! shell script did. DNS service-name handling stays best-effort for v1.

use crate::plan::RoutePlan;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PhysicalState {
    gateway: Option<String>,
    iface: Option<String>,
    local_ip: Option<String>,
}

pub fn apply(plan: &RoutePlan) -> Result<()> {
    match plan.reason.as_str() {
        // A previous engine may have died without running the vpnc-script
        // disconnect phase (sleep, forced kill, reconnect timeout). Its
        // host route for the VPN gateway can point at an unreachable old
        // next hop, which makes every new connect() fail with
        // EADDRNOTAVAIL ("Can't assign requested address"). Delete it
        // before openconnect opens the HTTPS socket.
        "pre-init" => {
            if let Some(gw) = &plan.vpn_gateway {
                let _ = run(&["delete", "-host", gw]);
            }
            Ok(())
        }
        "connect" | "reconnect" | "attempt-reconnect" => connect(plan),
        "disconnect" => disconnect(plan),
        other => Err(Error::Route(format!("unsupported reason: {other}"))),
    }
}

fn state_path(plan: &RoutePlan) -> PathBuf {
    let dir = std::env::var("INODE_ROUTECTL_STATE_DIR").unwrap_or_else(|_| {
        if cfg!(test) {
            std::env::temp_dir()
                .join("inode-routectl-test")
                .to_string_lossy()
                .into_owned()
        } else {
            "/var/run/inode-vpn".into()
        }
    });
    PathBuf::from(dir).join(format!("routectl-{}.json", plan.tun_iface))
}

fn save_state(plan: &RoutePlan, state: &PhysicalState) {
    let path = state_path(plan);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(state) {
        let _ = std::fs::write(path, text);
    }
}

fn load_state(plan: &RoutePlan) -> PhysicalState {
    std::fs::read_to_string(state_path(plan))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn discover(plan: &RoutePlan) -> Result<PhysicalState> {
    let gateway = plan
        .phys_gateway_override
        .clone()
        .or_else(|| default_route_field("gateway:").ok().flatten());
    let iface = plan
        .phys_iface_override
        .clone()
        .or_else(|| default_route_field("interface:").ok().flatten());
    let local_ip = match &iface {
        Some(iface) => ifconfig_local_ip(iface).ok().flatten(),
        None => None,
    };
    Ok(PhysicalState {
        gateway,
        iface,
        local_ip,
    })
}

fn connect(plan: &RoutePlan) -> Result<()> {
    let phys = discover(plan)?;
    save_state(plan, &phys);

    if let (Some(ip), Some(prefix)) = (&plan.tun_ip, plan.prefix_len) {
        let netmask = prefix_to_netmask(prefix);
        let mut cmd = Command::new("ifconfig");
        cmd.arg(&plan.tun_iface)
            .arg(ip)
            .arg(ip)
            .arg("netmask")
            .arg(netmask);
        if let Some(mtu) = plan.mtu {
            cmd.arg("mtu").arg(mtu.to_string());
        }
        cmd.arg("up");
        run_status(&mut cmd)?;
    }

    // VPN gateway host route on the physical path.
    if let (Some(gw), Some(phys_gw)) = (&plan.vpn_gateway, &phys.gateway) {
        run(&["add", "-host", gw, phys_gw])?;
    }
    // Protect the management address and explicit CIDRs.
    if let Some(phys_gw) = &phys.gateway {
        if let Some(local_ip) = &phys.local_ip {
            run(&["add", "-host", local_ip, phys_gw])?;
        }
        for cidr in &plan.preserve_cidrs {
            let (net, mask) = split_cidr(cidr)?;
            run(&["add", "-net", &net, "-netmask", &mask, phys_gw])?;
        }
    }
    // Split routes through the tun point-to-point peer address.
    if let Some(tun_ip) = &plan.tun_ip {
        for cidr in &plan.split_routes {
            let (net, mask) = split_cidr(cidr)?;
            run(&["add", "-net", &net, "-netmask", &mask, tun_ip])?;
        }
    }
    apply_dns(plan, &phys);
    Ok(())
}

fn disconnect(plan: &RoutePlan) -> Result<()> {
    let phys = {
        let saved = load_state(plan);
        if saved.gateway.is_some() || saved.iface.is_some() {
            saved
        } else {
            discover(plan)?
        }
    };

    if let Some(gw) = &plan.vpn_gateway {
        let _ = run(&["delete", "-host", gw]);
    }
    if let Some(local_ip) = &phys.local_ip {
        let _ = run(&["delete", "-host", local_ip]);
    }
    if let Some(tun_ip) = &plan.tun_ip {
        for cidr in &plan.split_routes {
            let (net, mask) = split_cidr(cidr).unwrap_or_default();
            let _ = run(&["delete", "-net", &net, "-netmask", &mask, tun_ip]);
        }
        let _ = Command::new("ifconfig")
            .args([&plan.tun_iface, "down"])
            .status();
    }
    revert_dns(plan, &phys);
    let _ = std::fs::remove_file(state_path(plan));
    Ok(())
}

fn run(args: &[&str]) -> Result<()> {
    run_status(Command::new("route").arg("-n").args(args))
}

fn run_status(cmd: &mut Command) -> Result<()> {
    let output = cmd
        .output()
        .map_err(|e| Error::Route(format!("failed to run {:?}: {e}", cmd)))?;
    if output.status.success() {
        tracing::debug!(cmd = ?cmd, "route command ok");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Route(format!(
            "{cmd:?} exited {}: {}",
            output.status,
            stderr.trim()
        )))
    }
}

/// Parse `gateway:`/`interface:` fields from `route -n get default`.
fn default_route_field(field: &str) -> Result<Option<String>> {
    let out = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .map_err(|e| Error::Route(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(field) {
            let value = rest.trim();
            if !value.is_empty() {
                return Ok(Some(value.to_string()));
            }
        }
    }
    Ok(None)
}

fn ifconfig_local_ip(iface: &str) -> Result<Option<String>> {
    let out = Command::new("ifconfig")
        .arg(iface)
        .output()
        .map_err(|e| Error::Route(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() == Some(&"inet") && words.len() > 1 {
            return Ok(Some(words[1].to_string()));
        }
    }
    Ok(None)
}

pub fn prefix_to_netmask(prefix: u8) -> String {
    match prefix {
        0 => "0.0.0.0".into(),
        p if p <= 32 => {
            let bits = u32::MAX << (32 - p);
            format!(
                "{}.{}.{}.{}",
                bits >> 24,
                (bits >> 16) & 0xff,
                (bits >> 8) & 0xff,
                bits & 0xff
            )
        }
        _ => "255.255.255.255".into(),
    }
}

pub fn split_cidr(cidr: &str) -> Result<(String, String)> {
    let (net, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| Error::Route(format!("bad CIDR: {cidr}")))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| Error::Route(format!("bad CIDR prefix: {cidr}")))?;
    if prefix > 32 {
        return Err(Error::Route(format!("bad CIDR prefix: {cidr}")));
    }
    Ok((net.to_string(), prefix_to_netmask(prefix)))
}

/// Find the networksetup service bound to an interface, e.g. en0 -> Wi-Fi.
fn service_for_iface(iface: &str) -> Option<String> {
    let out = Command::new("networksetup")
        .args(["-listnetworkserviceorder"])
        .output()
        .ok()?;
    parse_service_order(&String::from_utf8_lossy(&out.stdout), iface)
}

fn parse_service_order(text: &str, iface: &str) -> Option<String> {
    let mut current_service: Option<&str> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('(') {
            let Some(close) = rest.find(')') else {
                continue;
            };
            let inner = rest[..close].trim();
            let after = rest[close + 1..].trim();
            if inner.starts_with("Hardware Port:") {
                let device = inner.split("Device:").nth(1).map(str::trim).unwrap_or("");
                if device == iface {
                    return current_service.map(str::to_string);
                }
            } else {
                current_service = Some(after);
            }
        }
    }
    None
}

fn apply_dns(plan: &RoutePlan, phys: &PhysicalState) {
    if plan.dns_mode == "ignore" || plan.dns.is_empty() {
        return;
    }
    let Some(iface) = phys.iface.as_ref() else {
        return;
    };
    let Some(service) = service_for_iface(iface) else {
        return;
    };
    let mut cmd = Command::new("networksetup");
    cmd.arg("-setdnsservers").arg(&service);
    cmd.args(&plan.dns);
    if cmd.status().map(|s| s.success()).unwrap_or(false) {
        tracing::info!(iface, service = %service, "VPN DNS applied");
    }
}

fn revert_dns(plan: &RoutePlan, phys: &PhysicalState) {
    if plan.dns_mode == "ignore" || plan.dns.is_empty() {
        return;
    }
    let Some(iface) = phys.iface.as_ref() else {
        return;
    };
    let Some(service) = service_for_iface(iface) else {
        return;
    };
    let _ = Command::new("networksetup")
        .args(["-setdnsservers", &service, "empty"])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netmask_and_cidr_conversion() {
        assert_eq!(prefix_to_netmask(24), "255.255.255.0");
        assert_eq!(prefix_to_netmask(18), "255.255.192.0");
        assert_eq!(
            split_cidr("192.168.0.0/18").unwrap(),
            ("192.168.0.0".to_string(), "255.255.192.0".to_string())
        );
        assert!(split_cidr("10.0.0.0/99").is_err());
    }

    #[test]
    fn parses_networksetup_service_order() {
        let text = "(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n\n(2) USB 10/100/1000 LAN\n(Hardware Port: USB 10/100/1000 LAN, Device: en5)\n";
        assert_eq!(parse_service_order(text, "en0").as_deref(), Some("Wi-Fi"));
        assert_eq!(
            parse_service_order(text, "en5").as_deref(),
            Some("USB 10/100/1000 LAN")
        );
        assert_eq!(parse_service_order(text, "en7"), None);
    }
}
