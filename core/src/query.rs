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

/// Hard cap on the number of cells `cells_for_bounds` will return. A viewport
/// this large (e.g. res-7 cells, ~5.16 km^2 each, cover roughly a state at
/// this count) means the caller almost certainly should be zoomed in, using
/// a coarser resolution, or paging the request rather than asking this
/// library to walk thousands of individual blocks. Chosen to match the
/// demo's own client-side cap (`cells.slice(0, 300)` in
/// steele.red/ptiles/index.html) with headroom.
pub const MAX_BOUNDS_CELLS: usize = 512;

/// `cells_for_bounds` failure: the requested bbox would cover more than
/// [`MAX_BOUNDS_CELLS`] res-7 cells.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum BoundsError {
    #[error(
        "bounding box ({min_lat}, {min_lon})..({max_lat}, {max_lon}) is too large: \
         covers more than {max} H3 res-7 cells (cap is {max}); zoom in or split the request"
    )]
    TooManyCells {
        min_lat: OrderedF64,
        min_lon: OrderedF64,
        max_lat: OrderedF64,
        max_lon: OrderedF64,
        max: usize,
    },
    #[error(
        "invalid bounding box: min ({min_lat}, {min_lon}) must be <= max ({max_lat}, {max_lon}) and all coordinates finite"
    )]
    InvalidBounds {
        min_lat: OrderedF64,
        min_lon: OrderedF64,
        max_lat: OrderedF64,
        max_lon: OrderedF64,
    },
}

/// Thin wrapper so `f64` can appear in a `PartialEq`/`Eq`-deriving error enum
/// (this crate has no other need for a general-purpose ordered-float type,
/// so this stays local rather than pulling in a crate for it). Only used for
/// display/equality in `BoundsError`, never arithmetic.
#[derive(Debug, Clone, Copy)]
pub struct OrderedF64(pub f64);
impl PartialEq for OrderedF64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for OrderedF64 {}
impl core::fmt::Display for OrderedF64 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.0, f)
    }
}

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
    try_cell_center(cell).unwrap_or((0.0, 0.0))
}

/// Centre of an H3 cell, or `None` if `cell` is not a valid H3 index.
///
/// [`cell_center`] answers `(0.0, 0.0)` for an invalid id, and that fallback has
/// cost real debugging: a caller that masks an id's filler bits before asking
/// gets null island back, and a decoder that positions its records relative to
/// that centre produces geometry ~9,700 km from where it belongs -- plausible
/// records, wrong planet, no error anywhere. Anything deriving a *position* from
/// a cell should use this and handle the `None`, which is why
/// [`crate::decode_buildings_for_cell`] exists.
pub fn try_cell_center(cell: u64) -> Option<(f64, f64)> {
    let idx = CellIndex::try_from(cell).ok()?;
    let ll = LatLng::from(idx);
    Some((ll.lat(), ll.lng()))
}

