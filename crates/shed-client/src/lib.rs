//! Talking to the keeper without a window: the connection (starting
//! `shedd` if needed), blocking requests and the CLI's keeper commands,
//! edits routed to running or closed benches, the pegboard's list, and
//! remote shells. No GPUI here, so the windows, the CLI and other clients
//! share it.

pub mod bench_edit;
pub mod cli;
pub mod keeper_conn;
pub mod link_status;
pub mod pegboard;
pub mod remote;

pub use bench_edit::BenchEdit;
pub use keeper_conn::KeeperConn;
