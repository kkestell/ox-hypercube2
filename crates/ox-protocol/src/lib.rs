//! Wire types and client for oxd, the daemon that owns Ox's ACP agent
//! sessions.
//!
//! Messages are newline-delimited JSON over a Unix socket. Clients send
//! [`Request`]s; the daemon answers each with a [`ServerMessage`] carrying the
//! same `id`. The daemon also pushes streams, each named in
//! every message by the `id` of the request that started it: a subscription
//! (`session.subscribe`) delivers one session's [`Event`]s, a watch (`watch`)
//! delivers every workspace, worktree, and session, then each one that is
//! added, changes, or is removed, and a
//! directory watch (`directory.watch`) delivers a directory's entries whenever
//! they change. A file watch (`file.watch`) delivers changed text or read errors.
//! `unsubscribe` ends any of them.

#[cfg(unix)]
mod client;
pub mod config;
mod messages;

#[cfg(unix)]
pub use client::{
    Client, End, Error, Stream, Subscription, bridge_mode, daemon_binary, data_dir,
    default_socket_path,
};
#[cfg(unix)]
pub mod content;
pub use messages::*;

pub use agent_client_protocol::schema::MaybeUndefined;
pub use agent_client_protocol::v2::schema;