/// Ring-1 neighbors of `cell` at the same resolution, **excluding** the
/// center cell itself -- 6 cells for a normal hexagon cell (SPEC.md step 6:
/// "6 neighbors at res 7"), fewer near a pentagon.
///
/// Returns an empty `Vec` if `cell` is not a valid H3 cell index (e.g. the
/// `0` sentinel from `cell_for_coord` on invalid input).
pub fn neighbor_cells(cell: u64) -> Vec<u64> {
    match CellIndex::try_from(cell) {
        Ok(idx) => idx
            .grid_ring::<Vec<_>>(1)
            .into_iter()
            .map(u64::from)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// H3 res-7 cells covering a lat/lon bounding box -- the "viewport -> cell
/// list" step of the loading pattern described in
/// `ptiles-client/docs/INTEGRATION.md` (mirrors the demo's
/// `h3.polygonToCells([...], 7)` call, see steele.red/ptiles/index.html).
///
/// This does not depend on h3o's `geo` feature (which would pull in `geo`/
/// `geo-types` and force `std`, at odds with this crate's no_std-optional
/// stance, see core/Cargo.toml). Instead it's a flood fill from the bbox's
/// center cell: pop a cell, keep it if its hexagonal boundary (`.boundary()`,
/// core h3o, no extra feature) overlaps the bbox, and if so enqueue its
/// ring-1 neighbors. Cells that don't overlap are dropped without expanding
/// past them, which bounds the fill to (approximately) the bbox's footprint
/// instead of spreading over the whole globe. This is an approximation, not
/// exact polygon-cell coverage: for a normal (non-degenerate, several-cells-
/// wide) bbox it matches true polyfill, but on a bbox so thin one of its
/// cell-width "waists" pinches to a non-intersecting cell, a run of cells on
/// the far side of that waist could be missed. Good enough for viewport
/// queries (which are never that pathological); exact polygon coverage is
/// `h3o::geom::TilerBuilder`, not used here for the reason above.
///
/// A zero-area bbox (`min == max`, i.e. a single point) is valid and returns
/// exactly the one cell covering that point; a sub-cell bbox likewise returns
/// its covering cell. Only an inverted bbox (`min > max`) or non-finite input
/// is rejected.
///
/// Errors if any input is non-finite, `min` is not `<=` `max`, or covering
/// the box would need more than [`MAX_BOUNDS_CELLS`] cells (a bbox that
/// large means the caller should zoom in rather than pull hundreds of
/// blocks at once).
pub fn cells_for_bounds(
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
) -> Result<Vec<u64>, BoundsError> {
    let invalid = || BoundsError::InvalidBounds {
        min_lat: OrderedF64(min_lat),
        min_lon: OrderedF64(min_lon),
        max_lat: OrderedF64(max_lat),
        max_lon: OrderedF64(max_lon),
    };
    if ![min_lat, min_lon, max_lat, max_lon]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(invalid());
    }
    if min_lat > max_lat || min_lon > max_lon {
        return Err(invalid());
    }

    let center_lat = (min_lat + max_lat) / 2.0;
    let center_lon = (min_lon + max_lon) / 2.0;
    let start = match LatLng::new(center_lat, center_lon) {
        Ok(ll) => ll.to_cell(RESOLUTION),
        Err(_) => return Err(invalid()),
    };

    let overlaps = |cell: CellIndex| -> bool {
        cell.boundary().iter().any(|ll| {
            ll.lat() >= min_lat && ll.lat() <= max_lat && ll.lng() >= min_lon && ll.lng() <= max_lon
        })
    };

    let mut visited: alloc::collections::BTreeSet<u64> = alloc::collections::BTreeSet::new();
    let mut queue: alloc::collections::VecDeque<CellIndex> = alloc::collections::VecDeque::new();
    let mut result = Vec::new();

    // The bbox center is by definition inside its own cell, so the start cell
    // always covers (part of) the box -- include it unconditionally rather
    // than gating it on the boundary-vertex overlap test. Without this, a bbox
    // smaller than a single res-7 cell (a zero-area point query, or any
    // viewport a few metres across) has *none* of the start cell's boundary
    // vertices inside it and would return an empty list, silently dropping the
    // one cell that actually contains the query. Neighbors are still gated on
    // `overlaps` so the fill stays bounded to the box's footprint.
    visited.insert(u64::from(start));
    result.push(u64::from(start));
    for neighbor in start.grid_ring::<Vec<_>>(1) {
        if visited.insert(u64::from(neighbor)) {
            queue.push_back(neighbor);
        }
    }

    while let Some(cell) = queue.pop_front() {
        if !overlaps(cell) {
            continue;
        }
        result.push(u64::from(cell));
        if result.len() > MAX_BOUNDS_CELLS {
            return Err(BoundsError::TooManyCells {
                min_lat: OrderedF64(min_lat),
                min_lon: OrderedF64(min_lon),
                max_lat: OrderedF64(max_lat),
                max_lon: OrderedF64(max_lon),
                max: MAX_BOUNDS_CELLS,
            });
        }
        for neighbor in cell.grid_ring::<Vec<_>>(1) {
            if visited.insert(u64::from(neighbor)) {
                queue.push_back(neighbor);
            }
        }
    }

    Ok(result)
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
        assert!(
            !neighbors.contains(&cell),
            "ring must exclude the center cell"
        );
        for n in &neighbors {
            let idx = CellIndex::try_from(*n).expect("neighbor must be a valid cell index");
            assert_eq!(idx.resolution(), Resolution::Seven);
        }
    }

    #[test]
    fn neighbor_cells_invalid_input_returns_empty() {
        assert!(neighbor_cells(0).is_empty());
    }

    /// Small bbox around downtown Nashville (a few blocks) -- should
    /// resolve to a handful of res-7 cells, one of which is the cell
    /// covering Music City Center itself.
    #[test]
    fn cells_for_bounds_small_bbox_contains_downtown_cell() {
        let downtown_cell = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        let cells =
            cells_for_bounds(36.14, -86.80, 36.18, -86.76).expect("small bbox must not error");
        assert!(!cells.is_empty());
        assert!(
            cells.contains(&downtown_cell),
            "expected downtown cell {downtown_cell} in {cells:?}"
        );
        for c in &cells {
            let idx = CellIndex::try_from(*c).expect("must be a valid cell index");
            assert_eq!(idx.resolution(), Resolution::Seven);
        }
    }

    #[test]
    fn cells_for_bounds_oversized_bbox_errors() {
        // Roughly the continental US -- far more than MAX_BOUNDS_CELLS
        // res-7 cells (~5.16 km^2 each).
        let result = cells_for_bounds(24.0, -125.0, 49.0, -66.0);
        assert!(matches!(result, Err(BoundsError::TooManyCells { .. })));
    }

    #[test]
    fn cells_for_bounds_invalid_input_errors() {
        assert!(matches!(
            cells_for_bounds(f64::NAN, 0.0, 1.0, 1.0),
            Err(BoundsError::InvalidBounds { .. })
        ));
        // min > max
        assert!(matches!(
            cells_for_bounds(10.0, 10.0, 5.0, 5.0),
            Err(BoundsError::InvalidBounds { .. })
        ));
    }

    /// Only lon inverted (lat ok) must still be rejected -- the check is
    /// per-axis, not a single combined comparison.
    #[test]
    fn cells_for_bounds_partially_inverted_errors() {
        // min_lat <= max_lat but min_lon > max_lon.
        assert!(matches!(
            cells_for_bounds(36.14, -86.70, 36.18, -86.80),
            Err(BoundsError::InvalidBounds { .. })
        ));
    }

    #[test]
    fn cells_for_bounds_infinite_bound_errors() {
        assert!(matches!(
            cells_for_bounds(0.0, 0.0, f64::INFINITY, 1.0),
            Err(BoundsError::InvalidBounds { .. })
        ));
        assert!(matches!(
            cells_for_bounds(0.0, 0.0, 1.0, f64::NEG_INFINITY),
            Err(BoundsError::InvalidBounds { .. })
        ));
    }

    /// A zero-area bbox (a single point) is valid and must return exactly the
    /// one cell that covers the point -- the same cell `cell_for_coord`
    /// resolves. Regression test for the sub-cell-bbox-returns-empty bug: the
    /// point is smaller than a res-7 cell so none of that cell's boundary
    /// vertices fall inside the (degenerate) box, yet the covering cell must
    /// still be returned.
    #[test]
    fn cells_for_bounds_zero_area_returns_covering_cell() {
        let cell = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        let cells = cells_for_bounds(NASHVILLE_LAT, NASHVILLE_LON, NASHVILLE_LAT, NASHVILLE_LON)
            .expect("point bbox must be valid");
        assert_eq!(
            cells,
            alloc::vec![cell],
            "point query must yield its one covering cell"
        );
    }

    /// A bbox a few metres across (far smaller than a ~5.16 km^2 res-7 cell)
    /// must not return empty -- same regression as the zero-area case for a
    /// non-degenerate but sub-cell box.
    #[test]
    fn cells_for_bounds_tiny_bbox_is_not_empty() {
        let cell = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        // ~11 m in latitude, ~9 m in longitude at this latitude.
        let cells = cells_for_bounds(
            NASHVILLE_LAT - 0.0001,
            NASHVILLE_LON - 0.0001,
            NASHVILLE_LAT + 0.0001,
            NASHVILLE_LON + 0.0001,
        )
        .expect("tiny bbox must be valid");
        assert!(!cells.is_empty(), "sub-cell bbox must not return empty");
        assert!(cells.contains(&cell), "must include the covering cell");
    }

    /// Every returned cell must be unique -- the flood fill dedups via the
    /// `visited` set, and the unconditionally-seeded start cell must not be
    /// re-added by a neighbor expansion.
    #[test]
    fn cells_for_bounds_returns_no_duplicates() {
        let cells = cells_for_bounds(36.10, -86.85, 36.22, -86.72).expect("bbox must be valid");
        let mut sorted = cells.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            cells.len(),
            "result must contain no duplicate cells"
        );
        assert!(
            cells.len() > 1,
            "a multi-cell viewport must expand past the start cell"
        );
    }

    /// The center cell of a normal viewport is included, and it matches the
    /// cell the point-resolver returns for the bbox center.
    #[test]
    fn cells_for_bounds_includes_center_cell() {
        let center_cell = cell_for_coord((36.14 + 36.18) / 2.0, (-86.80 + -86.76) / 2.0);
        let cells = cells_for_bounds(36.14, -86.80, 36.18, -86.76).expect("bbox must be valid");
        assert!(cells.contains(&center_cell), "center cell must be covered");
    }

    #[test]
    fn cell_center_roundtrips_through_cell_for_coord() {
        let cell = cell_for_coord(NASHVILLE_LAT, NASHVILLE_LON);
        let (clat, clon) = cell_center(cell);
        // The cell center resolves back to the same cell.
        assert_eq!(cell_for_coord(clat, clon), cell);
    }

    #[test]
    fn cell_center_invalid_cell_is_origin_not_panic() {
        assert_eq!(cell_center(0), (0.0, 0.0));
    }
}
