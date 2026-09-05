//! Workflow execution engine for llmux
//!
//! This module handles:
//! - Workflow state management
//! - Step execution (shell, query, apply, input)
//! - DAG-based dependency resolution
//! - for_each iteration
//! - Conditional execution
//!
//! # Example
//!
//! ```ignore
//! use llmux::workflow::WorkflowRunner;
//! use llmux::config::{LlmuxConfig, load_workflow};
//! use std::sync::Arc;
//!
//! let config = Arc::new(LlmuxConfig::load(None)?);
//! let runner = WorkflowRunner::new(config.clone());
//!
//! let workflow = load_workflow("review", None, false)?;
//! let result = runner.run(workflow, args, Path::new("."), None).await?;
//!
//! if result.success {
//!     println!("Workflow completed successfully!");
//! }
//! ```

mod ecosystem_detector;
mod executor;
mod runner;
mod state;

pub use ecosystem_detector::detect_ecosystem;
pub use runner::WorkflowRunner;
pub use state::WorkflowResult;
