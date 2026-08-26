//! `inode` — user-facing CLI (M2: real IPC to inode-vpnd).

use clap::{CommandFactory, Parser, Subcommand};
use inode_core::ipc::{self, Request, Response};
use inode_core::redact::Redactor;
use inode_core::state::StatusSnapshot;
use inode_core::{Config, SessionState};
use serde_json::{json, Value};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

mod service;

#[derive(Debug, Parser)]
#[command(name = "inode", version, about = "H3C SSL VPN client (inode-vpn)")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// IPC socket override (tests).
    #[arg(long, hide = true)]
    socket: Option<PathBuf>,

    /// Connect timeout in milliseconds.
    #[arg(long, hide = true, default_value_t = 3000)]
    timeout_ms: u64,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Connect the VPN.
    Start,
    /// Disconnect the VPN.
    Stop,
    /// Disconnect, then connect again.
    Restart,
    /// Show daemon/session status.
    Status {
        /// Emit the stable JSON schema defined in docs/architecture.md.
        #[arg(long)]
        json: bool,
        /// Subscribe to daemon events and stream them until interrupted.
        #[arg(long)]
        watch: bool,
    },
    /// Show or follow daemon logs.
    Logs {
        /// Follow new log lines (not implemented yet in M2).
        #[arg(short, long)]
        follow: bool,
    },
    /// Install/enable the system service (M3/M4).
    Enable {
        /// Also connect immediately.
        #[arg(long)]
        now: bool,
    },
    /// Disable/uninstall the system service (M3/M4).
    Disable {
        /// Also disconnect immediately.
        #[arg(long)]
        now: bool,
    },
    /// Configuration helpers.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Produce a redacted diagnostic bundle (M5).
    Diagnose,
    /// Discover the gateway certificate pin (TOFU, pin-sha256 = SPKI hash).
    DiscoverCert {
        /// Allow replacing an existing, different pin.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print effective configuration (secrets redacted).
    Show,
    /// Set a configuration value (not implemented yet).
    Set { key: String, value: String },
    /// Migrate legacy `.auth` to ~/.config/inode-vpn/config.toml (0600).
    Migrate,
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

fn connect(socket: Option<PathBuf>, timeout_ms: u64) -> Result<UnixStream, String> {
    let path = socket.unwrap_or_else(|| ipc::socket_path(euid()));
    let stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "cannot connect to daemon at {}: {e} (is `inode-vpnd` running?)",
            path.display()
        )
    })?;
    let _ = timeout_ms;
    // Short idle read timeout is not supported by std UnixStream; the
    // daemon answers promptly or the read blocks until EOF. Keep the arg
    // for the upcoming tokio rewrite.
    Ok(stream)
}

fn rpc(mut stream: UnixStream, id: u64, method: &str, params: Value) -> Result<Response, String> {
    let req = Request::with_params(id, method, params);
    ipc::write_line(&mut stream, &req).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let line = ipc::read_line(&mut reader).map_err(|e| e.to_string())?;
    serde_json::from_str::<Response>(line.trim()).map_err(|e| format!("bad daemon response: {e}"))
}

fn unwrap_result(response: Response, method: &str) -> Value {
    if let Some(err) = response.error {
        eprintln!("{method} failed: {}", err.message);
        std::process::exit(1);
    }
    response.result.unwrap_or(json!({}))
}

fn status_text(status: &StatusSnapshot) {
    match status.state {
        SessionState::Connected => {
            let ip = status
                .session
                .as_ref()
                .and_then(|s| s.ip.clone())
                .unwrap_or_else(|| "?".into());
            println!("● vpn - 已连接 (active, running, ip {ip})");
        }
        SessionState::Stopped => println!("○ vpn - 未连接 (inactive)"),
        state => println!("◐ vpn - {state}"),
    }
    if let Some(err) = &status.last_error {
        println!("  最后错误: {err}");
    }
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    Some(format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

fn locate_openconnect() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("INODE_OPENCONNECT_BIN") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("openconnect-h3c");
            if bundled.is_file() {
                return Some(bundled);
            }
        }
    }
    let in_path = PathBuf::from("openconnect");
    if in_path.is_file() {
        return Some(in_path);
    }
    None
}

