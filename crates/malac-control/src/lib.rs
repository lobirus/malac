//! Computer and browser control exposed as MCP tools.
//!
//! [`handler`] builds the MCP [`Handler`]; the `malac-control` binary serves
//! it over stdio or WebSocket, and malac can also embed it in-process.

mod browser;
mod computer;
pub mod config;
mod control;
pub mod discovery;
mod imaging;
mod keys;
mod tool;

use std::sync::Arc;

use malac_mcp::Handler;

pub use browser::list_profiles;
pub use config::Config;
pub use control::Control;

pub fn handler(config: Config) -> Arc<dyn Handler> {
    Arc::new(Control::new(config))
}
