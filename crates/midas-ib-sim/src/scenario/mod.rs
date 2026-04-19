//! YAML scenario runner. Stage 06 fills in.

pub mod injector;
pub mod script;

pub use self::script::{Scenario, ScenarioStep};

use std::path::Path;

/// Runtime handle for an in-flight scenario. Stage 06 implements.
pub struct ScenarioRunner {
    _priv: (),
}

impl ScenarioRunner {
    pub fn load(_path: &Path) -> Result<Scenario, ScenarioError> {
        todo!("Stage 06 — ScenarioRunner::load")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("yaml parse error: {0}")]
    Parse(String),
    #[error("unknown verb: {0}")]
    UnknownVerb(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
