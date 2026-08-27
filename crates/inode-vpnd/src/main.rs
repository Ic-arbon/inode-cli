//! `inode-vpnd` — root VPN daemon (M2).
//!
//! Engine runs libopenconnect in a dedicated thread; an IPC thread serves
//! JSON-RPC 2.0 over a Unix domain socket.

use clap::Parser;
use inode_core::ipc::{self, Event, Request, Response};
use inode_core::redact::Redactor;
use inode_core::state::{
    HealthStatus, ServiceStatus, SessionInfo, SessionState, Stats, StatusSnapshot,
};
use inode_core::{Config, Error, Result};
use inode_openconnect_sys::ffi as oc;
use serde_json::json;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::{BufReader, Write};
use std::os::fd::RawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

mod netwatch;

#[derive(Debug, Parser)]
#[command(name = "inode-vpnd", version, about = "inode-vpn root daemon")]
struct Cli {
    /// Target user id whose config and VPN session this daemon manages.
    #[arg(long)]
    uid: Option<u32>,

    /// Configuration file (default: ~/.config/inode-vpn/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,

    /// IPC socket override (tests).
    #[arg(long, hide = true)]
    socket: Option<PathBuf>,

    /// Run one foreground engine cycle and exit (M0 smoke test only).
    #[arg(long, hide = true)]
    smoke: bool,

    /// Use openconnect script-tun with this command instead of a real tun
    /// device (M2 tests; production uses setup_tun_device).
    #[arg(long, hide = true)]
    tun_script: Option<String>,
}

