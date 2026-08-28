//! Network-change watcher.
//!
//! Linux: spawns `ip monitor address link` and forwards physical-interface
//! events to [`Shared::network_changed`], which debounces and issues
//! OC_CMD_PAUSE so the engine reconnects on the same cookie.
//!
//! macOS: uses a SystemConfiguration dynamic store with a run-loop source.
//! It watches the global IPv4 state plus service IPv4 / interface link
//! changes, filters out the VPN's own utun traffic, and forwards genuine
//! physical-network transitions to [`Shared::network_changed`].

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

// ---------------------------------------------------------------------------
// macOS SystemConfiguration watcher

#[cfg(target_os = "macos")]
mod ffi {
    use libc::{c_char, c_void};

    pub type Boolean = u8;
    pub type CFIndex = isize;
    pub type CFAllocatorRef = *const c_void;
    pub type CFStringRef = *const c_void;
    pub type CFArrayRef = *const c_void;
    pub type CFRunLoopRef = *mut c_void;
    pub type CFRunLoopSourceRef = *mut c_void;
    pub type SCDynamicStoreRef = *mut c_void;
    pub type SCDynamicStoreCallBack =
        unsafe extern "C" fn(SCDynamicStoreRef, CFArrayRef, *mut c_void);

    #[repr(C)]
    pub struct SCDynamicStoreContext {
        pub version: CFIndex,
        pub info: *mut c_void,
        pub retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
        pub release: Option<unsafe extern "C" fn(*const c_void)>,
        pub copy_description: Option<unsafe extern "C" fn(*const c_void) -> CFStringRef>,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFStringCreateWithCString(
            alloc: CFAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        pub fn CFArrayCreate(
            alloc: CFAllocatorRef,
            values: *const *const c_void,
            num_values: CFIndex,
            callbacks: *const c_void,
        ) -> CFArrayRef;
        pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
        pub fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
        pub fn CFStringGetCString(
            string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> Boolean;
        pub fn CFRelease(cf: *const c_void);
        pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        pub fn CFRunLoopAddSource(
            run_loop: CFRunLoopRef,
            source: CFRunLoopSourceRef,
            mode: CFStringRef,
        );
        pub fn CFRunLoopRun();
    }

    #[link(name = "SystemConfiguration", kind = "framework")]
    extern "C" {
        pub fn SCDynamicStoreCreate(
            allocator: CFAllocatorRef,
            name: CFStringRef,
            callout: Option<SCDynamicStoreCallBack>,
            context: *const SCDynamicStoreContext,
        ) -> SCDynamicStoreRef;
        pub fn SCDynamicStoreSetNotificationKeys(
            store: SCDynamicStoreRef,
            keys: CFArrayRef,
            patterns: CFArrayRef,
        ) -> Boolean;
        pub fn SCDynamicStoreCreateRunLoopSource(
            allocator: CFAllocatorRef,
            store: SCDynamicStoreRef,
            order: CFIndex,
        ) -> CFRunLoopSourceRef;
    }

    pub const UTF8: u32 = 0x0800_0100;
}

#[cfg(target_os = "macos")]
pub fn spawn(shared: Arc<Shared>) -> JoinHandle<()> {
    thread::Builder::new()
        .name("inode-netwatch".into())
        .spawn(move || loop {
            if !run_dynamic_store(Arc::clone(&shared)) {
                tracing::warn!("SystemConfiguration netwatch setup failed; retrying");
            }
            thread::sleep(std::time::Duration::from_secs(2));
        })
        .expect("spawn netwatch thread")
}

#[cfg(target_os = "macos")]
fn run_dynamic_store(shared: Arc<Shared>) -> bool {
    use ffi::*;
    use std::ffi::CString;

    // The context info pointer must outlive the store. The watcher thread
    // owns the Box and never returns for the lifetime of the daemon, and no
    // retain/release callbacks are registered, so the store holds a raw
    // borrow of this pointer.
    let info = Box::into_raw(Box::new(shared)) as *mut libc::c_void;
    let context = SCDynamicStoreContext {
        version: 0,
        info,
        retain: None,
        release: None,
        copy_description: None,
    };

    let store_name = CString::new("inode-vpnd-network").expect("static string");
    let global_ipv4 = CString::new("State:/Network/Global/IPv4").expect("static string");
    let service_ipv4_pattern =
        CString::new("State:/Network/Service/.*/IPv4").expect("static string");
    let link_pattern = CString::new("State:/Network/Interface/.*/Link").expect("static string");
    let mode = CString::new("kCFRunLoopDefaultMode").expect("static string");

    unsafe {
        let name = CFStringCreateWithCString(std::ptr::null(), store_name.as_ptr(), UTF8);
        let global_key = CFStringCreateWithCString(std::ptr::null(), global_ipv4.as_ptr(), UTF8);
        let service_pattern =
            CFStringCreateWithCString(std::ptr::null(), service_ipv4_pattern.as_ptr(), UTF8);
        let link_pattern_cf =
            CFStringCreateWithCString(std::ptr::null(), link_pattern.as_ptr(), UTF8);
        let mode_cf = CFStringCreateWithCString(std::ptr::null(), mode.as_ptr(), UTF8);

        if name.is_null()
            || global_key.is_null()
            || service_pattern.is_null()
            || link_pattern_cf.is_null()
            || mode_cf.is_null()
        {
            tracing::warn!("failed to create CFString values for netwatch");
            for cf in [name, global_key, service_pattern, link_pattern_cf, mode_cf] {
                if !cf.is_null() {
                    CFRelease(cf);
                }
            }
            return false;
        }

        let store = SCDynamicStoreCreate(std::ptr::null(), name, Some(store_changed), &context);
        if store.is_null() {
            tracing::warn!("SCDynamicStoreCreate failed");
            for cf in [name, global_key, service_pattern, link_pattern_cf, mode_cf] {
                CFRelease(cf);
            }
            return false;
        }

        let keys: [CFStringRef; 1] = [global_key];
        let keys_array = CFArrayCreate(
            std::ptr::null(),
            keys.as_ptr(),
            keys.len() as CFIndex,
            std::ptr::null(),
        );
        let patterns: [CFStringRef; 2] = [service_pattern, link_pattern_cf];
        let patterns_array = CFArrayCreate(
            std::ptr::null(),
            patterns.as_ptr(),
            patterns.len() as CFIndex,
            std::ptr::null(),
        );

        if keys_array.is_null()
            || patterns_array.is_null()
            || SCDynamicStoreSetNotificationKeys(store, keys_array, patterns_array) == 0
        {
            tracing::warn!("SCDynamicStoreSetNotificationKeys failed");
            if !keys_array.is_null() {
                CFRelease(keys_array);
            }
            if !patterns_array.is_null() {
                CFRelease(patterns_array);
            }
            CFRelease(store);
            for cf in [name, global_key, service_pattern, link_pattern_cf, mode_cf] {
                CFRelease(cf);
            }
            return false;
        }

        let source = SCDynamicStoreCreateRunLoopSource(std::ptr::null(), store, 0);
        if source.is_null() {
            tracing::warn!("SCDynamicStoreCreateRunLoopSource failed");
            CFRelease(keys_array);
            CFRelease(patterns_array);
            CFRelease(store);
            for cf in [name, global_key, service_pattern, link_pattern_cf, mode_cf] {
                CFRelease(cf);
            }
            return false;
        }

        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, mode_cf);
        // The run loop and store now retain what they need; the strings and
        // arrays were only needed for setup. The store itself is never
        // released because this thread owns it for the daemon's lifetime.
        CFRelease(keys_array);
        CFRelease(patterns_array);
        CFRelease(name);
        CFRelease(global_key);
        CFRelease(service_pattern);
        CFRelease(link_pattern_cf);
        CFRelease(mode_cf);

        tracing::info!("SystemConfiguration netwatch active");
        CFRunLoopRun();
    }

