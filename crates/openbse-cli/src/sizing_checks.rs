//! Checks on sized equipment, run once sizing has filled every `autosize`.
//!
//! A DX coil's performance curves and rated bypass factor hold only near its
//! rated ratio of air flow to capacity. Far below it, the coil chills a small
//! airflow by a large capacity and its outlet can run below freezing; far
//! above it, the coil barely dehumidifies. EnergyPlus warns when the rated air
//! volume flow per watt of rated total capacity leaves 300–450 cfm/ton
//! (`DXCoils.cc` `SizeDXCoil`), for cooling and heating DX coils alike, and its
//! capacity sizing holds autosized capacities inside the same range. OpenBSE's
//! capacity sizing does the same (`openbse_io::system_sizing::dx_rated_capacity`),
//! so this check finds coils whose airflow or capacity was given by hand.

use openbse_core::graph::{GraphComponent, SimulationGraph};
use openbse_core::types::{is_autosize, DX_FLOW_PER_CAPACITY};

/// cfm/ton per m³/s/W: 2118.88 cfm per m³/s × 3516.85 W per ton.
const CFM_PER_TON: f64 = 2118.88 * 3516.85;
/// E+ `SmallDifferenceTest` on the ratio [m³/s/W].
const TOLERANCE: f64 = 1.0e-8;

/// Why a flow-per-capacity ratio is out of range, if it is.
fn out_of_range(flow: f64, capacity: f64) -> Option<f64> {
    if flow <= 0.0 || capacity <= 0.0 || is_autosize(flow) || is_autosize(capacity) {
        return None;
    }
    let ratio = flow / capacity;
    let (lo, hi) = DX_FLOW_PER_CAPACITY;
    (ratio < lo - TOLERANCE || ratio > hi + TOLERANCE).then_some(ratio)
}

/// One warning per DX coil speed whose rated air volume flow per watt of rated
/// total capacity is outside [`DX_FLOW_PER_CAPACITY`].
pub fn dx_flow_per_capacity_warnings(graph: &SimulationGraph) -> Vec<String> {
    let (lo, hi) = DX_FLOW_PER_CAPACITY;
    let mut warnings = Vec::new();
    for component in graph.components() {
        let GraphComponent::Air(c) = component else {
            continue;
        };
        let points = c.dx_rated_points();
        for (i, &(flow, capacity)) in points.iter().enumerate() {
            let Some(ratio) = out_of_range(flow, capacity) else {
                continue;
            };
            let speed = if points.len() > 1 {
                format!(" speed {} of {}", i + 1, points.len())
            } else {
                String::new()
            };
            warnings.push(format!(
                "DX coil '{}'{speed}: rated airflow {flow:.3} m³/s for {capacity:.0} W of rated \
                 total capacity is {ratio:.3e} m³/s per W ({:.0} cfm/ton), outside the \
                 {lo:.3e}–{hi:.3e} m³/s per W ({:.0}–{:.0} cfm/ton) its performance curves hold \
                 in (EnergyPlus warns on the same range). Give the airflow and capacity from \
                 the same source, or autosize both",
                c.name(),
                ratio * CFM_PER_TON,
                lo * CFM_PER_TON,
                hi * CFM_PER_TON,
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_matches_energyplus_cfm_per_ton() {
        let (lo, hi) = DX_FLOW_PER_CAPACITY;
        assert!((lo * CFM_PER_TON - 300.0).abs() < 0.5);
        assert!((hi * CFM_PER_TON - 450.0).abs() < 0.5);
    }

    #[test]
    fn ratio_outside_the_range_is_flagged() {
        // doe_retail_standalone's Core Retail coil before its sizing inputs
        // matched the prototype: 2.726 m³/s for 137,784 W.
        let r = out_of_range(2.726, 137_784.0).expect("148 cfm/ton is low");
        assert!((r * CFM_PER_TON - 147.4).abs() < 1.0, "{}", r * CFM_PER_TON);
        // The prototype's own coil: 8.094 m³/s, 438 cfm/ton.
        assert!(out_of_range(8.09372, 137_784.2).is_none());
        assert!(out_of_range(0.07, 1000.0).is_some(), "520 cfm/ton is high");
        // Unsized or absent values are the autosize check's business.
        assert!(out_of_range(openbse_core::types::AUTOSIZE, 1000.0).is_none());
        assert!(out_of_range(0.05, 0.0).is_none());
    }
}
