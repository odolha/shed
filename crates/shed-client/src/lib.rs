//! Talking to the keeper without a window: the connection (starting
//! `shedd` if needed), blocking requests and the CLI's keeper commands, and
//! remote shells. No GPUI here, so the windows, the CLI and other clients
//! share it.

pub mod cli;
pub mod keeper_conn;
pub mod remote;

pub use keeper_conn::KeeperConn;