#[derive(Clone)]
struct Shared {
    inner: Arc<Mutex<Inner>>,
    subscribers: Arc<Mutex<Vec<mpsc::Sender<Event>>>>,
    redactor: Arc<Mutex<Redactor>>,
    health_stop: Arc<AtomicBool>,
    health_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

struct Inner {
    state: SessionState,
    since: Option<String>,
    last_error: Option<String>,
    gateway: Option<String>,
    session: Option<SessionInfo>,
    stats: Stats,
    health: Option<HealthStatus>,
    cookie: Option<String>,
    config_path: PathBuf,
    config: Option<Config>,
    target_uid: u32,
    target_gid: u32,
    stop_requested: bool,
    last_net_event: Option<Instant>,
    cmd_fd: Option<RawFd>,
    engine: Option<JoinHandle<()>>,
    tun_script: Option<String>,
}

impl Shared {
    fn new(
        target_uid: u32,
        target_gid: u32,
        config_path: PathBuf,
        tun_script: Option<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: SessionState::Stopped,
                since: None,
                last_error: None,
                gateway: None,
                session: None,
                stats: Stats::default(),
                health: None,
                cookie: None,
                config_path,
                config: None,
                target_uid,
                target_gid,
                stop_requested: false,
                last_net_event: None,
                cmd_fd: None,
                engine: None,
                tun_script,
            })),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            redactor: Arc::new(Mutex::new(Redactor::default())),
            health_stop: Arc::new(AtomicBool::new(false)),
            health_thread: Arc::new(Mutex::new(None)),
        }
    }

    fn now() -> String {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => format!("{}", d.as_secs()),
            Err(_) => "0".into(),
        }
    }

    fn emit(&self, method: &str, params: serde_json::Value) {
        let event = Event::new(method, params);
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(event.clone()).is_ok());
    }

    fn set_state(&self, state: SessionState, last_error: Option<String>) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.state != state {
                inner.state = state;
                inner.since = Some(Self::now());
            }
            if last_error.is_some() {
                inner.last_error = last_error;
            }
        }
        self.persist_state();
        self.emit("state_changed", self.snapshot_json());
    }

    /// Crash-recovery breadcrumb. systemd/launchd restart the daemon; the
    /// restart path reads config.service.autostart and re-enters the engine.
    fn persist_state(&self) {
        let inner = self.inner.lock().unwrap();
        let path = state_file_for(&inner.config_path);
        let payload = json!({
            "state": inner.state,
            "since": inner.since,
            "last_error": inner.last_error,
            "autostart": inner.config.as_ref().map(|c| c.service.autostart).unwrap_or(false),
        });
        let Ok(text) = serde_json::to_string(&payload) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::File::create(&path) {
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::PermissionsExt;
                let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
                let _ = file.write_all(text.as_bytes());
            }
            #[cfg(not(unix))]
            {
                use std::io::Write;
                let _ = file.write_all(text.as_bytes());
            }
        }
    }

    fn set_connected_info(&self, gateway: String, session: SessionInfo) {
        self.inner.lock().unwrap().gateway = Some(gateway);
        self.inner.lock().unwrap().session = Some(session);
        self.emit("session_updated", self.snapshot_json());
    }

    fn store_cmd_fd(&self, fd: RawFd) {
        self.inner.lock().unwrap().cmd_fd = Some(fd);
    }

    fn set_engine(&self, handle: Option<JoinHandle<()>>) {
        self.inner.lock().unwrap().engine = handle;
    }

    /// Wait until the engine thread has dropped its handle (normal exit or
    /// panic). Returns false if it is still running after `timeout`.
    fn wait_for_engine_finish(&self, timeout: std::time::Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.inner.lock().unwrap().engine.is_none() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn stop_requested(&self) -> bool {
        self.inner.lock().unwrap().stop_requested
    }

    fn request_stop(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.stop_requested = true;
        if let Some(fd) = inner.cmd_fd {
            let byte = [oc::OC_CMD_CANCEL];
            unsafe {
                libc::write(fd, byte.as_ptr() as *const c_void, 1);
            }
        }
    }

    /// Network-change trigger.
    ///
    /// * Running engine: send OC_CMD_PAUSE so the mainloop reconnects on the
    ///   same cookie. This is the fast path for a brief outage.
    /// * Failed engine: start a fresh engine from the stored config. This
    ///   recovers after an outage longer than openconnect's reconnect window
    ///   (the H3C DPD path gives up after `reconnect_timeout`).
    ///
    /// Debounced to collapse network-change bursts.
    fn network_changed(self: &Arc<Self>) {
        enum NetAction {
            Pause(RawFd),
            Restart(Config),
        }

        let action = {
            let mut inner = self.inner.lock().unwrap();
            let now = Instant::now();
            if inner
                .last_net_event
                .map(|t| now.duration_since(t).as_secs() < 2)
                .unwrap_or(false)
            {
                return;
            }
            inner.last_net_event = Some(now);

            if matches!(
                inner.state,
                SessionState::Connected | SessionState::Reconnecting
            ) {
                match inner.cmd_fd {
                    Some(fd) => NetAction::Pause(fd),
                    None => return,
                }
            } else if inner.state == SessionState::Failed && inner.engine.is_none() {
                match inner.config.clone() {
                    Some(config) => NetAction::Restart(config),
                    None => return,
                }
            } else {
                return;
            }
        };

        match action {
            NetAction::Pause(fd) => {
                self.set_state(SessionState::Reconnecting, None);
                let byte = [oc::OC_CMD_PAUSE];
                unsafe {
                    libc::write(fd, byte.as_ptr() as *const c_void, 1);
                }
                tracing::info!("network change detected; reconnecting");
            }
            NetAction::Restart(config) => match self.engine_start(config) {
                Ok(()) => tracing::info!("network change detected; restarting failed engine"),
                Err(e) => tracing::warn!("network-triggered engine restart failed: {e}"),
            },
        }
    }

    fn mark_engine_finished(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.cmd_fd = None;
        inner.engine = None;
    }

    fn snapshot(&self) -> StatusSnapshot {
        let inner = self.inner.lock().unwrap();
        StatusSnapshot {
            state: inner.state,
            since: inner.since.clone(),
            gateway: inner.gateway.clone(),
            session: inner.session.clone(),
            stats: Some(inner.stats),
            last_error: inner.last_error.clone(),
            health: inner.health.clone(),
            service: ServiceStatus {
                supervisor: if cfg!(target_os = "macos") {
                    "launchd".into()
                } else {
                    "systemd".into()
                },
                enabled: true,
                autostart: inner
                    .config
                    .as_ref()
                    .map(|c| c.service.autostart)
                    .unwrap_or(false),
            },
        }
    }

    fn snapshot_json(&self) -> serde_json::Value {
        serde_json::to_value(self.snapshot()).unwrap_or(serde_json::Value::Null)
    }

    fn add_redaction_secrets(&self, config: &Config) {
        let mut r = self.redactor.lock().unwrap();
        r.add(config.credentials.password.clone());
        r.add(config.credentials.username.clone());
    }

    fn cookie(&self) -> Option<String> {
        self.inner.lock().unwrap().cookie.clone()
    }

    fn set_cookie(&self, cookie: String) {
        self.inner.lock().unwrap().cookie = Some(cookie);
    }

    fn clear_cookie(&self) {
        self.inner.lock().unwrap().cookie = None;
    }

    fn set_health(&self, ok: bool) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.health = Some(HealthStatus {
                checkonline_ok: ok,
                last_check: Some(Self::now()),
            });
        }
        self.emit("health", self.snapshot_json());
    }

    fn start_health(self: &Arc<Self>, gateway: String, interval_secs: u64) {
        self.health_stop.store(false, Ordering::SeqCst);
        let shared = Arc::clone(self);
        let interval = interval_secs.max(5);
        let handle = thread::Builder::new()
            .name("inode-health".into())
            .spawn(move || health_loop(shared, gateway, interval))
            .expect("spawn health thread");
        *self.health_thread.lock().unwrap() = Some(handle);
    }

    fn stop_health(&self) {
        self.health_stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.health_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
        self.inner.lock().unwrap().health = None;
        self.clear_cookie();
    }

    /// Spawn the engine thread for `config`. Returns Ok(()) if accepted.
    fn engine_start(self: &Arc<Self>, config: Config) -> Result<()> {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.engine.is_some() {
                return Err(Error::Ipc("engine already running".into()));
            }
            inner.stop_requested = false;
            inner.last_error = None;
            inner.config = Some(config.clone());
        }
        self.add_redaction_secrets(&config);
        let shared = Arc::clone(self);
        let tun_script = {
            let inner = self.inner.lock().unwrap();
            inner.tun_script.clone()
        };
        let handle = thread::Builder::new()
            .name("inode-engine".into())
            .spawn(move || engine_run(Arc::clone(&shared), config, tun_script))
            .map_err(|e| Error::Ipc(format!("spawn engine failed: {e}")))?;
        self.set_engine(Some(handle));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// libopenconnect callbacks

