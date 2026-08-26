//! `inode-vpnd` — root VPN daemon.
//!
//! M0 skeleton: parse the planned daemon CLI and initialize tracing. Engine,
//! state machine, IPC and platform service integration land in M2.

use clap::Parser;
use inode_core::SessionState;

#[derive(Debug, Parser)]
#[command(name = "inode-vpnd", version, about = "inode-vpn root daemon")]
struct Cli {
    /// Target user id whose config and VPN session this daemon manages.
    #[arg(long)]
    uid: Option<u32>,

    /// Run one foreground cycle and exit (M0 smoke test only).
    #[arg(long)]
    smoke: bool,
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
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        uid = cli.uid,
        state = %SessionState::Stopped,
        "inode-vpnd skeleton started"
    );

    if !cli.smoke {
        eprintln!("M0 skeleton: engine not implemented yet; pass --smoke for a clean exit");
        std::process::exit(1);
    }

    println!("inode-vpnd {} smoke OK", env!("CARGO_PKG_VERSION"));
}
