//! Built-in file and shell tools plus the RTK output filters.
//!
//! - [`fs_tools`]: `read`, `write`, `edit`, `glob`, `grep`
//! - [`shell_tool`]: `shell`
//! - [`plan_tool`]: `plan` (plan mode only)
//! - [`rtk`]: `filter_output` / `cap_output` for compressing and capping tool output

mod edit;
mod glob;
mod grep;
mod plan;
mod read;
pub mod rtk;
mod shell;
mod util;
mod write;

use std::sync::Arc;
use xode_core::tool::ToolRef;

pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use plan::{plan_path, PlanTool};
pub use read::ReadTool;
pub use shell::ShellTool;
pub use write::WriteTool;

/// `read`, `write`, `edit`, `glob`, `grep`.
pub fn fs_tools() -> Vec<ToolRef> {
    vec![Arc::new(ReadTool), Arc::new(WriteTool), Arc::new(EditTool), Arc::new(GlobTool), Arc::new(GrepTool)]
}

/// `plan` (plan mode only).
pub fn plan_tool() -> ToolRef {
    Arc::new(PlanTool)
}

/// `shell` (PowerShell on Windows, bash/sh elsewhere; configurable via `tools.shell`).
pub fn shell_tool() -> ToolRef {
    Arc::new(ShellTool)
}
