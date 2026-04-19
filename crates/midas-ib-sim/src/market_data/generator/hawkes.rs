//! Hawkes-lite arrival process. Stage 03 fills in.

/// Exponentially-decaying excitement process for tick arrivals.
#[derive(Clone, Debug, Default)]
pub struct HawkesLite {
    pub baseline_rate: f64,
    pub excitement: f64,
    pub decay: f64,
}

impl HawkesLite {
    pub fn sample_next(&mut self, _uniform_draw: f64) -> f64 {
        todo!("Stage 03 — Hawkes next-arrival")
    }
}
