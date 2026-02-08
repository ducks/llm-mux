//! Configuration types and loading for llmux

mod backend;
mod error;
mod loader;
mod role;
mod workflow;

pub use backend::BackendConfig;
pub use error::{ErrorKind, StepError};
pub use loader::{load_workflow, Defaults, LlmuxConfig, StepResult};
pub use role::{RoleConfig, RoleExecution, RoleOverride, TeamConfig};
pub use workflow::{ArgDef, OutputSchema, PropertySchema, StepConfig, StepType, WorkflowConfig};
