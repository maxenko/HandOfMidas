//! Scenario verb → engine command translation.
//!
//! Stage 06-skeleton only declares the entry point; Wave 2's runner fills it
//! in. Keeping the stub on the public path reserves the module name and
//! prevents accidental divergence between the verb enum and the dispatcher.

use crate::engine::types::EngineCmd;
use crate::scenario::schema::Verb;

/// Translate a parsed verb into the engine command(s) it represents.
///
/// Returns `None` for verbs that are scenario-local (e.g. `Sleep`, `Include`,
/// `Assert*`) and thus have no engine-side effect. Wave 2 implements.
pub fn verb_to_cmd(_verb: &Verb) -> Option<EngineCmd> {
    // Stage 06 skeleton: no dispatch yet. Keeping this as an explicit `None`
    // (rather than `todo!()`) lets the crate boot with scenarios loaded but
    // inert — valuable for tests that only exercise the loader.
    None
}