fn parse_pin(output: &str) -> Option<String> {
    const PREFIX: &str = "pin-sha256:";
    let idx = output.find(PREFIX)?;
    let rest = &output[idx + PREFIX.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '+' && c != '/' && c != '=')
        .unwrap_or(rest.len());
    let pin = &rest[..end];
    if pin.len() >= 20 {
        Some(format!("{PREFIX}{pin}"))
    } else {
        None
    }
}

fn run_discover_cert(force: bool) -> i32 {
    let Some(bin) = locate_openconnect() else {
        eprintln!(
            "cannot locate openconnect-h3c binary; set INODE_OPENCONNECT_BIN or install the fork"
        );
        return 1;
    };
    let config_path = match Config::default_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut cfg = match Config::load(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let mut child = match std::process::Command::new(&bin)
        .args(["--protocol", "h3c", "--no-dtls", &cfg.gateway.url])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to spawn {}: {e}", bin.display());
            return 1;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"no\n");
    }
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("openconnect failed: {e}");
            return 1;
        }
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let Some(pin) = parse_pin(&text) else {
        eprintln!("no pin-sha256 found in openconnect output");
        return 1;
    };

    if !cfg.gateway.servercert.is_empty() && cfg.gateway.servercert != pin && !force {
        eprintln!(
            "certificate pin changed\n  configured: {}\n  discovered: {pin}\nre-run with --force to accept the new pin",
            cfg.gateway.servercert
        );
        return 1;
    }
    cfg.gateway.servercert = pin.clone();
    if let Err(e) = cfg.save(&config_path) {
        eprintln!("{e}");
        return 1;
    }
    println!("discovered and saved {pin}");
    0
}

/// Redacted diagnostic bundle. Never includes password/cookie values.
fn run_diagnose(socket: Option<PathBuf>, timeout_ms: u64) -> i32 {
    let mut redactor = Redactor::new(std::iter::empty::<String>());
    let mut bundle = json!({
        "inode_version": env!("CARGO_PKG_VERSION"),
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
    });

    let config_path = Config::default_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
    match Config::load(&config_path) {
        Ok(cfg) => {
            redactor.add(cfg.credentials.password.clone());
            redactor.add(cfg.credentials.username.clone());
            bundle["config"] = serde_json::to_value(cfg.redacted()).unwrap_or(Value::Null);
        }
        Err(e) => bundle["config_error"] = json!(e.to_string()),
    }

    if let Ok(stream) = connect(socket.clone(), timeout_ms) {
        match rpc(stream, 1, "status", json!({})) {
            Ok(resp) if resp.error.is_none() => {
                bundle["daemon_status"] = resp.result.unwrap_or(Value::Null);
            }
            Ok(resp) => bundle["daemon_error"] = json!(resp.error.map(|e| e.message)),
            Err(e) => bundle["daemon_error"] = json!(e),
        }
    } else {
        bundle["daemon_error"] = json!("daemon not reachable");
    }

    bundle["uname"] = json!(command_output("uname", &["-a"]).unwrap_or_default());
    #[cfg(target_os = "linux")]
    {
        bundle["routes"] = json!(command_output("ip", &["route", "show"]).unwrap_or_default());
        bundle["addrs"] = json!(command_output("ip", &["-4", "addr", "show"]).unwrap_or_default());
        bundle["dns"] = json!(command_output("resolvectl", &["status"]).unwrap_or_default());
    }
    #[cfg(target_os = "macos")]
    {
        bundle["routes"] =
            json!(command_output("netstat", &["-rn", "-f", "inet"]).unwrap_or_default());
        bundle["addrs"] = json!(command_output("ifconfig", &[]).unwrap_or_default());
        bundle["dns"] = json!(command_output("scutil", &["--dns"]).unwrap_or_default());
    }

    let text = serde_json::to_string_pretty(&bundle).unwrap_or_default();
    println!("{}", redactor.redact(&text));
    0
}

