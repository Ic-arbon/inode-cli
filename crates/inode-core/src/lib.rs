//! Shared types, state machine, and errors for the inode-vpn project.

pub mod error;
pub mod state;

pub use error::{Error, Result};
pub use state::SessionState;
