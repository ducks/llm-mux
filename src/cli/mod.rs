//! CLI module for llmux
//!
//! This module provides:
//! - Command implementations (run, validate, doctor, etc.)
//! - Output handlers (console, JSON, quiet)
//! - Signal handling for graceful shutdown
//!
//! # Example
//!
//! ```ignore
//! use llmux::cli::{commands, output, signals};
//!
//! let handler = output::create_handler(output::OutputMode::Console, false);
//! let exit_code = commands::run_workflow("my-workflow", args, dir, None, config, &*handler).await?;
//! ```

pub mod commands;
pub mod output;
pub mod signals;

pub use commands::{
    doctor, list_backends, list_roles, list_teams, run_workflow, validate_workflow,
};
pub use output::{OutputEvent, OutputHandler, OutputMode, create_handler};
pub use signals::{
    CancellationToken, is_shutdown_requested, setup_signal_handlers, with_cancellation,
};
