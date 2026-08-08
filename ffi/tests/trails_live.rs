//! Trails against the published file, over the network.
//!
//! `#[ignore]` because it needs the host: `cargo test -p ptiles-ffi --test
//! trails_live -- --ignored`. The offline suites prove the decoder against
//! fixtures; this proves the decoder against what is actually being served,
//! which is the part a fixture cannot promise.
use ptiles_ffi::{PtilesError, PtilesLayer};

const TRAILS: &str = "https://maps.mydatatimeline.com/maps/TN.trails_v1.ptiles";

/// Great Smoky Mountains, near Newfound Gap -- dense, well-mapped trail country.
const SMOKIES_LAT: f64 = 35.611;
const SMOKIES_LON: f64 = -83.425;

#[test]
#[ignore]
fn opens_and_decodes_the_published_trails_layer() {
    let layer = PtilesLayer::open(TRAILS.to_string()).expect("open trails_v1");
    let meta = layer.metadata();
    eprintln!("trails_v1: version {} blocks {}", meta.version, meta.block_count);
    assert!(meta.block_count > 0, "a published trails layer should carry blocks");

    let trails = layer.trails(SMOKIES_LAT, SMOKIES_LON, 1).expect("query trails");
    eprintln!("found {} trail features", trails.len());
    assert!(!trails.is_empty(), "the Smokies should have mapped trails");

    let named = trails.iter().filter(|t| t.name.is_some()).count();
    eprintln!("  {named} named, {} trailheads", trails.iter().filter(|t| t.is_trailhead).count());
    for t in trails.iter().take(5) {
        eprintln!("  {:?} type={} surface={:?} developed={}", t.name, t.trail_type, t.surface, t.developed);
    }
}

#[test]
#[ignore]
fn finds_the_nearest_trail_and_skips_trailheads() {
    let layer = PtilesLayer::open(TRAILS.to_string()).expect("open trails_v1");
    let near = layer.nearest_trail(SMOKIES_LAT, SMOKIES_LON).expect("nearest_trail");
    let near = near.expect("a trail near Newfound Gap");
    eprintln!(
        "nearest: {:?} type={} {:.1} m on_it={} developed={}",
        near.name, near.trail_type, near.distance_m, near.on_it, near.developed
    );
    assert!(near.distance_m >= 0.0);
    // nearest_trail answers "which trail am I walking on", so it must return a
    // length of trail rather than the sign at its entrance.
    assert!(near.geometry.len() > 1, "a trailhead point is not an answer to 'which trail'");
}

#[test]
#[ignore]
fn refuses_a_roads_query_on_a_trails_layer() {
    // The layer-kind guard is what stops a trails file being decoded with the
    // roads framing and returning confident nonsense.
    let layer = PtilesLayer::open(TRAILS.to_string()).expect("open trails_v1");
    match layer.nearest_road(SMOKIES_LAT, SMOKIES_LON) {
        Err(PtilesError::UnsupportedForLayer { layer }) => assert_eq!(layer, "trails_v1"),
        other => panic!("expected UnsupportedForLayer, got {other:?}"),
    }
}