#[repr(C)]
struct CallbackCtx {
    /// First field: C shim `inode_oc_progress_shim` dereferences this.
    callbacks: oc::OcCallbacks,
    shared: Arc<Shared>,
    vpninfo: *mut oc::OpenConnectInfo,
    username: CString,
    password: CString,
    servercert: CString,
    tun_script: CString,
}

unsafe extern "C" fn progress_cb(privdata: *mut c_void, level: c_int, msg: *const c_char) {
    if privdata.is_null() || msg.is_null() {
        return;
    }
    let ctx = &mut *(privdata as *mut CallbackCtx);
    let text = CStr::from_ptr(msg).to_string_lossy();
    let redacted = {
        let r = ctx.shared.redactor.lock().unwrap();
        r.redact(&text)
    };
    match level {
        oc::PRG_ERR => tracing::error!("{redacted}"),
        oc::PRG_DEBUG => tracing::debug!("{redacted}"),
        oc::PRG_TRACE => tracing::trace!("{redacted}"),
        _ => tracing::info!("{redacted}"),
    }
}

unsafe extern "C" fn validate_cb(privdata: *mut c_void, reason: *const c_char) -> c_int {
    let ctx = &mut *(privdata as *mut CallbackCtx);
    let reason = if reason.is_null() {
        "certificate rejected"
    } else {
        CStr::from_ptr(reason)
            .to_str()
            .unwrap_or("certificate rejected")
    };
    tracing::warn!(reason, "server certificate requires pin validation");
    if ctx.servercert.as_bytes().is_empty() {
        return 1;
    }
    oc::openconnect_check_peer_cert_hash(ctx.vpninfo, ctx.servercert.as_ptr())
}

