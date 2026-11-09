//! mcp-core: shared building blocks for mcpOS servers.
//! - `json`: dependency-free JSON
//! - `server`: MCP (JSON-RPC 2.0 over stdio, newline-delimited) server loop
//! - `proc`: run commands with timeout, sudo helpers
//! - `util`: base64, truncation, schema helpers

pub mod json;
pub mod proc;
pub mod server;
pub mod util;

pub use json::{obj, Value};
pub use server::{serve_stdio, Handler, ToolDef, ToolResult, Tools};
