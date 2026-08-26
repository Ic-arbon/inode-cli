//! Pure parsing of the vpnc-script environment into a route plan.
//!
//! Keeping this module free of `Command` makes the route policy unit-testable
//! on every platform.

use crate::Error;
use crate::Result;
use std::env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePlan {
    pub reason: String,
    pub tun_iface: String,
    pub vpn_gateway: Option<String>,
    pub tun_ip: Option<String>,
    pub prefix_len: Option<u8>,
    pub mtu: Option<u16>,
    pub dns: Vec<String>,
    pub dns_mode: String,
    pub split_routes: Vec<String>,
    pub preserve_cidrs: Vec<String>,
    /// Test override: physical default gateway (skips `ip route` discovery).
    pub phys_gateway_override: Option<String>,
    /// Test override: physical interface (skips `ip route` discovery).
    pub phys_iface_override: Option<String>,
}

impl RoutePlan {
    pub fn from_env(reason: &str) -> Result<Self> {
        let tun_iface = env::var("TUNDEV").unwrap_or_else(|_| "tun0".into());
        let vpn_gateway = env::var("VPNGATEWAY").ok().filter(|v| !v.is_empty());
        let tun_ip = env::var("INTERNAL_IP4_ADDRESS")
            .ok()
            .filter(|v| !v.is_empty());
        let prefix_len = env::var("INTERNAL_IP4_NETMASK")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| netmask_to_prefix(&v))
            .transpose()?;
        let mtu = env::var("INTERNAL_IP4_MTU")
            .ok()
            .and_then(|v| v.parse::<u16>().ok());
        let dns = env::var("INTERNAL_IP4_DNS")
            .ok()
            .map(|v| v.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        let dns_mode = env::var("INODE_DNS_MODE")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "server".into());
        let split_routes = parse_split_routes()?;
        let preserve_cidrs = env::var("INODE_PRESERVE_CIDRS")
            .ok()
            .map(|v| {
                v.split([',', ';', ' '])
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Self {
            reason: reason.to_string(),
            tun_iface,
            vpn_gateway,
            tun_ip,
            prefix_len,
            mtu,
            dns,
            dns_mode,
            split_routes,
            preserve_cidrs,
            phys_gateway_override: env::var("INODE_PHYS_GATEWAY").ok(),
            phys_iface_override: env::var("INODE_PHYS_IFACE").ok(),
        })
    }

    pub fn is_up_phase(&self) -> bool {
        matches!(self.reason.as_str(), "connect" | "reconnect")
    }

    pub fn tun_cidr(&self) -> Option<String> {
        match (&self.tun_ip, self.prefix_len) {
            (Some(ip), Some(prefix)) => Some(format!("{ip}/{prefix}")),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "reason={} tun={} tun_ip={:?}/{:?} mtu={:?} dns={:?} dns_mode={} split={:?} preserve={:?} vpn_gateway={:?}",
            self.reason,
            self.tun_iface,
            self.tun_ip,
            self.prefix_len,
            self.mtu,
            self.dns,
            self.dns_mode,
            self.split_routes,
            self.preserve_cidrs,
            self.vpn_gateway
        )
    }
}

fn parse_split_routes() -> Result<Vec<String>> {
    let count = env::var("CISCO_SPLIT_INC")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut routes = Vec::new();
    for i in 0..count {
        let addr = env::var(format!("CISCO_SPLIT_INC_{i}_ADDR")).unwrap_or_default();
        let masklen = env::var(format!("CISCO_SPLIT_INC_{i}_MASKLEN")).unwrap_or_default();
        if addr.is_empty() || masklen.is_empty() {
            continue;
        }
        if masklen.parse::<u8>().is_err() {
            return Err(Error::Route(format!(
                "bad CISCO_SPLIT_INC_{i}_MASKLEN value: {masklen:?}"
            )));
        }
        routes.push(format!("{addr}/{masklen}"));
    }
    Ok(routes)
}

