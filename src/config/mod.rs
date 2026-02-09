//! Configuration types and loading for llmux

mod backend;
mod error;
mod loader;
mod role;
mod workflow;

pub use backend::BackendConfig;
pub use loader::{LlmuxConfig, StepResult, load_workflow};
#[allow(unused_imports)]
pub use role::{RoleConfig, RoleExecution, RoleOverride, TeamConfig};
#[allow(unused_imports)]
pub use workflow::{OutputSchema, PropertySchema, StepConfig, StepType, WorkflowConfig};
