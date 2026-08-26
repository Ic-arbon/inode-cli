//! `inode` — user-facing CLI (M2: real IPC to inode-vpnd).

use clap::{CommandFactory, Parser, Subcommand};
use inode_core::ipc::{self, Request, Response};
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
                Command::Status { json } => match response {
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
            #[cfg(target_os = "linux")]
            {
                if let Err(e) = service::logs(euid(), follow) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = follow;
                eprintln!("logs on macOS lands in M4");
                std::process::exit(1);
            }
        }
        Command::Diagnose => {
            eprintln!("not implemented yet (planned in M5)");
            std::process::exit(1);
        }
        ref command @ (Command::Enable { now } | Command::Disable { now }) => {
            #[cfg(target_os = "linux")]
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
            #[cfg(not(target_os = "linux"))]
            {
                let _ = now;
                eprintln!("service installation lands in M4 on macOS");
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
            ConfigCommand::Set { .. } => {
                eprintln!("`config set` not implemented yet; edit the TOML file manually");
                std::process::exit(1);
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