/// `255.255.255.0` -> `24`. Accepts a prefix length string as well (`"24"`).
pub fn netmask_to_prefix(netmask: &str) -> Result<u8> {
    if let Ok(prefix) = netmask.parse::<u8>() {
        if prefix <= 32 {
            return Ok(prefix);
        }
    }
    let octets: Vec<u8> = netmask
        .split('.')
        .map(|o| {
            o.parse::<u8>()
                .map_err(|_| Error::Route(format!("bad netmask: {netmask:?}")))
        })
        .collect::<Result<_>>()?;
    if octets.len() != 4 {
        return Err(Error::Route(format!("bad netmask: {netmask:?}")));
    }
    let mut bits = 0u32;
    for (i, octet) in octets.iter().enumerate() {
        bits |= u32::from(*octet) << (24 - 8 * i);
    }
    if bits == 0 {
        return Ok(0);
    }
    let prefix = (!bits).leading_zeros();
    if bits | ((1u32 << prefix) - 1) != u32::MAX {
        return Err(Error::Route(format!("non-contiguous netmask: {netmask:?}")));
    }
    Ok(prefix as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Process env is shared by all test threads; serialise env-mutating tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env(kvs: &[(&str, &str)], f: impl FnOnce()) {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut guard = Vec::new();
        for (k, v) in kvs {
            if env::var(k).is_ok() {
                guard.push((*k, Some(env::var(k).unwrap())));
            } else {
                guard.push((*k, None));
            }
            env::set_var(k, v);
        }
        f();
        for (k, v) in guard {
            match v {
                Some(v) => env::set_var(k, v),
                None => env::remove_var(k),
            }
        }
    }

    #[test]
    fn netmask_conversion() {
        assert_eq!(netmask_to_prefix("255.255.255.0").unwrap(), 24);
        assert_eq!(netmask_to_prefix("255.255.192.0").unwrap(), 18);
        assert_eq!(netmask_to_prefix("24").unwrap(), 24);
        assert_eq!(netmask_to_prefix("0.0.0.0").unwrap(), 0);
        assert!(netmask_to_prefix("255.0.255.0").is_err());
    }

    #[test]
    fn parses_live_gateway_environment() {
        with_env(
            &[
                ("TUNDEV", "tun0"),
                ("INTERNAL_IP4_ADDRESS", "10.1.1.20"),
                ("INTERNAL_IP4_NETMASK", "255.255.255.0"),
                ("INTERNAL_IP4_MTU", "1400"),
                ("INTERNAL_IP4_DNS", "223.5.5.5 223.6.6.6"),
                ("CISCO_SPLIT_INC", "1"),
                ("CISCO_SPLIT_INC_0_ADDR", "192.168.0.0"),
                ("CISCO_SPLIT_INC_0_MASKLEN", "18"),
                ("VPNGATEWAY", "140.206.103.26"),
                ("INODE_PRESERVE_CIDRS", "192.168.0.0/23"),
            ],
            || {
                let p = RoutePlan::from_env("connect").unwrap();
                assert_eq!(p.tun_cidr().as_deref(), Some("10.1.1.20/24"));
                assert_eq!(p.mtu, Some(1400));
                assert_eq!(p.dns, vec!["223.5.5.5", "223.6.6.6"]);
                assert_eq!(p.split_routes, vec!["192.168.0.0/18"]);
                assert_eq!(p.preserve_cidrs, vec!["192.168.0.0/23"]);
                assert!(p.is_up_phase());
            },
        );
    }

    #[test]
    fn disconnect_parses_without_ip() {
        with_env(
            &[("TUNDEV", "tun0"), ("VPNGATEWAY", "140.206.103.26")],
            || {
                let p = RoutePlan::from_env("disconnect").unwrap();
                assert!(!p.is_up_phase());
                assert!(p.tun_ip.is_none());
            },
        );
    }
}
