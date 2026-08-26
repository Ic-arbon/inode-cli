use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(String),

    #[error("ipc error: {0}")]
    Ipc(String),

    #[error("openconnect error: {0}")]
    OpenConnect(String),

    #[error("route setup failed: {0}")]
    Route(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