unsafe extern "C" fn auth_cb(privdata: *mut c_void, form: *mut oc::OcAuthForm) -> c_int {
    if privdata.is_null() || form.is_null() {
        return -1;
    }
    let ctx = &mut *(privdata as *mut CallbackCtx);
    let mut opt = (*form).opts;
    while !opt.is_null() {
        let name = CStr::from_ptr((*opt).name).to_string_lossy();
        match name.as_ref() {
            "username" => {
                if oc::openconnect_set_option_value(opt, ctx.username.as_ptr()) != 0 {
                    return -1;
                }
            }
            "password" => {
                if oc::openconnect_set_option_value(opt, ctx.password.as_ptr()) != 0 {
                    return -1;
                }
            }
            _ => {}
        }
        opt = (*opt).next;
    }
    0
}

unsafe extern "C" fn reconnected_cb(privdata: *mut c_void) {
    if privdata.is_null() {
        return;
    }
    let ctx = &*(privdata as *mut CallbackCtx);
    tracing::info!("openconnect session reconnected");
    ctx.shared.set_state(SessionState::Connected, None);
}

unsafe extern "C" fn stats_cb(privdata: *mut c_void, stats: *const oc::OcStats) {
    if privdata.is_null() || stats.is_null() {
        return;
    }
    let ctx = &*(privdata as *mut CallbackCtx);
    let s = &*stats;
    {
        let mut inner = ctx.shared.inner.lock().unwrap();
        inner.stats = Stats {
            tx_pkts: s.tx_pkts,
            tx_bytes: s.tx_bytes,
            rx_pkts: s.rx_pkts,
            rx_bytes: s.rx_bytes,
        };
    }
    ctx.shared.emit("stats", ctx.shared.snapshot_json());
}

// ---------------------------------------------------------------------------
// Engine thread

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::OpenConnect("string contains NUL".into()))
}

fn checkonline_url(gateway: &str) -> String {
    format!("https://{gateway}/_xml/checkonline.cgi")
}

/// `curl -b -` reads a Netscape cookie file from stdin, so the svpnginfo
/// cookie never appears in argv or logs.
fn curl_checkonline(gateway: &str, cookie: &str) -> bool {
    let Some((_, value)) = cookie.split_once('=') else {
        return false;
    };
    let host = gateway.split(':').next().unwrap_or(gateway);
    let cookie_file = format!("{host}\tFALSE\t/\tFALSE\t0\tsvpnginfo\t{value}\n");
    let url = checkonline_url(gateway);

    let Ok(mut child) = std::process::Command::new("curl")
        .args([
            "-ks",
            "--max-time",
            "5",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-b",
            "-",
            &url,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(cookie_file.as_bytes());
    }
    let Ok(output) = child.wait_with_output() else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).trim() == "200"
}

fn health_loop(shared: Arc<Shared>, gateway: String, interval_secs: u64) {
    let interval = std::time::Duration::from_secs(interval_secs);
    while !shared.health_stop.load(Ordering::SeqCst) {
        thread::sleep(interval);
        if shared.health_stop.load(Ordering::SeqCst) {
            break;
        }
        let Some(cookie) = shared.cookie() else {
            continue;
        };
        let ok = curl_checkonline(&gateway, &cookie);
        tracing::debug!(ok, "checkonline probe");
        shared.set_health(ok);
    }
}