    // CFRunLoopRun() returning is abnormal for a daemon thread; caller
    // re-creates the store after a short delay. The Box<Arc<Shared>> is
    // intentionally not dropped because the run loop/store may still hold it.
    true
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn store_changed(
    _store: ffi::SCDynamicStoreRef,
    changed_keys: ffi::CFArrayRef,
    info: *mut libc::c_void,
) {
    if info.is_null() || !changed_keys_changed_physical_network(changed_keys) {
        return;
    }
    let shared = &*(info as *const Arc<Shared>);
    shared.network_changed();
}

/// True when any changed SCDynamicStore key describes the physical network
/// rather than the engine's own tun/utun plumbing.
#[cfg(target_os = "macos")]
fn changed_keys_changed_physical_network(keys: ffi::CFArrayRef) -> bool {
    use ffi::*;
    use std::ffi::{c_char, CStr};

    unsafe {
        let count = CFArrayGetCount(keys);
        for i in 0..count {
            let key = CFArrayGetValueAtIndex(keys, i);
            let mut buf = [0i8; 512];
            if key.is_null()
                || CFStringGetCString(
                    key,
                    buf.as_mut_ptr() as *mut c_char,
                    buf.len() as CFIndex,
                    UTF8,
                ) == 0
            {
                continue;
            }
            let text = CStr::from_ptr(buf.as_ptr()).to_string_lossy();
            if interesting_macos_key(&text) {
                return true;
            }
        }
    }
    false
}

/// SCDynamicStore key filter shared by the watcher and its unit tests.
#[cfg(any(target_os = "macos", test))]
fn interesting_macos_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let ignored = [
        "/utun", "/lo0", "/awdl", "/llw", "/bridge", "/ipsec", "/ppp", "/tap", "/gif", "/stf",
        "/xlink",
    ];
    if ignored.iter().any(|needle| key.contains(needle)) {
        return false;
    }
    key == "state:/network/global/ipv4"
        || key.starts_with("state:/network/service/")
        || key.starts_with("state:/network/interface/en")
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

    #[test]
    fn filters_macos_virtual_interfaces() {
        assert!(super::interesting_macos_key("State:/Network/Global/IPv4"));
        assert!(super::interesting_macos_key(
            "State:/Network/Service/AA-BB-CC/IPv4"
        ));
        assert!(super::interesting_macos_key(
            "State:/Network/Interface/en0/Link"
        ));
        assert!(!super::interesting_macos_key(
            "State:/Network/Interface/utun5/IPv4"
        ));
        assert!(!super::interesting_macos_key(
            "State:/Network/Interface/utun5/Link"
        ));
        assert!(!super::interesting_macos_key(
            "State:/Network/Interface/lo0/IPv4"
        ));
        assert!(!super::interesting_macos_key(
            "State:/Network/Interface/awdl0/Link"
        ));
    }
}
