//! GARCH(1,1) volatility process. Stage 03 fills in.

/// Per-symbol volatility state for GARCH(1,1): σ²_t = ω + α·r²_{t-1} + β·σ²_{t-1}.
#[derive(Clone, Debug, Default)]
pub struct GarchState {
    pub variance: f64,
    pub prev_return: f64,
    pub omega: f64,
    pub alpha: f64,
    pub beta: f64,
}

impl GarchState {
    /// Advance one step. Stage 03 fills in.
    pub fn step(&mut self, _uniform_draw: f64) -> f64 {
        todo!("Stage 03 — GARCH step")
    }
}