fn engine_run(shared: Arc<Shared>, config: Config, tun_script: Option<String>) {
    let _guard = EngineGuard(Arc::clone(&shared));
    shared.set_state(SessionState::Authenticating, None);

    let username = match cstring(&config.credentials.username) {
        Ok(v) => v,
        Err(e) => return shared.set_state(SessionState::Failed, Some(e.to_string())),
    };
    let password = match cstring(&config.credentials.password) {
        Ok(v) => v,
        Err(e) => return shared.set_state(SessionState::Failed, Some(e.to_string())),
    };
    let servercert = match cstring(&config.gateway.servercert) {
        Ok(v) => v,
        Err(e) => return shared.set_state(SessionState::Failed, Some(e.to_string())),
    };
    let url = match cstring(&config.gateway.url) {
        Ok(v) => v,
        Err(e) => return shared.set_state(SessionState::Failed, Some(e.to_string())),
    };
    let script = match cstring(tun_script.as_deref().unwrap_or("")) {
        Ok(v) => v,
        Err(e) => return shared.set_state(SessionState::Failed, Some(e.to_string())),
    };

    let mut ctx = Box::new(CallbackCtx {
        callbacks: oc::OcCallbacks {
            progress: Some(progress_cb),
        },
        shared: shared.clone(),
        vpninfo: std::ptr::null_mut(),
        username,
        password,
        servercert,
        tun_script: script,
    });

    let result: Result<()> = (|| unsafe {
        tracing::info!("engine: openconnect_init_ssl");
        let mut ret = oc::openconnect_init_ssl();
        if ret != 0 {
            return Err(Error::OpenConnect(format!(
                "openconnect_init_ssl failed ({ret})"
            )));
        }

        let useragent = cstring("inode-vpn")?;
        let progress_fn: oc::ProgressFn = Some(
            oc::inode_oc_progress_shim
                as unsafe extern "C" fn(*mut c_void, c_int, *const c_char, ...),
        );
        let vpninfo = oc::openconnect_vpninfo_new(
            useragent.as_ptr(),
            Some(validate_cb),
            None,
            Some(auth_cb),
            progress_fn,
            &mut *ctx as *mut CallbackCtx as *mut c_void,
        );
        tracing::info!(vpninfo = %(vpninfo as usize), "engine: vpninfo_new ok");
        if vpninfo.is_null() {
            return Err(Error::OpenConnect("openconnect_vpninfo_new failed".into()));
        }
        ctx.vpninfo = vpninfo;

        oc::openconnect_set_loglevel(vpninfo, oc::PRG_INFO);
        tracing::info!("engine: set_protocol");
        ret = oc::openconnect_set_protocol(vpninfo, c"h3c".as_ptr());
        if ret != 0 {
            return Err(Error::OpenConnect(format!(
                "openconnect_set_protocol failed ({ret})"
            )));
        }
        tracing::info!("engine: parse_url");
        ret = oc::openconnect_parse_url(vpninfo, url.as_ptr());
        if ret != 0 {
            return Err(Error::OpenConnect(format!(
                "openconnect_parse_url failed ({ret})"
            )));
        }
        oc::openconnect_disable_dtls(vpninfo);
        tracing::info!("engine: disable_dtls done");
        let (target_uid, target_gid) = {
            let inner = shared.inner.lock().unwrap();
            (inner.target_uid, inner.target_gid)
        };
        oc::openconnect_set_drop_uid(vpninfo, target_uid, target_gid);
        tracing::info!("engine: drop_uid set");
        oc::openconnect_set_reconnected_handler(vpninfo, Some(reconnected_cb));
        oc::openconnect_set_stats_handler(vpninfo, Some(stats_cb));
        tracing::info!("engine: handlers set");

        let cmd_fd = oc::openconnect_setup_cmd_pipe(vpninfo);
        tracing::info!(cmd_fd, "engine: cmd pipe");
        if cmd_fd < 0 {
            return Err(Error::OpenConnect(
                "openconnect_setup_cmd_pipe failed".into(),
            ));
        }
        shared.store_cmd_fd(cmd_fd);

        tracing::info!("engine: obtain_cookie");
        ret = oc::openconnect_obtain_cookie(vpninfo);
        if ret != 0 {
            return Err(Error::OpenConnect(format!("authentication failed ({ret})")));
        }

        // Copy the session cookie for the checkonline health probe and
        // register it for redaction. Never log or expose the value itself.
        let cookie_ptr = oc::openconnect_get_cookie(vpninfo);
        if !cookie_ptr.is_null() {
            let cookie = CStr::from_ptr(cookie_ptr).to_string_lossy().into_owned();
            shared.redactor.lock().unwrap().add(cookie.clone());
            shared.set_cookie(cookie);
        }
        shared.start_health(
            config.gateway.url.clone(),
            config.network.keepalive.seconds(30),
        );

        shared.set_state(SessionState::Connecting, None);
        tracing::info!("engine: make_cstp_connection");
        ret = oc::openconnect_make_cstp_connection(vpninfo);
        if ret != 0 {
            return Err(Error::OpenConnect(format!("NET_EXTEND failed ({ret})")));
        }

        // Extract session parameters before pointers become invalid.
        let mut ip_ptr: *const oc::OcIpInfo = std::ptr::null();
        let mut cstp: *const oc::OcVpnOption = std::ptr::null();
        let mut dtls: *const oc::OcVpnOption = std::ptr::null();
        let mut session = SessionInfo::default();
        if oc::openconnect_get_ip_info(vpninfo, &mut ip_ptr, &mut cstp, &mut dtls) == 0
            && !ip_ptr.is_null()
        {
            let ip = &*ip_ptr;
            if !ip.addr.is_null() {
                session.ip = Some(CStr::from_ptr(ip.addr).to_string_lossy().into_owned());
            }
            if ip.mtu > 0 {
                session.mtu = Some(ip.mtu as u32);
            }
            for dns in ip.dns.iter() {
                if !dns.is_null() {
                    session
                        .dns
                        .push(CStr::from_ptr(*dns).to_string_lossy().into_owned());
                }
            }
            session.keepalive = Some(30);
        }
        shared.set_connected_info(config.gateway.url.clone(), session);

        // Routectl configuration travels through the process environment and
        // is inherited by the fork's /bin/sh -c child.
        std::env::set_var(
            "INODE_DNS_MODE",
            match config.network.dns {
                inode_core::config::DnsMode::Server => "server",
                inode_core::config::DnsMode::Ignore => "ignore",
            },
        );
        if !config.network.preserve_cidrs.is_empty() {
            std::env::set_var(
                "INODE_PRESERVE_CIDRS",
                config.network.preserve_cidrs.join(","),
            );
        }
        if std::env::var_os("INODE_ROUTECTL_STATE_DIR").is_none() {
            std::env::set_var("INODE_ROUTECTL_STATE_DIR", "/run/inode-vpn");
        }

        if !ctx.tun_script.as_bytes().is_empty() {
            ret = oc::openconnect_setup_tun_script(vpninfo, ctx.tun_script.as_ptr());
        } else {
            // Production path: let the fork exec inode-routectl for
            // connect/reconnect/disconnect. Prefer the binary next to the
            // daemon (Nix puts all three in the same bin dir).
            let routectl = std::env::var_os("INODE_ROUTECTL")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.join("inode-routectl")))
                })
                .ok_or_else(|| Error::OpenConnect("cannot locate inode-routectl".into()))?;
            let script = cstring(routectl.to_string_lossy().as_ref())?;
            ret = oc::openconnect_setup_tun_device(vpninfo, script.as_ptr(), std::ptr::null());
        }
        tracing::info!("engine: tun setup");
        if ret != 0 {
            return Err(Error::OpenConnect(format!("tun setup failed ({ret})")));
        }

        shared.set_state(SessionState::Connected, None);
        tracing::info!("engine entering mainloop");

        loop {
            let r = oc::openconnect_mainloop(vpninfo, 120, 10);
            if shared.stop_requested() {
                shared.set_state(SessionState::Stopped, None);
                break;
            }
            if r == 0 {
                // OC_CMD_PAUSE (network change watcher) closes the data
                // connection; the next mainloop call reconnects.
                if shared.stop_requested() {
                    shared.set_state(SessionState::Stopped, None);
                    break;
                }
                shared.set_state(SessionState::Reconnecting, None);
                continue;
            }
            return Err(Error::OpenConnect(format!("mainloop exited ({r})")));
        }

        Ok(())
    })();

    shared.stop_health();

    unsafe {
        if !ctx.vpninfo.is_null() {
            oc::openconnect_vpninfo_free(ctx.vpninfo);
            ctx.vpninfo = std::ptr::null_mut();
        }
    }

    if let Err(e) = result {
        let msg = e.to_string();
        tracing::error!(error = %msg, "engine stopped");
        shared.set_state(
            if shared.stop_requested() {
                SessionState::Stopped
            } else {
                SessionState::Failed
            },
            Some(msg),
        );
    }
}

