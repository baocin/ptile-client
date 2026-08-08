//! Trails against the published file, over the network.
//!
//! `#[ignore]` because it needs the host: `cargo test -p ptiles-ffi --test
//! trails_live -- --ignored`. The offline suites prove the decoder against
//! fixtures; this proves the decoder against what is actually being served,
//! which is the part a fixture cannot promise.
use ptiles_ffi::{trail_is_developed, PtilesError, PtilesLayer};

const TRAILS: &str = "https://maps.mydatatimeline.com/maps/TN.trails_v1.ptiles";

/// Great Smoky Mountains, near Newfound Gap -- dense, well-mapped trail country.
const SMOKIES_LAT: f64 = 35.611;
const SMOKIES_LON: f64 = -83.425;

/// Ring 1, not ring 0, for every query here: a trail is a long thin feature
/// that routinely runs along a cell edge for its whole length, so a ring-0
/// answer misses the trail underfoot whenever the walker is on the far side
/// of the boundary.
const RING: u8 = 1;

#[test]
#[ignore]
fn opens_and_decodes_the_published_trails_layer() {
    let layer = PtilesLayer::open(TRAILS.to_string()).expect("open trails_v1");
    let meta = layer.metadata();
    eprintln!("trails_v1: version {} blocks {}", meta.version, meta.block_count);
    assert!(meta.block_count > 0, "a published trails layer should carry blocks");
    // The `_v<N>` suffix is stripped at layer inference; the kind is `trails`.
    assert_eq!(meta.layer, "trails");

    let trails = layer.trails(SMOKIES_LAT, SMOKIES_LON, RING).expect("query trails");
    eprintln!("found {} trail features", trails.len());
    assert!(!trails.is_empty(), "the Smokies should have mapped trails");

    let named = trails.iter().filter(|t| t.name.is_some()).count();
    eprintln!("  {named} named, {} trailheads", trails.iter().filter(|t| t.is_trailhead).count());
    for t in trails.iter().take(5) {
        eprintln!("  {:?} type={} surface={:?} developed={}", t.name, t.trail_type, t.surface, t.developed);
    }
    // `is_trailhead` and `geom_type` are the same fact under two names; a
    // caller reading either must get the same answer.
    assert!(trails.iter().all(|t| t.is_trailhead == (t.geom_type == 1)));
}

#[test]
#[ignore]
fn finds_the_nearest_trail_and_skips_trailheads() {
    let layer = PtilesLayer::open(TRAILS.to_string()).expect("open trails_v1");
    let near = layer
        .nearest_trail(SMOKIES_LAT, SMOKIES_LON, RING)
        .expect("nearest_trail")
        .expect("a trail near Newfound Gap");
    eprintln!(
        "nearest: {:?} type={} {:.1} m on_it={} developed={} osm_id={:?}",
        near.name,
        near.class,
        near.distance_m,
        near.on_it,
        trail_is_developed(near.class.clone()),
        near.osm_id,
    );
    assert_eq!(near.kind, "trail");
    assert!(near.distance_m >= 0.0);
    assert!(near.osm_id.is_some(), "the lookup owns the slice, so it can name the feature");
    // nearest_trail answers "which trail am I walking on", so it must return a
    // length of trail rather than the sign at its entrance.
    assert_ne!(near.class, "trailhead", "a trailhead point is not an answer to 'which trail'");

    // The trailhead lookup answers the other question, from the same file.
    let head = layer
        .nearest_trailhead(SMOKIES_LAT, SMOKIES_LON, RING)
        .expect("nearest_trailhead");
    if let Some(h) = head {
        eprintln!("nearest trailhead: {:?} {:.1} m", h.name, h.distance_m);
        assert_eq!(h.kind, "trailhead");
    }
}

#[test]
#[ignore]
fn refuses_a_roads_query_on_a_trails_layer() {
    // The layer-kind guard is what stops a trails file being decoded with the
    // roads framing and returning confident nonsense.
    let layer = PtilesLayer::open(TRAILS.to_string()).expect("open trails_v1");
    match layer.nearest_road(SMOKIES_LAT, SMOKIES_LON) {
        Err(PtilesError::UnsupportedForLayer { layer }) => assert_eq!(layer, "trails"),
        other => panic!("expected UnsupportedForLayer, got {other:?}"),
    }
}
