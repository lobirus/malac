//! The `malac` command line, terminal UI, web server and daemon. The binary
//! only calls [`main`]. Clients of the daemon, such as editor integrations
//! or front ends, use [`client`] and the message types in [`protocol`].

mod approve;
mod cli;
pub mod client;
mod commands;
pub mod connections;
pub mod daemon;
mod files;
mod plugins;
mod project;
pub mod protocol;
mod qr;
mod remote;
mod remotes;
mod render;
mod repl;
mod style;
mod tui;
mod web;

pub use cli::{RunArgs, SessionOptions, main};