/// Ensures `mark_engine_finished()` runs even if the engine panics.
struct EngineGuard(Arc<Shared>);

impl Drop for EngineGuard {
    fn drop(&mut self) {
        self.0.mark_engine_finished();
    }
}

// ---------------------------------------------------------------------------
// IPC server

fn peer_uid(stream: &UnixStream) -> Result<u32> {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        #[repr(C)]
        struct UCred {
            pid: i32,
            uid: u32,
            gid: u32,
        }
        let mut cred: UCred = UCred {
            pid: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        let mut len = std::mem::size_of::<UCred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut UCred as *mut c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(Error::Ipc(format!("SO_PEERCRED failed: rc={rc}")));
        }
        Ok(cred.uid)
    }
    #[cfg(target_os = "macos")]
    {
        let mut euid: u32 = u32::MAX;
        let mut egid: u32 = u32::MAX;
        let rc = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        if rc != 0 {
            return Err(Error::Ipc(format!("getpeereid failed: rc={rc}")));
        }
        Ok(euid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = stream;
        Err(Error::Ipc("unsupported platform for peer uid check".into()))
    }
}

fn dispatch(shared: &Arc<Shared>, request: Request) -> Response {
    match request.method.as_str() {
        "ping" => Response::ok(
            request.id,
            serde_json::json!({"pong": true, "version": env!("CARGO_PKG_VERSION")}),
        ),
        "status" => Response::ok(request.id, shared.snapshot_json()),
        "start" => {
            let result = load_config_for_daemon(shared);
            match result {
                Ok(config) => match shared.engine_start(config) {
                    Ok(()) => Response::ok(request.id, serde_json::json!({"accepted": true})),
                    Err(e) => Response::err(request.id, -32001, e.to_string()),
                },
                Err(e) => Response::err(request.id, -32002, e.to_string()),
            }
        }
        "stop" => {
            shared.request_stop();
            Response::ok(request.id, serde_json::json!({"accepted": true}))
        }
        "restart" => {
            shared.request_stop();
            if !shared.wait_for_engine_finish(std::time::Duration::from_secs(30)) {
                return Response::err(request.id, -32003, "engine did not stop in time");
            }
            match load_config_for_daemon(shared) {
                Ok(config) => match shared.engine_start(config) {
                    Ok(()) => Response::ok(request.id, serde_json::json!({"accepted": true})),
                    Err(e) => Response::err(request.id, -32001, e.to_string()),
                },
                Err(e) => Response::err(request.id, -32002, e.to_string()),
            }
        }
        _ => Response::err(
            request.id,
            -32601,
            format!("method not found: {}", request.method),
        ),
    }
}

