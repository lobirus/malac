//! A small Model Context Protocol implementation: enough of the spec for
//! malac to consume tool servers and for `malac-control` to be one.

pub mod client;
pub mod protocol;
pub mod server;

pub use client::Client;
pub use protocol::{CallToolResult, Content, Implementation, Tool};
pub use server::Handler;
