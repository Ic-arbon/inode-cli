//! Linux route/DNS executor for inode-routectl (M3).
//!
//! `connect/reconnect` persists the discovered physical path so that
//! `disconnect` can remove routes even after the routing table changed.

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
    local_prefix: Option<String>,
}

impl PhysicalState {
    fn discover(plan: &RoutePlan) -> Result<Self> {
        let gateway = plan
            .phys_gateway_override
            .clone()
            .or_else(|| parse_default_route_field("via").ok().flatten());
        let iface = plan
            .phys_iface_override
            .clone()
            .or_else(|| parse_default_route_field("dev").ok().flatten());

        let (local_ip, local_prefix) = match &iface {
            Some(iface) => parse_local_addr(iface)
                .ok()
                .flatten()
                .map(|(ip, prefix)| (Some(ip), Some(prefix)))
                .unwrap_or((None, None)),
            None => (None, None),
        };

        Ok(Self {
            gateway,
            iface,
            local_ip,
            local_prefix,
        })
    }
}

pub fn apply(plan: &RoutePlan) -> Result<()> {
    match plan.reason.as_str() {
        "pre-init" => Ok(()),
        "connect" | "reconnect" => connect(plan),
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
            "/run/inode-vpn".into()
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

fn connect(plan: &RoutePlan) -> Result<()> {
    let phys = PhysicalState::discover(plan)?;
    save_state(plan, &phys);

    if let (Some(ip), Some(prefix)) = (&plan.tun_ip, plan.prefix_len) {
        // Point-to-point tun: local IP with the server-provided netmask as peer.
        run(&[
            "addr",
            "replace",
            &format!("{ip}/{prefix}"),
            "peer",
            &format!("{ip}/{prefix}"),
            "dev",
            &plan.tun_iface,
        ])?;
        run(&["link", "set", "dev", &plan.tun_iface, "up"])?;
    }
    if let Some(mtu) = plan.mtu {
        let _ = run(&[
            "link",
            "set",
            "dev",
            &plan.tun_iface,
            "mtu",
            &mtu.to_string(),
        ]);
    }

    // 1. Keep the VPN control path alive even if the tunnel covers it.
    if let (Some(gw), Some(phys_gw), Some(phys_if)) =
        (&plan.vpn_gateway, &phys.gateway, &phys.iface)
    {
        run(&[
            "route",
            "replace",
            &format!("{gw}/32"),
            "via",
            phys_gw,
            "dev",
            phys_if,
        ])?;
    }

    // 2. Protect the management network: more specific than any VPN route.
    if let (Some(phys_gw), Some(phys_if)) = (&phys.gateway, &phys.iface) {
        if let Some(local_ip) = &phys.local_ip {
            run(&[
                "route",
                "replace",
                &format!("{local_ip}/32"),
                "via",
                phys_gw,
                "dev",
                phys_if,
            ])?;
        }
        if let Some(prefix) = &phys.local_prefix {
            run(&["route", "replace", prefix, "via", phys_gw, "dev", phys_if])?;
        }
        for cidr in &plan.preserve_cidrs {
            run(&["route", "replace", cidr, "via", phys_gw, "dev", phys_if])?;
        }
    }

    // 3. Split routes from NET_EXTEND ROUTES go through the tun.
    for cidr in &plan.split_routes {
        run(&["route", "replace", cidr, "dev", &plan.tun_iface])?;
    }

    apply_dns(plan);
    Ok(())
}

fn disconnect(plan: &RoutePlan) -> Result<()> {
    let phys = {
        let saved = load_state(plan);
        if saved.gateway.is_some() || saved.iface.is_some() {
            saved
        } else {
            PhysicalState::discover(plan)?
        }
    };

    for cidr in &plan.split_routes {
        let _ = run(&["route", "del", cidr, "dev", &plan.tun_iface]);
    }
    if let (Some(gw), Some(phys_gw), Some(phys_if)) =
        (&plan.vpn_gateway, &phys.gateway, &phys.iface)
    {
        let _ = run(&[
            "route",
            "del",
            &format!("{gw}/32"),
            "via",
            phys_gw,
            "dev",
            phys_if,
        ]);
    }
    if let (Some(phys_gw), Some(phys_if)) = (&phys.gateway, &phys.iface) {
        if let Some(local_ip) = &phys.local_ip {
            let _ = run(&[
                "route",
                "del",
                &format!("{local_ip}/32"),
                "via",
                phys_gw,
                "dev",
                phys_if,
            ]);
        }
        if let Some(prefix) = &phys.local_prefix {
            let _ = run(&["route", "del", prefix, "via", phys_gw, "dev", phys_if]);
        }
        for cidr in &plan.preserve_cidrs {
            let _ = run(&["route", "del", cidr, "via", phys_gw, "dev", phys_if]);
        }
    }

    if plan.tun_ip.is_some() {
        let _ = run(&["link", "set", "dev", &plan.tun_iface, "down"]);
    }
    revert_dns(plan);
    let _ = std::fs::remove_file(state_path(plan));
    Ok(())
}

fn run(args: &[&str]) -> Result<()> {
    let output = Command::new("ip")
        .args(args)
        .output()
        .map_err(|e| Error::Route(format!("failed to run `ip {}`: {e}", args.join(" "))))?;
    if output.status.success() {
        tracing::debug!(cmd = %format!("ip {}", args.join(" ")), "route command ok");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Route(format!(
            "`ip {}` exited {}: {}",
            args.join(" "),
            output.status,
            stderr.trim()
        )))
    }
}

/// Parse `via <gateway>` or `dev <iface>` from `ip route show default`.
fn parse_default_route_field(field: &str) -> Result<Option<String>> {
    let out = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .map_err(|e| Error::Route(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut words = text.split_whitespace();
    while let Some(word) = words.next() {
        if word == field {
            return Ok(words.next().map(str::to_string));
        }
    }
    Ok(None)
}

fn parse_local_addr(iface: &str) -> Result<Option<(String, String)>> {
    let out = Command::new("ip")
        .args(["-4", "-o", "addr", "show", "dev", iface])
        .output()
        .map_err(|e| Error::Route(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut words = line.split_whitespace();
        while let Some(word) = words.next() {
            if word == "inet" {
                let cidr = words.next().unwrap_or_default();
                let (ip, prefix) = cidr.split_once('/').unwrap_or((cidr, ""));
                return Ok(Some((ip.to_string(), network_cidr(ip, prefix))));
            }
        }
    }
    Ok(None)
}

/// Normalise `192.168.0.128/23` to `192.168.0.0/23`; `ip route replace`
/// rejects prefixes whose host bits are non-zero.
fn network_cidr(ip: &str, prefix: &str) -> String {
    let bits = prefix.parse::<u32>().unwrap_or(32).min(32);
    match ip.parse::<std::net::Ipv4Addr>() {
        Ok(addr) => {
            let host = u32::from(addr);
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            format!("{}/{bits}", std::net::Ipv4Addr::from(host & mask))
        }
        Err(_) => format!("{ip}/{prefix}"),
    }
}

fn apply_dns(plan: &RoutePlan) {
    if plan.dns_mode == "ignore" || plan.dns.is_empty() {
        return;
    }
    if let Ok(out) = Command::new("resolvectl")
        .args(["dns", &plan.tun_iface])
        .args(&plan.dns)
        .output()
    {
        if out.status.success() {
            tracing::info!(iface = %plan.tun_iface, dns = ?plan.dns, "VPN DNS applied via resolvectl");
            return;
        }
    }
    if let Ok(out) = Command::new("resolvconf")
        .args(["-a", &format!("{}.openconnect", plan.tun_iface)])
        .output()
    {
        if out.status.success() {
            tracing::info!("VPN DNS applied via resolvconf");
        }
    }
}

fn revert_dns(plan: &RoutePlan) {
    let _ = Command::new("resolvectl")
        .args(["revert", &plan.tun_iface])
        .output();
    let _ = Command::new("resolvconf")
        .args(["-d", &format!("{}.openconnect", plan.tun_iface)])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_cidr_normalises_host_bits() {
        assert_eq!(network_cidr("192.168.0.128", "23"), "192.168.0.0/23");
        assert_eq!(network_cidr("10.1.2.3", "24"), "10.1.2.0/24");
        assert_eq!(network_cidr("10.1.2.3", "0"), "0.0.0.0/0");
        assert_eq!(network_cidr("10.1.2.3", "33"), "10.1.2.3/32");
    }

    #[test]
    fn state_round_trip() {
        let plan = RoutePlan {
            reason: "connect".into(),
            tun_iface: "tun-test".into(),
            vpn_gateway: None,
            tun_ip: None,
            prefix_len: None,
            mtu: None,
            dns: vec![],
            dns_mode: "server".into(),
            split_routes: vec![],
            preserve_cidrs: vec![],
            phys_gateway_override: None,
            phys_iface_override: None,
        };
        let state = PhysicalState {
            gateway: Some("192.168.1.1".into()),
            iface: Some("wlan0".into()),
            local_ip: Some("192.168.1.23".into()),
            local_prefix: Some("192.168.1.23/24".into()),
        };
        save_state(&plan, &state);
        let loaded = load_state(&plan);
        assert_eq!(loaded.gateway.as_deref(), Some("192.168.1.1"));
        assert_eq!(loaded.local_prefix.as_deref(), Some("192.168.1.23/24"));
        let _ = std::fs::remove_file(state_path(&plan));
    }
}