fn load_config_for_daemon(shared: &Shared) -> Result<Config> {
    let path = shared.inner.lock().unwrap().config_path.clone();
    Config::load(&path)
}

fn handle_client(shared: Arc<Shared>, stream: UnixStream) {
    let target_uid = shared.inner.lock().unwrap().target_uid;
    match peer_uid(&stream) {
        Ok(uid) if uid == 0 || uid == target_uid => {}
        Ok(uid) => {
            tracing::warn!(uid, "rejected IPC peer");
            return;
        }
        Err(e) => {
            tracing::warn!("peer uid check failed: {e}");
            return;
        }
    }

    let mut stream = stream;
    let mut reader = BufReader::new(stream.try_clone().unwrap_or_else(|_| {
        // try_clone failure is impossible for a freshly accepted stream in
        // practice; fall back to closing the connection.
        let _ = stream.shutdown(std::net::Shutdown::Both);
        UnixStream::pair().unwrap().0
    }));
    while let Ok(line) = ipc::read_line(&mut reader) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                let _ = ipc::write_line(
                    &mut stream,
                    &Response::err(0, -32700, format!("parse error: {e}")),
                );
                continue;
            }
        };

        if request.method == "subscribe" {
            let (tx, rx) = mpsc::channel::<Event>();
            shared.subscribers.lock().unwrap().push(tx.clone());
            let mut writer = match stream.try_clone() {
                Ok(w) => w,
                Err(_) => break,
            };
            let response = Response::ok(request.id, serde_json::json!({"subscribed": true}));
            if ipc::write_line(&mut stream, &response).is_err() {
                break;
            }
            // Push the current snapshot immediately.
            let _ = tx.send(Event::new("state_changed", shared.snapshot_json()));
            thread::spawn(move || {
                for event in rx {
                    if ipc::write_line(&mut writer, &event).is_err() {
                        break;
                    }
                }
            });
            break;
        }

        let response = dispatch(&shared, request);
        if ipc::write_line(&mut stream, &response).is_err() {
            break;
        }
    }
}

