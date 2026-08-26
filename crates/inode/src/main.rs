//! `inode` — user-facing CLI.
//!
//! M0 skeleton: full command surface with placeholders. IPC to the daemon
//! arrives in M2.

use clap::{CommandFactory, Parser, Subcommand};
use inode_core::SessionState;

#[derive(Debug, Parser)]
#[command(name = "inode", version, about = "H3C SSL VPN client (inode-vpn)")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
        /// Follow new log lines.
        #[arg(short, long)]
        follow: bool,
    },
    /// Install/enable the system service.
    Enable {
        /// Also connect immediately.
        #[arg(long)]
        now: bool,
    },
    /// Disable/uninstall the system service.
    Disable {
        /// Also disconnect immediately.
        #[arg(long)]
        now: bool,
    },
    /// Configuration helpers (show/set/migrate).
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Produce a redacted diagnostic bundle.
    Diagnose,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print effective configuration (secrets redacted).
    Show,
    /// Set a configuration value.
    Set { key: String, value: String },
    /// Migrate legacy `.auth` to ~/.config/inode-vpn/config.toml (0600).
    Migrate,
}

fn main() {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        Cli::command().print_help().unwrap();
        println!();
        return;
    };

    match command {
        Command::Status { json } => {
            // M0 skeleton: no daemon yet, so report Stopped with the exit code
            // contract already exercised.
            let state = SessionState::Stopped;
            if json {
                println!("{}", serde_json::json!({ "state": state }));
            } else {
                println!("○ vpn - 未连接 (inactive)");
            }
            std::process::exit(state.exit_code());
        }
        other => {
            eprintln!("inode M0 skeleton: `{other:?}` is not implemented yet");
            std::process::exit(1);
        }
    }
}
