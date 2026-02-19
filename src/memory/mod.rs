//! Ecosystem memory - persistent storage for knowledge and findings

mod schema;
mod store;

#[allow(unused_imports)]
pub use store::{
    EcosystemMemory, Entity, EntityProperty, Fact, Finding, ProjectRelationship, WorkflowRun,
};
