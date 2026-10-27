//! The malac agent runtime. Front ends (terminal, web, editor) create a
//! [`Runtime`], spawn agents on it and render the [`event`] stream.

pub mod agent;
pub mod attach;
pub mod catalog;
pub mod commands;
pub mod config;
pub mod context;
pub mod event;
pub mod frontmatter;
pub mod jobs;
pub mod mcp;
pub mod memory;
pub mod message;
pub mod permission;
pub mod plugins;
pub mod prompt;
pub mod provider;
pub mod runtime;
pub mod session;
pub mod shared;
pub mod tools;
pub mod update;
pub mod voice;

pub use agent::{AgentHandle, AgentSnapshot, AgentStatus};
pub use config::Config;
pub use event::{Envelope, Event};
pub use message::{Block, Message};
pub use runtime::{AgentSpec, Host, Runtime};
