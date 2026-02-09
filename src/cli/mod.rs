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

pub use output::OutputEvent;
