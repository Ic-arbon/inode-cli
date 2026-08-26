//! Shared types, configuration, state machine, IPC contract and errors for
//! the inode-vpn project.

pub mod config;
pub mod error;
pub mod ipc;
pub mod redact;
pub mod state;

pub use config::Config;
pub use error::{Error, Result};
pub use state::SessionState;