/// Subscribe to daemon events and stream `state_changed` snapshots.
fn run_watch(socket: Option<PathBuf>, timeout_ms: u64, json: bool) -> i32 {
    let mut stream = match connect(socket, timeout_ms) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if let Err(e) = ipc::write_line(
        &mut stream,
        &Request::with_params(1, "subscribe", json!({})),
    ) {
        eprintln!("subscribe failed: {e}");
        return 1;
    }
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    match ipc::read_line(&mut reader) {
        Ok(line) => {
            let Ok(resp) = serde_json::from_str::<Response>(line.trim()) else {
                eprintln!("bad subscribe response");
                return 1;
            };
            if let Some(err) = resp.error {
                eprintln!("subscribe failed: {}", err.message);
                return 1;
            }
        }
        Err(e) => {
            eprintln!("subscribe failed: {e}");
            return 1;
        }
    }

    loop {
        let line = match ipc::read_line(&mut reader) {
            Ok(l) => l,
            Err(_) => return 0,
        };
        let Ok(event) = serde_json::from_str::<inode_core::ipc::Event>(line.trim()) else {
            continue;
        };
        if event.method == "state_changed" {
            if json {
                println!("{}", serde_json::to_string_pretty(&event.params).unwrap());
            } else if let Ok(status) = serde_json::from_value::<StatusSnapshot>(event.params) {
                status_text(&status);
                println!("---");
            }
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        Cli::command().print_help().unwrap();
        println!();
        return;
    };

    match command {
        Command::Start | Command::Stop | Command::Restart | Command::Status { .. } => {
            let stream = match connect(cli.socket.clone(), cli.timeout_ms) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            };
            let method = match &command {
                Command::Start => "start",
                Command::Stop => "stop",
                Command::Restart => "restart",
                Command::Status { .. } => "status",
                _ => unreachable!(),
            };
            let response = rpc(stream, 1, method, json!({}));
            match command {
                Command::Status { json, watch } if watch => {
                    drop(response);
                    std::process::exit(run_watch(cli.socket.clone(), cli.timeout_ms, json));
                }
                Command::Status { json, .. } => match response {
                    Ok(resp) => {
                        let value = unwrap_result(resp, "status");
                        let status: StatusSnapshot =
                            serde_json::from_value(value).unwrap_or_else(|e| {
                                eprintln!("status parse failed: {e}");
                                std::process::exit(1);
                            });
                        if json {
                            println!("{}", serde_json::to_string_pretty(&status).unwrap());
                        } else {
                            status_text(&status);
                        }
                        std::process::exit(status.state.exit_code());
                    }
                    Err(e) => {
                        eprintln!("status failed: {e}");
                        std::process::exit(1);
                    }
                },
                _ => match response {
                    Ok(resp) => {
                        let value = unwrap_result(resp, method);
                        if value.get("accepted").and_then(Value::as_bool) == Some(true) {
                            println!("{method} accepted");
                        }
                    }
                    Err(e) => {
                        eprintln!("{method} failed: {e}");
                        std::process::exit(1);
                    }
                },
            }
        }
        Command::Logs { follow } => {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                if let Err(e) = service::logs(euid(), follow) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = follow;
                eprintln!("logs are unsupported on this platform");
                std::process::exit(1);
            }
        }
        Command::Diagnose => {
            std::process::exit(run_diagnose(cli.socket.clone(), cli.timeout_ms));
        }
        Command::DiscoverCert { force } => {
            std::process::exit(run_discover_cert(force));
        }
        ref command @ (Command::Enable { now } | Command::Disable { now }) => {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                let result = match command {
                    Command::Enable { .. } => service::enable(euid(), now),
                    Command::Disable { .. } => service::disable(euid(), now),
                    _ => unreachable!(),
                };
                if let Err(e) = result {
                    eprintln!("service operation failed: {e}");
                    std::process::exit(1);
                }
                println!("service operation complete");
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = now;
                eprintln!("service installation is unsupported on this platform");
                std::process::exit(1);
            }
        }
        Command::Config(cmd) => match cmd {
            ConfigCommand::Show => {
                let path = Config::default_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
                match Config::load(&path) {
                    Ok(cfg) => {
                        println!("{}", serde_json::to_string_pretty(&cfg.redacted()).unwrap());
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            ConfigCommand::Set { key, value } => {
                let path = Config::default_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
                let mut cfg = match Config::load(&path) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                };
                if let Err(e) = cfg.with_key_set(&key, &value) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                if let Err(e) = cfg.save(&path) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("set {key} (secrets not displayed)");
            }
            ConfigCommand::Migrate => {
                let auth = std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".auth");
                let dest = Config::default_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
                match Config::migrate_legacy_auth(&auth, &dest) {
                    Ok(cfg) => {
                        println!("migrated to {} (mode 0600)", dest.display());
                        println!(
                            "gateway={} username={}",
                            cfg.gateway.url, cfg.credentials.username
                        );
                        println!("提示：确认无误后可删除旧 .auth 文件");
                    }
                    Err(e) => {
                        eprintln!("migrate failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
        },
    }
}
