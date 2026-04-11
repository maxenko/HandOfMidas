//! Pure invariant checker for [`super::TickerOrderIntent`].
//!
//! Runs at two places:
//! 1. On load from disk — the actor drops any row that fails, with
//!    a `tracing::warn!`. (No quarantine sidecar in Slice 1a.)
//! 2. Before a write is coalesced — though Slice 1a does not yet
//!    wire this in; see Slice 3.

use midas_chart::widget::order_bracket::EntryType;

use crate::order_panel::OrderSide;

use super::{EntryMemory, TickerOrderIntent};

/// A structural or numeric defect in a [`TickerOrderIntent`].
///
/// Defective intents are dropped at load with a `tracing::warn!`. The
/// variants are informative — tests match on them to verify that the
/// right rule flagged the row.
#[derive(Debug, thiserror::Error)]
pub enum IntentDefect {
    /// A numeric field was NaN or infinity.
    #[error("non-finite value in field `{field}`")]
    NaN {
        /// The field that tripped the check.
        field: &'static str,
    },
    /// A quantity was negative. Negative qty is always invalid.
    #[error("negative value in field `{field}`")]
    Negative {
        /// The field that tripped the check.
        field: &'static str,
    },
    /// A price was outside ±5 × gatr_abs of `last_price`. This guard
    /// only fires when both `last_price` and `gatr_abs` are known.
    #[error("value out of band in field `{which}`")]
    OutOfBand {
        /// The field that tripped the check.
        which: &'static str,
    },
    /// A textual TP / SL value failed to parse when its enabled flag
    /// was set.
    #[error("failed to parse `{field}`: {reason}")]
    ParseFailed {
        /// Which textual field failed to parse.
        field: &'static str,
        /// The underlying parse error.
        reason: String,
    },
    /// Serde decoding failed at load time.
    #[error("failed to decode blob: {reason}")]
    DecodeFailed {
        /// The underlying serde error.
        reason: String,
    },
    /// TP on a Long bracket was below the entry (or TP on Short above).
    /// The plan calls this out as a load-time drop rule.
    #[error("TP on the wrong side of entry for bucket {bucket:?}")]
    TpWrongSide {
        /// The `(side, entry type)` compound key that tripped the check.
        bucket: (OrderSide, EntryType),
    },
    /// SL on a Long bracket was above the entry (or SL on Short below).
    #[error("SL on the wrong side of entry for bucket {bucket:?}")]
    SlWrongSide {
        /// The `(side, entry type)` compound key that tripped the check.
        bucket: (OrderSide, EntryType),
    },
}

/// Check every invariant on a [`TickerOrderIntent`].
///
/// * Numeric fields must be finite.
/// * Enabled TP / SL values must parse to a finite number.
/// * When `last_price` and `gatr_abs` are both known, every
///   price must lie within ±5 × `gatr_abs` of `last_price`.
/// * Long brackets must have TP above entry and SL below; Short is
///   mirrored. Only checked when the entry price is known.
pub fn validate(
    intent: &TickerOrderIntent,
    last_price: Option<f64>,
    gatr_abs: Option<f64>,
) -> Result<(), IntentDefect> {
    // GATR anchors must be finite (or None).
    if let Some(p) = intent.gatr_anchor.anchor_price {
        check_finite(p, "gatr_anchor.anchor_price")?;
    }
    if let Some(g) = intent.gatr_anchor.anchor_gatr {
        check_finite(g, "gatr_anchor.anchor_gatr")?;
        if g < 0.0 {
            return Err(IntentDefect::Negative {
                field: "gatr_anchor.anchor_gatr",
            });
        }
    }

    let band = match (last_price, gatr_abs) {
        (Some(p), Some(g)) if p.is_finite() && g.is_finite() && g > 0.0 => Some((p, 5.0 * g)),
        _ => None,
    };

    for (bucket, mem) in &intent.entries {
        check_entry_memory(*bucket, mem, band)?;
    }

    Ok(())
}

fn check_entry_memory(
    bucket: (OrderSide, EntryType),
    mem: &EntryMemory,
    band: Option<(f64, f64)>,
) -> Result<(), IntentDefect> {
    if let Some(p) = mem.entry_price_or_offset {
        check_finite(p, "entry_price_or_offset")?;
        if let Some((center, half_width)) = band {
            if (p - center).abs() > half_width {
                return Err(IntentDefect::OutOfBand {
                    which: "entry_price_or_offset",
                });
            }
        }
    }
    if let Some(q) = mem.quantity {
        check_finite(q, "quantity")?;
        if q < 0.0 {
            return Err(IntentDefect::Negative { field: "quantity" });
        }
    }

    let tp_price = if mem.tp_enabled {
        Some(parse_field(&mem.tp_value, "tp_value")?)
    } else {
        None
    };
    let sl_price = if mem.sl_enabled {
        Some(parse_field(&mem.sl_value, "sl_value")?)
    } else {
        None
    };
    if let (Some(tp), Some((center, half_width))) = (tp_price, band) {
        if (tp - center).abs() > half_width {
            return Err(IntentDefect::OutOfBand { which: "tp_value" });
        }
    }
    if let (Some(sl), Some((center, half_width))) = (sl_price, band) {
        if (sl - center).abs() > half_width {
            return Err(IntentDefect::OutOfBand { which: "sl_value" });
        }
    }

    // Directional sanity: TP above entry for Long, below for Short; SL mirrored.
    if let (Some(entry), Some(tp)) = (mem.entry_price_or_offset, tp_price) {
        let long = matches!(bucket.0, OrderSide::Buy);
        let ok = if long { tp >= entry } else { tp <= entry };
        if !ok {
            return Err(IntentDefect::TpWrongSide { bucket });
        }
    }
    if let (Some(entry), Some(sl)) = (mem.entry_price_or_offset, sl_price) {
        let long = matches!(bucket.0, OrderSide::Buy);
        let ok = if long { sl <= entry } else { sl >= entry };
        if !ok {
            return Err(IntentDefect::SlWrongSide { bucket });
        }
    }

    Ok(())
}

fn check_finite(v: f64, field: &'static str) -> Result<(), IntentDefect> {
    if !v.is_finite() {
        return Err(IntentDefect::NaN { field });
    }
    Ok(())
}

fn parse_field(raw: &str, field: &'static str) -> Result<f64, IntentDefect> {
    raw.trim()
        .parse::<f64>()
        .map_err(|e| IntentDefect::ParseFailed {
            field,
            reason: e.to_string(),
        })
        .and_then(|v| {
            if v.is_finite() {
                Ok(v)
            } else {
                Err(IntentDefect::NaN { field })
            }
        })
}
