// TODO: Wire up apply_and_verify to workflow runner for `type = "apply"` steps
#![allow(dead_code)]

//! Apply and verify module for llmux
//!
//! This module handles:
//! - Parsing edit formats from LLM output (unified diff, old/new pairs, full file)
//! - Applying edits to files with fuzzy matching
//! - Creating backups before modifications
//! - Running verification commands
//! - Rollback on verification failure
//! - Retry loop with error context
//!
//! # Example
//!
//! ```ignore
//! use llmux::apply_and_verify::{apply_and_verify, ApplyVerifyConfig};
//!
//! let config = ApplyVerifyConfig {
//!     verify_command: Some("cargo test".into()),
//!     verify_retries: 2,
//!     ..Default::default()
//! };
//!
//! let result = apply_and_verify(llm_output, &config, working_dir).await?;
//!
//! if result.success {
//!     println!("Edits applied and verified!");
//! }
//! ```

mod diff_applier;
mod edit_parser;
mod retry_loop;
mod rollback;
mod verification;
