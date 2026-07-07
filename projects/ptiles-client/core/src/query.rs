//! H3 cell resolution (res-7 fixed by spec, SPEC.md:292-299) and ring-1
//! neighbor lookup via `h3o`.
//!
//! The demo (steele.red/ptiles/index.html) always resolves at
//! `h3.latLngToCell(lat, lon, 7)` and, when it needs neighboring cells
//! (nearest-road edge handling, business radius query), expands with
//! `h3.gridRing(cellHex, 1)` — ring-1 only, **excluding** the center cell,
//! which is 6 cells for a non-pentagon (matches SPEC.md step 6: "check
//! neighboring H3 cells (6 neighbors at res 7)"). `neighbor_cells` mirrors
//! that: it does not include the center cell itself.

use alloc::vec::Vec;

use h3o::{CellIndex, LatLng, Resolution};

/// H3 resolution used by every ptiles layer (SPEC.md fixes this at 7).
const RESOLUTION: Resolution = Resolution::Seven;

/// Resolve `(lat, lon)` in degrees to its H3 res-7 cell index.
///
/// Returns `0` for non-finite/out-of-range input (mirrors `h3o::LatLng::new`
/// rejecting non-finite coordinates) rather than panicking -- callers doing
/// GPS-derived queries should not be able to crash a lookup on bad input.
/// `0` is never a valid `CellIndex` (see `CellIndex::try_from`), so it is
/// safe to use as an unambiguous invalid-input sentinel.
pub fn cell_for_coord(lat: f64, lon: f64) -> u64 {
    match LatLng::new(lat, lon) {
        Ok(ll) => u64::from(ll.to_cell(RESOLUTION)),
        Err(_) => 0,
    }
}

/// Center `(lat, lon)` in degrees of an H3 cell. Used by callers (e.g. the
/// v8 buildings decoder, see `buildings.rs`'s doc comment) that need the
/// cell's center to reconstruct cell-relative-delta coordinates. Returns
/// `(0.0, 0.0)` for an invalid `cell` id (mirrors `cell_for_coord`'s
/// no-panic-on-bad-input stance).
pub fn cell_center(cell: u64) -> (f64, f64) {
    match CellIndex::try_from(cell) {
        Ok(idx) => {
            let ll = LatLng::from(idx);
            (ll.lat(), ll.lng())
        }
        Err(_) => (0.0, 0.0),
    }
}

/// Ring-1 neighbors of `cell` at the same resolution, **excluding** the
/// center cell itself -- 6 cells for a normal hexagon cell (SPEC.md step 6:
/// "6 neighbors at res 7"), fewer near a pentagon.
///
/// Returns an empty `Vec` if `cell` is not a valid H3 cell index (e.g. the
/// `0` sentinel from `cell_for_coord` on invalid input).
pub fn neighbor_cells(cell: u64) -> Vec<u64> {
    match CellIndex::try_from(cell) {
        Ok(idx) => idx.grid_ring::<Vec<_>>(1).into_iter().map(u64::from).collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nashville, TN (Music City Center area). Expected cell computed with
    /// `h3o` itself (`LatLng::new(36.16, -86.78).unwrap().to_cell(Resolution::Seven)`)
    /// then sanity-checked independently below (resolution, validity,
    /// round-trip through `CellIndex::try_from(u64)`, and that
    /// `cell_for_coord` reproduces the same value deterministically).
    const NASHVILLE_LAT: f64 = 36.16;
    const NASHVILLE_LON: f64 = -86.78;

    #[test]
    fn cell_for_coord_is_resolution_7_and_valid() {
        let cell = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        assert_ne!(cell, 0);
        let idx = CellIndex::try_from(cell).expect("must be a valid H3 cell index");
        assert_eq!(idx.resolution(), Resolution::Seven);
    }

    #[test]
    fn cell_for_coord_matches_h3o_direct() {
        let expected = u64::from(
            LatLng::new(NASHVILLE_LAT, NASHVILLE_LON)
                .unwrap()
                .to_cell(Resolution::Seven),
        );
        assert_eq!(cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON), expected);
    }

    #[test]
    fn cell_for_coord_is_deterministic() {
        let a = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        let b = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        assert_eq!(a, b);
    }

    #[test]
    fn cell_for_coord_invalid_input_is_sentinel_not_panic() {
        assert_eq!(cell_for_coord(f64::NAN, 0.0), 0);
        assert_eq!(cell_for_coord(0.0, f64::INFINITY), 0);
    }

    #[test]
    fn neighbor_cells_returns_six_for_nashville() {
        let cell = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        let neighbors = neighbor_cells(cell);
        // Ring-1 excluding center is 6 cells away from any pentagon.
        assert_eq!(neighbors.len(), 6);
        assert!(!neighbors.contains(&cell), "ring must exclude the center cell");
        for n in &neighbors {
            let idx = CellIndex::try_from(*n).expect("neighbor must be a valid cell index");
            assert_eq!(idx.resolution(), Resolution::Seven);
        }
    }

    #[test]
    fn neighbor_cells_invalid_input_returns_empty() {
        assert!(neighbor_cells(0).is_empty());
    }
}
