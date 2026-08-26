//! Minimal hand-written FFI bindings for `libopenconnect-h3c`.
//!
//! Only the API surface inode-vpnd needs is declared. Layout-sensitive
//! structs mirror `openconnect.h` and must be kept in sync with the fork.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_uint, c_void};

pub type OpenConnectInfo = c_void;

pub const PRG_ERR: c_int = 0;
pub const PRG_INFO: c_int = 1;
pub const PRG_DEBUG: c_int = 2;
pub const PRG_TRACE: c_int = 3;

pub const OC_FORM_OPT_TEXT: c_int = 1;
pub const OC_FORM_OPT_PASSWORD: c_int = 2;

pub const OC_CMD_CANCEL: u8 = b'x';
pub const OC_CMD_PAUSE: u8 = b'p';
pub const OC_CMD_DETACH: u8 = b'd';
pub const OC_CMD_STATS: u8 = b's';

pub type ValidatePeerCertFn =
    Option<unsafe extern "C" fn(privdata: *mut c_void, reason: *const c_char) -> c_int>;
pub type WriteNewConfigFn =
    Option<unsafe extern "C" fn(privdata: *mut c_void, buf: *const c_char, buflen: c_int) -> c_int>;
pub type ProcessAuthFormFn =
    Option<unsafe extern "C" fn(privdata: *mut c_void, form: *mut OcAuthForm) -> c_int>;
pub type ProgressFn =
    Option<unsafe extern "C" fn(privdata: *mut c_void, level: c_int, fmt: *const c_char, ...)>;
pub type ReconnectedFn = Option<unsafe extern "C" fn(privdata: *mut c_void)>;
pub type StatsFn = Option<unsafe extern "C" fn(privdata: *mut c_void, stats: *const OcStats)>;

#[repr(C)]
pub struct OcFormOpt {
    pub next: *mut OcFormOpt,
    pub form_type: c_int,
    pub name: *mut c_char,
    pub label: *mut c_char,
    pub value: *mut c_char,
    pub flags: c_uint,
    pub reserved: *mut c_void,
}

#[repr(C)]
pub struct OcAuthForm {
    pub banner: *mut c_char,
    pub message: *mut c_char,
    pub error: *mut c_char,
    pub auth_id: *mut c_char,
    pub method: *mut c_char,
    pub action: *mut c_char,
    pub opts: *mut OcFormOpt,
    pub authgroup_opt: *mut c_void,
    pub authgroup_selection: c_int,
}

#[repr(C)]
pub struct OcVpnOption {
    pub option: *mut c_char,
    pub value: *mut c_char,
    pub next: *mut OcVpnOption,
}

#[repr(C)]
pub struct OcIpInfo {
    pub addr: *const c_char,
    pub netmask: *const c_char,
    pub addr6: *const c_char,
    pub netmask6: *const c_char,
    pub dns: [*const c_char; 3],
    pub nbns: [*const c_char; 3],
    pub domain: *const c_char,
    pub proxy_pac: *const c_char,
    pub mtu: c_int,
    pub split_dns: *mut c_void,
    pub split_includes: *mut c_void,
    pub split_excludes: *mut c_void,
    pub gateway_addr: *mut c_char,
}

#[repr(C)]
pub struct OcStats {
    pub tx_pkts: u64,
    pub tx_bytes: u64,
    pub rx_pkts: u64,
    pub rx_bytes: u64,
}

#[repr(C)]
pub struct OcCallbacks {
    pub progress: Option<unsafe extern "C" fn(*mut c_void, c_int, *const c_char)>,
}

unsafe extern "C" {
    pub fn inode_oc_progress_shim(privdata: *mut c_void, level: c_int, fmt: *const c_char, ...);
}

#[link(name = "openconnect")]
unsafe extern "C" {
    pub fn openconnect_get_version() -> *const c_char;
    pub fn openconnect_init_ssl() -> c_int;
    pub fn openconnect_vpninfo_new(
        useragent: *const c_char,
        validate_peer_cert: ValidatePeerCertFn,
        write_new_config: WriteNewConfigFn,
        process_auth_form: ProcessAuthFormFn,
        progress: ProgressFn,
        privdata: *mut c_void,
    ) -> *mut OpenConnectInfo;
    pub fn openconnect_vpninfo_free(vpninfo: *mut OpenConnectInfo);
    pub fn openconnect_set_loglevel(vpninfo: *mut OpenConnectInfo, level: c_int);
    pub fn openconnect_set_protocol(
        vpninfo: *mut OpenConnectInfo,
        protocol: *const c_char,
    ) -> c_int;
    pub fn openconnect_parse_url(vpninfo: *mut OpenConnectInfo, url: *const c_char) -> c_int;
    pub fn openconnect_disable_dtls(vpninfo: *mut OpenConnectInfo) -> c_int;
    pub fn openconnect_set_drop_uid(vpninfo: *mut OpenConnectInfo, uid: u32, gid: u32) -> c_int;
    pub fn openconnect_setup_cmd_pipe(vpninfo: *mut OpenConnectInfo) -> c_int;
    pub fn openconnect_set_option_value(opt: *mut OcFormOpt, value: *const c_char) -> c_int;
    pub fn openconnect_check_peer_cert_hash(
        vpninfo: *mut OpenConnectInfo,
        fingerprint: *const c_char,
    ) -> c_int;
    pub fn openconnect_get_peer_cert_hash(vpninfo: *mut OpenConnectInfo) -> *const c_char;
    pub fn openconnect_get_cookie(vpninfo: *mut OpenConnectInfo) -> *const c_char;
    pub fn openconnect_obtain_cookie(vpninfo: *mut OpenConnectInfo) -> c_int;
    pub fn openconnect_make_cstp_connection(vpninfo: *mut OpenConnectInfo) -> c_int;
    pub fn openconnect_get_ip_info(
        vpninfo: *mut OpenConnectInfo,
        ip_info: *mut *const OcIpInfo,
        cstp_options: *mut *const OcVpnOption,
        dtls_options: *mut *const OcVpnOption,
    ) -> c_int;
    pub fn openconnect_setup_tun_device(
        vpninfo: *mut OpenConnectInfo,
        vpnc_script: *const c_char,
        ifname: *const c_char,
    ) -> c_int;
    pub fn openconnect_setup_tun_script(
        vpninfo: *mut OpenConnectInfo,
        tun_script: *const c_char,
    ) -> c_int;
    pub fn openconnect_mainloop(
        vpninfo: *mut OpenConnectInfo,
        reconnect_timeout: c_int,
        reconnect_interval: c_int,
    ) -> c_int;
    pub fn openconnect_set_reconnected_handler(
        vpninfo: *mut OpenConnectInfo,
        handler: ReconnectedFn,
    );
    pub fn openconnect_set_stats_handler(vpninfo: *mut OpenConnectInfo, handler: StatsFn);
}
