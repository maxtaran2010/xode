//! Network tools: web search/fetch, browser control over CDP, MCP client.

pub mod browser;
pub mod cdp;
pub mod html;
pub mod mcp;
pub mod util;
pub mod web;

pub use browser::{browser_test, browser_tool};
pub use mcp::{test_server, McpManager, McpStatus};
pub use web::{web_fetch_tool, web_search_tool};
