//! Writes the canonical scenario JSON Schema to
//! `fixtures/scenarios/schema/v1.json`.
//!
//! Invoke with `cargo run -p midas-ib-sim --example export_scenario_schema`
//! whenever the `Scenario` type changes; the regenerated file should be
//! committed alongside the schema edit.

use std::path::PathBuf;

use midas_ib_sim::scenario::json_schema;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("scenarios")
        .join("schema")
        .join("v1.json");
    json_schema::write_schema_to(&target)?;
    println!("wrote {}", target.display());
    Ok(())
}