fn run_ipc_server(shared: Arc<Shared>, path: PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Ipc(format!("failed to create {}: {e}", parent.display())))?;
        // Directory ownership/permissions are managed by the service layer
        // (systemd RuntimeDirectory / LaunchDaemon); do not chmod arbitrary
        // parents such as /tmp when a test socket override is used.
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .map_err(|e| Error::Ipc(format!("bind {} failed: {e}", path.display())))?;
    // connect(2) requires write permission on the socket inode; bind() leaves
    // it 0755 under the default umask, which would block the unprivileged
    // client. World-connectable is safe here because handle_client rejects
    // every peer whose uid is neither root nor the target user.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))
        .map_err(|e| Error::Ipc(format!("chmod {} failed: {e}", path.display())))?;
    tracing::info!(socket = %path.display(), "IPC server listening");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                thread::spawn(move || handle_client(shared, stream));
            }
            Err(e) => tracing::warn!("accept failed: {e}"),
        }
    }
    Ok(())
}

fn state_file_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|p| p.join("state.json"))
        .unwrap_or_else(|| PathBuf::from("state.json"))
}

/// Resolve the config file for the target user.
///
/// The daemon is managed by systemd/launchd; its own `$HOME` may not match
/// the invoking user (e.g. a root-managed unit), so prefer the account
/// database for `--uid` and fall back to `$HOME` only when that fails.
fn default_config_path_for(uid: u32) -> PathBuf {
    let pw = unsafe { libc::getpwuid(uid) };
    if !pw.is_null() {
        let home = unsafe { CStr::from_ptr((*pw).pw_dir) };
        return PathBuf::from(home.to_string_lossy().as_ref())
            .join(".config")
            .join("inode-vpn")
            .join("config.toml");
    }
    if uid == unsafe { libc::geteuid() } {
        if let Ok(path) = Config::default_path() {
            return path;
        }
    }
    PathBuf::from("config.toml")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "inode_vpnd=info".into()),
        )
        .init();

    let cli = Cli::parse();
    if cli.smoke {
        println!("inode-vpnd {} smoke OK", env!("CARGO_PKG_VERSION"));
        return;
    }

    let uid = cli.uid.unwrap_or_else(|| unsafe { libc::geteuid() });
    let gid = cli.gid_for_uid(uid);
    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(|| default_config_path_for(uid));
    let socket_path = cli.socket.clone().unwrap_or_else(|| ipc::socket_path(uid));

    tracing::info!(uid, config = %config_path.display(), "daemon starting");
    let shared = Shared::new(uid, gid, config_path.clone(), cli.tun_script.clone());
    let shared_ipc = Arc::new(shared.clone());

    // Daemon startup: restore desired state when autostart is enabled.
    match Config::load(&config_path) {
        Ok(config) => {
            if config.service.autostart {
                tracing::info!(autostart = true, "restoring VPN state");
                shared_ipc.add_redaction_secrets(&config);
                if let Err(e) = shared_ipc.engine_start(config) {
                    tracing::error!("autostart failed: {e}");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "config not loaded at startup; autostart skipped"),
    }

    let _ipc_thread = {
        let shared = Arc::clone(&shared_ipc);
        let path = socket_path.clone();
        thread::spawn(move || {
            if let Err(e) = run_ipc_server(shared, path) {
                tracing::error!("IPC server failed: {e}");
            }
        })
    };

    let _netwatch_thread = netwatch::spawn(Arc::clone(&shared_ipc));

    // Wait for shutdown signals, then request a clean stop.
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }

    tracing::info!("shutdown requested");
    shared_ipc.request_stop();
    if shared_ipc.wait_for_engine_finish(std::time::Duration::from_secs(15)) {
        tracing::info!("engine stopped cleanly");
    } else {
        tracing::warn!("engine did not stop before shutdown deadline");
    }
    // Do not join the IPC listener: `UnixListener::incoming()` never returns
    // on its own, and joining it would hang shutdown until systemd sends
    // SIGKILL/SIGABRT. Exiting main() terminates the process and all threads.
    let _ = std::fs::remove_file(socket_path);
}

impl Cli {
    fn gid_for_uid(&self, uid: u32) -> u32 {
        unsafe {
            let pw = libc::getpwuid(uid);
            if pw.is_null() {
                uid
            } else {
                (*pw).pw_gid
            }
        }
    }
}
