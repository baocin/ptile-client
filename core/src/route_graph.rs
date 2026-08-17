//! Corridor graph routing from decoded road segments (browser/phone path).
//!
//! Port of daemon `build_graph` + A* ideas from
//! `timeline/ptiles/src/router.rs`, simplified for ptiles-core (no H3 cell,
//! no intersection delays, no FxHashMap). JS loads corridor cells; this
//! only builds a graph and searches.

use alloc::collections::{BTreeMap, BinaryHeap};
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Reverse;

use crate::math;
use crate::proximity::haversine_distance_m;
use crate::roads::RoadSegment;

/// Weight unit: centiseconds (1/100 s), matching the daemon.
type Weight = u64;

const SPEED_FACTOR: f64 = 0.85;
/// ~11 m at 50k micro-scale for cross-road merge.
/// ponytail: tighter than daemon's 10 (~22m) so path follows curves instead of chords.
const MERGE_THRESH: i32 = 5;
/// Ceiling on graph nodes, which is a memory guard rather than a policy.
///
/// A 120 km corridor with its middle pruned to arterials still builds past
/// 250,000 nodes, and the failure was a hard stop on an ordinary inter-city
/// drive. At ~26 bytes a node plus adjacency this is tens of megabytes, not
/// hundreds, and a phone that can hold the decoded segments can hold the
/// graph they build.
const NODE_CAP: usize = 600_000;
const BI_ASTAR_MIN_NODES: usize = 50_000;

/// Why a graph route could not be produced.
///
/// The original API returned only `None`, which made a disconnected corridor
/// indistinguishable from a bad endpoint snap or the browser safety cap.  The
/// browser needs the distinction: widening is useful only for a disconnected
/// graph, and repeating a node-budget failure just does more work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum RouteFailure {
    EmptyGraph,
    StartNotSnapped,
    EndNotSnapped,
    Disconnected,
    NodeBudgetExceeded,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RouteResult {
    pub distance_m: f64,
    pub duration_s: f64,
    /// Leaflet order: `[lat, lon]`.
    pub path: Vec<[f64; 2]>,
}

/// Optional routing preferences.
///
/// Both are **penalties, not prohibitions**. A hard ban returns "no route" the
/// moment the only river crossing for miles is a trunk bridge, or the only way
/// out of a subdivision is its one signalised exit; a penalty routes around
/// when there is an alternative and still gets you there when there is not.
///
/// They are applied to edge weights at graph build time, so the A* heuristic
/// (free-flow travel time, a lower bound) stays admissible: penalties only ever
/// raise a cost, never lower one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RoutePrefs {
    /// What is doing the travelling. Decides which classes are routable at
    /// all and how fast they are; the two flags below are driving concerns
    /// and do nothing on foot.
    pub profile: RouteProfile,
    /// Multiply motorway/trunk edge time so the route prefers arterials.
    pub avoid_highways: bool,
    /// Charge time for passing through junctions, so the route prefers fewer
    /// of them even when that means a slightly longer road.
    pub avoid_intersections: bool,
}

/// Who the route is for.
///
/// The trails layer decodes into the same shape roads do (see
/// [`trail_segments`]), so the graph builder needs no second implementation --
/// but a footpath is not a road with a low speed limit. It is routable where
/// a car is not, forbidden where a car is fine, and a staircase costs more
/// per metre than flat ground rather than less.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RouteProfile {
    #[default]
    Driving,
    /// Walking: trails, tracks, footways and steps, plus the quiet street
    /// classes a pedestrian legitimately uses. Motorways and trunk roads are
    /// excluded -- they are the one place walking is actually prohibited.
    Foot,
}

/// Walking speeds, km/h. Flat ground is the usual 5 km/h; steps and rough
/// alpine paths are slower, and a cycleway is not faster on foot -- it is
/// just smoother, which the surface field already says.
fn foot_speed_kmh(class: &str) -> f64 {
    match class {
        "steps" => 1.5,
        "path" | "bridleway" => 4.5,
        "track" => 4.5,
        _ => 5.0,
    }
}

/// Cost multiplier for a highway edge under `avoid_highways`. Chosen so a
/// motorway at 105 km/h costs about what a 25 km/h residential street does per
/// metre: enough to reject a highway used for convenience, not enough to make
/// a 30 km detour look attractive.
const HIGHWAY_PENALTY: f64 = 4.0;

/// Seconds charged per junction arm beyond a simple two-way node, under
/// `avoid_intersections`. A four-way costs 2 arms x this; roughly the delay of
/// a signal cycle, split across the edges that enter it.
const INTERSECTION_PENALTY_S: f64 = 12.0;

fn is_highway_class(class: &str) -> bool {
    matches!(
        class,
        "motorway" | "motorway_link" | "trunk" | "trunk_link"
    )
}

#[inline]
fn weight_from_seconds(seconds: f64) -> Weight {
    math::round(seconds * 100.0) as Weight
}

#[inline]
fn weight_to_seconds(w: Weight) -> f64 {
    w as f64 / 100.0
}

/// Driving profile filter (daemon `profile_allows("driving")`).
/// ponytail: drop `service` — parking aisles explode node count; local A→B
/// works on residential+arterial alone.
/// Whether a class is routable on foot.
///
/// Trail classes plus the street classes a pedestrian uses in practice: OSM
/// tags most American sidewalks as part of the road, so excluding residential
/// and service streets would disconnect every trailhead from every street it
/// meets. Motorway and trunk are the genuine exclusions.
pub fn profile_allows_foot(class: &str) -> bool {
    matches!(
        class,
        "path"
            | "footway"
            | "steps"
            | "bridleway"
            | "cycleway"
            | "track"
            | "pedestrian"
            | "living_street"
            | "residential"
            | "unclassified"
            | "service"
            | "tertiary"
            | "tertiary_link"
            | "secondary"
            | "secondary_link"
    )
}

/// Trails as router input.
///
/// `TrailFeature` and `RoadSegment` carry the same thing -- a named linestring
/// with a class -- so the graph builder needs no second implementation and a
/// mixed walk down a path and along a residential street works without the
/// two halves ever meeting a conversion. The trail type lands in
/// `road_class`, which is what [`profile_allows_foot`] and the foot speeds
/// read.
///
/// Trailhead points are skipped: a point is not an edge, and one with a
/// single coordinate would be dropped by the builder anyway.
pub fn trail_segments(trails: &[crate::trails::TrailFeature]) -> Vec<RoadSegment> {
    trails
        .iter()
        .filter(|t| t.geom_type == 0 && t.coords.len() >= 2)
        .map(|t| RoadSegment {
            osm_id: t.osm_id as u64,
            road_class: t.trail_type.clone(),
            coords: t.coords.clone(),
            name: t.name.clone(),
            ref_tag: None,
            // A trail has no posted limit, no direction of travel for a
            // walker, and no lane count. Left empty rather than invented:
            // the foot profile ignores all three.
            oneway: None,
            speed_limit_kmh: None,
            lanes: None,
            surface: if t.surface.is_empty() { None } else { Some(t.surface.clone()) },
            bridge_tunnel: None,
        })
        .collect()
}

/// Whether `class` is routable under `profile`.
pub fn profile_allows(class: &str, profile: RouteProfile) -> bool {
    match profile {
        RouteProfile::Driving => profile_allows_driving(class),
        RouteProfile::Foot => profile_allows_foot(class),
    }
}

pub fn profile_allows_driving(class: &str) -> bool {
    matches!(
        class,
        "motorway"
            | "motorway_link"
            | "trunk"
            | "trunk_link"
            | "primary"
            | "primary_link"
            | "secondary"
            | "secondary_link"
            | "tertiary"
            | "tertiary_link"
            | "unclassified"
            | "residential"
            | "living_street"
            | "service"
    )
}

/// End-cap = all driving; middle = arterial spine only (no highway sidecar).
/// Driving only -- see the `RouteProfile::Foot` arm in `build_graph`.
pub fn keep_road_class(class: &str, middle: bool) -> bool {
    if !profile_allows_driving(class) {
        return false;
    }
    if !middle {
        return true;
    }
    matches!(
        class,
        "motorway"
            | "motorway_link"
            | "trunk"
            | "trunk_link"
            | "primary"
            | "primary_link"
            | "secondary"
            | "secondary_link"
            | "tertiary"
            | "tertiary_link"
    )
}

fn default_speed_kmh(class: &str) -> f64 {
    match class {
        "motorway" => 90.0,
        "motorway_link" => 45.0,
        "trunk" => 85.0,
        "trunk_link" => 40.0,
        "primary" => 65.0,
        "primary_link" => 30.0,
        "secondary" => 55.0,
        "secondary_link" => 25.0,
        "tertiary" => 40.0,
        "tertiary_link" => 20.0,
        "unclassified" => 20.0,
        "residential" => 20.0,
        "living_street" => 8.0,
        "service" => 10.0,
        _ => 15.0,
    }
}

fn micro_key(lon: f64, lat: f64) -> [i32; 2] {
    [
        math::round(lon * 50_000.0) as i32,
        math::round(lat * 50_000.0) as i32,
    ]
}

struct Graph {
    adj: Vec<Vec<(u32, Weight)>>,
    /// `[lon, lat]` per node (road coord order).
    coords_geo: Vec<[f64; 2]>,
    /// Directed edge geometry after node merge: intermediate+end verts as `[lon,lat]`.
    /// Start is the from-node geo (or previous hop end). Used to draw road centerline.
    edge_geom: BTreeMap<(u32, u32), Vec<[f64; 2]>>,
}

fn build_graph(
    roads: &[RoadSegment],
    zone_middle: &[bool],
    prefs: RoutePrefs,
) -> Result<Graph, RouteFailure> {
    let mut coord_to_node: BTreeMap<[i32; 2], u32> = BTreeMap::new();
    let mut node_micro: Vec<[i32; 2]> = Vec::new();
    let mut node_geo: Vec<[f64; 2]> = Vec::new();
    // pre-merge edges: (from, to, weight) — geom filled after remap from full segs
    let mut edges: Vec<(u32, u32, Weight)> = Vec::new();
    // original segs kept for densify after merge: (node_ids along verts, coords [lon,lat])
    let mut segs_for_geom: Vec<(Vec<u32>, Vec<[f64; 2]>)> = Vec::new();

    let mut lat_scale = 1.0_f64;
    if let Some(s) = roads.iter().find(|s| !s.coords.is_empty()) {
        lat_scale = math::cos(s.coords[0][1].to_radians());
    }

    for (si, seg) in roads.iter().enumerate() {
        let middle = zone_middle.get(si).copied().unwrap_or(false);
        // The middle-of-corridor arterial pruning is a driving optimisation
        // -- it drops residential streets from the long middle of a route so
        // the graph stays small. On foot there is no arterial spine to prune
        // to, and dropping the paths would leave nothing.
        let keep = match prefs.profile {
            RouteProfile::Foot => profile_allows_foot(&seg.road_class),
            RouteProfile::Driving => keep_road_class(&seg.road_class, middle),
        };
        if !keep || seg.coords.len() < 2 {
            continue;
        }
        let mut cm = Vec::with_capacity(seg.coords.len());
        let mut ids = Vec::with_capacity(seg.coords.len());
        for c in &seg.coords {
            let k = micro_key(c[0], c[1]);
            cm.push(k);
            if let Some(&id) = coord_to_node.get(&k) {
                ids.push(id);
            } else {
                let id = node_micro.len() as u32;
                if node_micro.len() >= NODE_CAP {
                    return Err(RouteFailure::NodeBudgetExceeded);
                }
                coord_to_node.insert(k, id);
                node_micro.push(k);
                node_geo.push([c[0], c[1]]);
                ids.push(id);
            }
        }
        segs_for_geom.push((ids.clone(), seg.coords.clone()));
        // On foot the posted limit describes the traffic, not the walker, so
        // it is ignored outright rather than used as a fallback.
        let speed = match prefs.profile {
            RouteProfile::Foot => foot_speed_kmh(&seg.road_class),
            RouteProfile::Driving => seg
                .speed_limit_kmh
                .map(|s| s as f64)
                .unwrap_or_else(|| default_speed_kmh(&seg.road_class)),
        };
        for i in 0..cm.len().saturating_sub(1) {
            let from = ids[i];
            let to = ids[i + 1];
            if from == to {
                continue;
            }
            let (lon1, lat1) = (seg.coords[i][0], seg.coords[i][1]);
            let (lon2, lat2) = (seg.coords[i + 1][0], seg.coords[i + 1][1]);
            let dx = (lon2 - lon1) * lat_scale * 111_320.0;
            let dy = (lat2 - lat1) * 111_320.0;
            let meters = math::sqrt(dx * dx + dy * dy);
            let w = if speed < 0.1 {
                Weight::MAX / 4
            } else {
                let mut secs = meters / ((speed * SPEED_FACTOR) / 3.6);
                if prefs.avoid_highways && is_highway_class(&seg.road_class) {
                    secs *= HIGHWAY_PENALTY;
                }
                weight_from_seconds(secs)
            };
            // A one-way street is one-way for vehicles; a walker may go
            // either way along it, and along every trail.
            let ow = match prefs.profile {
                RouteProfile::Foot => None,
                RouteProfile::Driving => seg.oneway.as_deref(),
            };
            if ow != Some("reverse") {
                edges.push((from, to, w));
            }
            if ow != Some("forward") && ow != Some("yes") {
                edges.push((to, from, w));
            }
        }
    }

    if node_micro.is_empty() {
        return Ok(Graph {
            adj: Vec::new(),
            coords_geo: Vec::new(),
            edge_geom: BTreeMap::new(),
        });
    }

    // ponytail: spatial merge of nearby nodes (daemon grid + union-find lite)
    let n = node_micro.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut grid: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (i, coord) in node_micro.iter().enumerate() {
        let cell = (coord[0] / MERGE_THRESH, coord[1] / MERGE_THRESH);
        grid.entry(cell).or_default().push(i);
    }
    for i in 0..n {
        let coord = node_micro[i];
        let cx = coord[0] / MERGE_THRESH;
        let cy = coord[1] / MERGE_THRESH;
        for dx in -1i32..=1 {
            for dy in -1i32..=1 {
                if let Some(cell_nodes) = grid.get(&(cx + dx, cy + dy)) {
                    for &j in cell_nodes {
                        if j <= i {
                            continue;
                        }
                        let dist = (coord[0] - node_micro[j][0]).abs()
                            + (coord[1] - node_micro[j][1]).abs();
                        if dist <= MERGE_THRESH {
                            let ri = find(&mut parent, i);
                            let rj = find(&mut parent, j);
                            if ri != rj {
                                parent[ri] = rj;
                            }
                        }
                    }
                }
            }
        }
    }
    for i in 0..n {
        let r = find(&mut parent, i);
        parent[i] = r;
    }

    let mut remap = vec![0u32; n];
    let mut new_id = 0u32;
    let mut new_geo: Vec<[f64; 2]> = Vec::new();
    for i in 0..n {
        if parent[i] == i {
            remap[i] = new_id;
            new_geo.push(node_geo[i]);
            new_id += 1;
        }
    }
    for i in 0..n {
        if parent[i] != i {
            remap[i] = remap[parent[i]];
        }
    }

    let mut adj_map: BTreeMap<(u32, u32), Weight> = BTreeMap::new();
    for &(f, t, w) in &edges {
        let rf = remap[f as usize];
        let rt = remap[t as usize];
        if rf == rt {
            continue;
        }
        adj_map
            .entry((rf, rt))
            .and_modify(|e| *e = (*e).min(w))
            .or_insert(w);
    }

    // Junction penalty, charged on arrival at a node.
    //
    // Applied here rather than in build-time edge construction because degree
    // is only knowable after the spatial merge: before it, one intersection is
    // several coincident nodes, each looking like a plain two-way. Counting
    // distinct neighbours (not edge entries) keeps a two-way street at degree
    // 2 whether or not both directions were emitted.
    if prefs.avoid_intersections {
        let mut degree: BTreeMap<u32, alloc::collections::BTreeSet<u32>> = BTreeMap::new();
        for &(f, t) in adj_map.keys() {
            degree.entry(f).or_default().insert(t);
            degree.entry(t).or_default().insert(f);
        }
        for (&(_, to), w) in adj_map.iter_mut() {
            let arms = degree.get(&to).map(|s| s.len()).unwrap_or(0);
            if arms > 2 {
                *w = w.saturating_add(weight_from_seconds(
                    INTERSECTION_PENALTY_S * (arms - 2) as f64,
                ));
            }
        }
    }

    // densify: walk each original polyline through remapped nodes; keep full coords
    // between consecutive distinct remapped nodes so path follows road centerline
    let mut geom_map: BTreeMap<(u32, u32), Vec<[f64; 2]>> = BTreeMap::new();
    for (ids, coords) in &segs_for_geom {
        if ids.is_empty() {
            continue;
        }
        let mut prev_r = remap[ids[0] as usize];
        let mut buf: Vec<[f64; 2]> = alloc::vec![coords[0]];
        for i in 1..ids.len() {
            let r = remap[ids[i] as usize];
            buf.push(coords[i]);
            if r != prev_r {
                let key = (prev_r, r);
                // keep densest geom for this directed hop
                let replace = match geom_map.get(&key) {
                    Some(old) => buf.len() > old.len(),
                    None => true,
                };
                if replace {
                    geom_map.insert(key, buf.clone());
                }
                // reverse for two-way (cheap; adj may not have reverse but ok)
                let mut rev = buf.clone();
                rev.reverse();
                let rkey = (r, prev_r);
                let replace_r = match geom_map.get(&rkey) {
                    Some(old) => rev.len() > old.len(),
                    None => true,
                };
                if replace_r {
                    geom_map.insert(rkey, rev);
                }
                prev_r = r;
                buf = alloc::vec![coords[i]];
            }
        }
    }

    let final_n = new_id as usize;
    if final_n > NODE_CAP {
        return Err(RouteFailure::NodeBudgetExceeded);
    }
    let mut adj = vec![Vec::new(); final_n];
    for ((f, t), w) in adj_map {
        adj[f as usize].push((t, w));
    }
    Ok(Graph {
        adj,
        coords_geo: new_geo,
        edge_geom: geom_map,
    })
}

fn nearest_node(g: &Graph, lat: f64, lon: f64, snap_m: f64) -> Option<usize> {
    let mut best = None;
    let mut best_d = snap_m;
    for (i, c) in g.coords_geo.iter().enumerate() {
        // c is [lon, lat]
        let d = haversine_distance_m(lat, lon, c[1], c[0]);
        if d <= best_d {
            best_d = d;
            best = Some(i);
        }
    }
    best
}

fn haversine_h(dst_lat: f64, dst_lon: f64, coords: &[[f64; 2]], node: usize) -> Weight {
    let c = coords[node];
    let meters = haversine_distance_m(dst_lat, dst_lon, c[1], c[0]);
    // 130 km/h = 36.11 m/s → centiseconds
    (meters / 36.11 * 100.0) as Weight
}

fn astar(
    adj: &[Vec<(u32, Weight)>],
    coords: &[[f64; 2]],
    src: usize,
    dst: usize,
    dst_lat: f64,
    dst_lon: f64,
) -> Option<(Vec<usize>, Weight)> {
    let n = adj.len();
    if n == 0 || src >= n || dst >= n {
        return None;
    }
    if src == dst {
        return Some((alloc::vec![src], 0));
    }
    let mut dist = vec![Weight::MAX; n];
    let mut pred = vec![usize::MAX; n];
    let mut heap = BinaryHeap::new();
    dist[src] = 0;
    let h0 = haversine_h(dst_lat, dst_lon, coords, src);
    heap.push(Reverse((h0, src)));
    while let Some(Reverse((_, u))) = heap.pop() {
        if u == dst {
            break;
        }
        let du = dist[u];
        if du == Weight::MAX {
            continue;
        }
        for &(v, w) in &adj[u] {
            let nd = du.saturating_add(w);
            let vi = v as usize;
            if nd < dist[vi] {
                dist[vi] = nd;
                pred[vi] = u;
                let hv = haversine_h(dst_lat, dst_lon, coords, vi);
                heap.push(Reverse((nd.saturating_add(hv), vi)));
            }
        }
    }
    if dist[dst] == Weight::MAX {
        return None;
    }
    let mut path = Vec::new();
    let mut cur = dst;
    path.push(cur);
    while cur != src {
        cur = pred[cur];
        if cur == usize::MAX {
            return None;
        }
        path.push(cur);
    }
    path.reverse();
    Some((path, dist[dst]))
}

/// Bidirectional A* (ponytail: only when large).
fn bi_astar(
    adj: &[Vec<(u32, Weight)>],
    coords: &[[f64; 2]],
    src: usize,
    dst: usize,
    src_lat: f64,
    src_lon: f64,
    dst_lat: f64,
    dst_lon: f64,
) -> Option<(Vec<usize>, Weight)> {
    let n = adj.len();
    if n == 0 || src >= n || dst >= n {
        return None;
    }
    // reverse adj
    let mut radj = vec![Vec::new(); n];
    for (u, outs) in adj.iter().enumerate() {
        for &(v, w) in outs {
            radj[v as usize].push((u as u32, w));
        }
    }
    let mut df = vec![Weight::MAX; n];
    let mut db = vec![Weight::MAX; n];
    let mut pf = vec![usize::MAX; n];
    let mut pb = vec![usize::MAX; n];
    let mut hf = BinaryHeap::new();
    let mut hb = BinaryHeap::new();
    df[src] = 0;
    db[dst] = 0;
    hf.push(Reverse((haversine_h(dst_lat, dst_lon, coords, src), src)));
    hb.push(Reverse((haversine_h(src_lat, src_lon, coords, dst), dst)));
    let mut best = Weight::MAX;
    let mut meet = usize::MAX;
    let mut expand_f = true;
    for _ in 0..(n * 4) {
        if hf.is_empty() && hb.is_empty() {
            break;
        }
        if expand_f && !hf.is_empty() {
            if let Some(Reverse((_, u))) = hf.pop() {
                if df[u].saturating_add(db[u]) < best && db[u] != Weight::MAX {
                    best = df[u].saturating_add(db[u]);
                    meet = u;
                }
                if df[u] != Weight::MAX {
                    for &(v, w) in &adj[u] {
                        let nd = df[u].saturating_add(w);
                        let vi = v as usize;
                        if nd < df[vi] {
                            df[vi] = nd;
                            pf[vi] = u;
                            let hv = haversine_h(dst_lat, dst_lon, coords, vi);
                            hf.push(Reverse((nd.saturating_add(hv), vi)));
                        }
                    }
                }
            }
        } else if !hb.is_empty() {
            if let Some(Reverse((_, u))) = hb.pop() {
                if db[u].saturating_add(df[u]) < best && df[u] != Weight::MAX {
                    best = db[u].saturating_add(df[u]);
                    meet = u;
                }
                if db[u] != Weight::MAX {
                    for &(v, w) in &radj[u] {
                        let nd = db[u].saturating_add(w);
                        let vi = v as usize;
                        if nd < db[vi] {
                            db[vi] = nd;
                            pb[vi] = u;
                            let hv = haversine_h(src_lat, src_lon, coords, vi);
                            hb.push(Reverse((nd.saturating_add(hv), vi)));
                        }
                    }
                }
            }
        }
        expand_f = !expand_f;
        if best < Weight::MAX
            && hf.peek().map(|Reverse((f, _))| *f).unwrap_or(Weight::MAX)
                + hb.peek().map(|Reverse((f, _))| *f).unwrap_or(Weight::MAX)
                >= best
        {
            // weak stop; still may not be optimal — fall back ok
            break;
        }
    }
    if meet == usize::MAX || best == Weight::MAX {
        // fall back to uni
        return astar(adj, coords, src, dst, dst_lat, dst_lon);
    }
    let mut path_f = Vec::new();
    let mut cur = meet;
    path_f.push(cur);
    while cur != src {
        cur = pf[cur];
        if cur == usize::MAX {
            return astar(adj, coords, src, dst, dst_lat, dst_lon);
        }
        path_f.push(cur);
    }
    path_f.reverse();
    let mut path_b = Vec::new();
    cur = meet;
    while cur != dst {
        cur = pb[cur];
        if cur == usize::MAX {
            return astar(adj, coords, src, dst, dst_lat, dst_lon);
        }
        path_b.push(cur);
    }
    path_f.extend(path_b);
    Some((path_f, best))
}

/// Route on pre-decoded segments. `zone_middle` empty ⇒ all end-cap (full driving).
///
/// Kept at the original signature so existing callers are unaffected; see
/// [`route_roads_with`] for the preference-aware form.
pub fn route_roads(
    roads: &[RoadSegment],
    zone_middle: &[bool],
    lat1: f64,
    lon1: f64,
    lat2: f64,
    lon2: f64,
    snap_m: f64,
) -> Option<RouteResult> {
    route_roads_with(
        roads,
        zone_middle,
        lat1,
        lon1,
        lat2,
        lon2,
        snap_m,
        RoutePrefs::default(),
    )
}

/// [`route_roads`] with preferences applied.
#[allow(clippy::too_many_arguments)]
pub fn route_roads_with(
    roads: &[RoadSegment],
    zone_middle: &[bool],
    lat1: f64,
    lon1: f64,
    lat2: f64,
    lon2: f64,
    snap_m: f64,
    prefs: RoutePrefs,
) -> Option<RouteResult> {
    route_roads_diagnostic(roads, zone_middle, lat1, lon1, lat2, lon2, snap_m, prefs).ok()
}

/// [`route_roads_with`] with a structured failure instead of an ambiguous
/// `None`. Existing callers keep the nullable API; corridor loaders can use
/// this form to decide whether widening can actually help.
#[allow(clippy::too_many_arguments)]
pub fn route_roads_diagnostic(
    roads: &[RoadSegment],
    zone_middle: &[bool],
    lat1: f64,
    lon1: f64,
    lat2: f64,
    lon2: f64,
    snap_m: f64,
    prefs: RoutePrefs,
) -> Result<RouteResult, RouteFailure> {
    let g = build_graph(roads, zone_middle, prefs)?;
    if g.adj.is_empty() {
        return Err(RouteFailure::EmptyGraph);
    }
    let src = nearest_node(&g, lat1, lon1, snap_m).ok_or(RouteFailure::StartNotSnapped)?;
    let dst = nearest_node(&g, lat2, lon2, snap_m).ok_or(RouteFailure::EndNotSnapped)?;
    let (nodes, w) = if g.adj.len() > BI_ASTAR_MIN_NODES {
        bi_astar(&g.adj, &g.coords_geo, src, dst, lat1, lon1, lat2, lon2)
    } else {
        astar(&g.adj, &g.coords_geo, src, dst, lat2, lon2)
    }
    .ok_or(RouteFailure::Disconnected)?;
    // stitch edge geometries so path follows road centerline, not node chords
    let mut path: Vec<[f64; 2]> = Vec::new();
    let mut dist_m = 0.0_f64;
    if let Some(&n0) = nodes.first() {
        let c = g.coords_geo[n0];
        path.push([c[1], c[0]]); // lat, lon
    }
    for wpair in nodes.windows(2) {
        let a = wpair[0] as u32;
        let b = wpair[1] as u32;
        if let Some(geom) = g.edge_geom.get(&(a, b)) {
            // geom is [lon,lat]... including both ends; skip first (already in path)
            for pt in geom.iter().skip(1) {
                let prev = *path.last().unwrap();
                dist_m += haversine_distance_m(prev[0], prev[1], pt[1], pt[0]);
                path.push([pt[1], pt[0]]);
            }
        } else {
            // fallback chord
            let c = g.coords_geo[wpair[1]];
            let prev = *path.last().unwrap();
            dist_m += haversine_distance_m(prev[0], prev[1], c[1], c[0]);
            path.push([c[1], c[0]]);
        }
    }
    Ok(RouteResult {
        distance_m: dist_m,
        duration_s: weight_to_seconds(w),
        path,
    })
}

// --- Corridor policy ---------------------------------------------------------

/// Metres in a degree of latitude. A degree of longitude is this shortened by
/// the cosine of the latitude.
const M_PER_DEG_LAT: f64 = 111_320.0;

/// How wide a corridor to cut around a pair of endpoints, and what to do when
/// the graph inside it comes back disconnected.
///
/// Every number here was measured against one road network at one latitude,
/// so they are defaults rather than constants: a boat, a courier working
/// alleys, or a country with a sparser digitised network can say otherwise.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CorridorPrefs {
    /// Slack beyond each endpoint, in metres, so a route may leave the direct
    /// line to reach the road that actually joins the two ends.
    pub end_cap_m: f64,
    /// Margin as a fraction of the endpoint separation. The wider of this and
    /// [`Self::end_cap_m`] wins, so a long route gets a proportionally wider
    /// corridor.
    pub span_fraction: f64,
    /// Corridor widenings to try on a disconnected graph, widest first.
    ///
    /// One fixed scale does not work: the cell budget follows the box *area*,
    /// so 2.5x fits a 100 km north-south corridor (406 cells) and is rejected
    /// outright for a 50 km diagonal (376 cells at 1x). A rejected widening is
    /// a silent no-op, so the retry walks down until one fits.
    pub retry_scales: Vec<f64>,
    /// How far an endpoint may sit from the nearest routable way. `<= 0` uses
    /// [`default_snap_radius_m`] for the profile.
    pub snap_radius_m: f64,
    /// Cell ceiling for the corridor fetch.
    ///
    /// Routing gets its own, larger than the viewport cap: a corridor is long
    /// and thin, and a 200 km trip needs about 1,600 res-7 cells however
    /// narrowly it is cut. Splitting the trip instead put the seam in a field.
    pub max_cells: usize,
    /// Ceiling on the proportional margin, in metres each side.
    ///
    /// [`Self::span_fraction`] alone makes the corridor's *area* grow with the
    /// square of the trip, so a long route is refused before it is attempted.
    /// This bounds the width so the box stays inside the cell cap and the
    /// route is tried rather than split.
    pub max_margin_m: f64,
}

impl Default for CorridorPrefs {
    fn default() -> Self {
        Self {
            end_cap_m: 1_670.0,
            span_fraction: 0.15,
            retry_scales: vec![2.5, 2.0, 1.6, 1.3],
            snap_radius_m: 0.0,
            // 9 km each side: wide enough for a highway to bow away from the
            // straight line between two towns, narrow enough that a 250 km
            // corridor still fits the 512-cell cap.
            max_margin_m: 9_000.0,
            // Room for a ~250 km leg. Beyond that the corridor is better
            // served by a coarser road band than by more res-7 cells.
            max_cells: 2_048,
        }
    }
}

/// Snap radius for a profile when the caller does not name one. A car sits
/// further from the centreline of the road it is on than a walker does from
/// the path.
pub fn default_snap_radius_m(profile: RouteProfile) -> f64 {
    match profile {
        RouteProfile::Driving => 250.0,
        RouteProfile::Foot => 120.0,
    }
}

/// Corridor half-margins in degrees, `(lat, lon)`.
///
/// The longitude margin is divided by the cosine of the midpoint latitude so
/// the corridor is the same number of metres wide wherever it is cut. A fixed
/// degree margin loses ~45% of its width between 35°N and 60°N.
pub fn corridor_margins_deg(
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    prefs: &CorridorPrefs,
) -> (f64, f64) {
    let mid_lat = (start_lat + end_lat) / 2.0;
    // Clamped so a polar request widens to a bounded box rather than dividing
    // by zero; `cells_for_bounds` then refuses it honestly.
    let shrink = math::cos(mid_lat * (core::f64::consts::PI / 180.0))
        .abs()
        .max(0.05);
    let cap_deg = prefs.end_cap_m / M_PER_DEG_LAT;
    // The proportional margin needs a ceiling or a long route can never fit.
    // At 15% of the separation, a 200 km trip asks for 30 km of slack on each
    // side: a 200x60 km box, 12,000 km², against a cap of 512 res-7 cells
    // (~2,600 km²). It was refused outright, and the client answered by
    // halving the leg and routing each half -- which is how "bad bounding box"
    // turned into "disconnected", since a geometric midpoint lands in a field
    // and snaps to whatever lane is nearest.
    //
    // A road does not wander that far from the line between its ends. Beyond
    // [`CorridorPrefs::max_margin_m`] the extra width buys nothing and costs
    // the whole request.
    let ceiling_lat = prefs.max_margin_m / M_PER_DEG_LAT;
    (
        cap_deg
            .max((start_lat - end_lat).abs() * prefs.span_fraction)
            .min(ceiling_lat.max(cap_deg)),
        (cap_deg / shrink)
            .max((start_lon - end_lon).abs() * prefs.span_fraction)
            .min((ceiling_lat / shrink).max(cap_deg / shrink)),
    )
}

fn cells_at_scale(
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    lat_margin: f64,
    lon_margin: f64,
    scale: f64,
    max_cells: usize,
) -> Result<Vec<u64>, crate::query::BoundsError> {
    crate::query::cells_for_bounds_capped(
        start_lat.min(end_lat) - lat_margin * scale,
        start_lon.min(end_lon) - lon_margin * scale,
        start_lat.max(end_lat) + lat_margin * scale,
        start_lon.max(end_lon) + lon_margin * scale,
        max_cells,
    )
}

/// Cells covering the corridor between two endpoints.
pub fn corridor_cells(
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    prefs: &CorridorPrefs,
) -> Result<Vec<u64>, crate::query::BoundsError> {
    let (lat_margin, lon_margin) =
        corridor_margins_deg(start_lat, start_lon, end_lat, end_lon, prefs);
    cells_at_scale(
        start_lat, start_lon, end_lat, end_lon, lat_margin, lon_margin, 1.0, prefs.max_cells,
    )
}

/// The widest corridor that still fits the cell cap and holds more cells than
/// `base_cells`. `None` when nothing fits, which means a retry would only
/// repeat the same work.
pub fn widened_corridor_cells(
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    base_cells: usize,
    prefs: &CorridorPrefs,
) -> Option<Vec<u64>> {
    let (lat_margin, lon_margin) =
        corridor_margins_deg(start_lat, start_lon, end_lat, end_lon, prefs);
    prefs.retry_scales.iter().find_map(|scale| {
        cells_at_scale(
            start_lat, start_lon, end_lat, end_lon, lat_margin, lon_margin, *scale,
            prefs.max_cells,
        )
        .ok()
        .filter(|cells| cells.len() > base_cells)
    })
}

/// A corridor route plus how many segments had to be decoded to find it.
#[derive(Clone, Debug, PartialEq)]
pub struct CorridorRoute {
    pub route: RouteResult,
    pub decoded_segments: usize,
}

/// Why [`route_in_corridor`] produced no route: the corridor could not be
/// built, the caller's fetch failed, or the search itself failed.
#[derive(Clone, Debug, PartialEq)]
pub enum CorridorError<E> {
    Bounds(crate::query::BoundsError),
    Fetch(E),
    Route(RouteFailure),
}

/// Route between two points over whatever segments `fetch` returns for a
/// corridor of cells, retrying on a wider corridor when the graph is
/// disconnected.
///
/// `fetch` decides which layers contribute — roads alone, or roads plus
/// trails — and is the only part of this a binding has to supply.
///
/// A disconnected corridor means both ends snapped but the road joining them
/// arcs outside the box: a river crossing or an interchange just past the
/// edge. Widening pulls it in, but only while the cell cap has room left. A
/// request whose corridor cannot fit [`crate::query::MAX_BOUNDS_CELLS`] is
/// refused outright; split it into legs and route each.
/// Which segments lie in the long middle of a corridor rather than near an
/// end, so [`keep_road_class`] can drop residential streets there.
///
/// The flags existed and every caller passed an empty slice, which meant a
/// 120 km corridor built a graph containing every driveway along the way and
/// blew the 250,000-node budget. Nobody leaves Jackson by turning onto a
/// cul-de-sac 60 km down the road: away from the endpoints only the arterial
/// spine can be part of the answer.
///
/// "Near an end" is [`CorridorPrefs::end_cap_m`] times [`END_ZONE_SCALE`],
/// generous enough to include the whole town at either end.
fn middle_zones<'a>(
    segments: &'a [RoadSegment],
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    corridor: &CorridorPrefs,
) -> Vec<bool> {
    let reach = corridor.end_cap_m * END_ZONE_SCALE;
    segments
        .iter()
        .map(|seg| {
            let Some(first) = seg.coords.first() else { return false };
            let (lon, lat) = (first[0], first[1]);
            haversine_distance_m(lat, lon, start_lat, start_lon) > reach
                && haversine_distance_m(lat, lon, end_lat, end_lon) > reach
        })
        .collect()
}

/// How far past [`CorridorPrefs::end_cap_m`] the full street network is kept.
/// 1.67 km x 6 is about 10 km: a town's worth of streets at either end.
const END_ZONE_SCALE: f64 = 6.0;

pub fn route_in_corridor<E>(
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    prefs: RoutePrefs,
    corridor: &CorridorPrefs,
    mut fetch: impl FnMut(&[u64]) -> Result<Vec<RoadSegment>, E>,
) -> Result<CorridorRoute, CorridorError<E>> {
    let cells = corridor_cells(start_lat, start_lon, end_lat, end_lon, corridor)
        .map_err(CorridorError::Bounds)?;
    let segments = fetch(&cells).map_err(CorridorError::Fetch)?;
    let snap_m = if corridor.snap_radius_m > 0.0 {
        corridor.snap_radius_m
    } else {
        default_snap_radius_m(prefs.profile)
    };
    let mut decoded_segments = segments.len();
    let mut zones = middle_zones(&segments, start_lat, start_lon, end_lat, end_lon, corridor);
    let mut attempt = route_roads_diagnostic(
        &segments, &zones, start_lat, start_lon, end_lat, end_lon, snap_m, prefs,
    );

    if attempt == Err(RouteFailure::Disconnected) {
        if let Some(wider) = widened_corridor_cells(
            start_lat,
            start_lon,
            end_lat,
            end_lon,
            cells.len(),
            corridor,
        ) {
            let widened = fetch(&wider).map_err(CorridorError::Fetch)?;
            if widened.len() > segments.len() {
                decoded_segments = widened.len();
                zones = middle_zones(&widened, start_lat, start_lon, end_lat, end_lon, corridor);
                attempt = route_roads_diagnostic(
                    &widened, &zones, start_lat, start_lon, end_lat, end_lon, snap_m, prefs,
                );
            }
        }
    }
    Ok(CorridorRoute {
        route: attempt.map_err(CorridorError::Route)?,
        decoded_segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;

    fn base_cell_count(start: (f64, f64), end: (f64, f64)) -> usize {
        corridor_cells(start.0, start.1, end.0, end.1, &CorridorPrefs::default())
            .expect("the base corridor must fit")
            .len()
    }

    #[test]
    fn a_short_route_widens_by_the_full_step() {
        let start = (35.0, -88.0);
        let end = (35.45, -88.0);
        let base = base_cell_count(start, end);

        let wider = widened_corridor_cells(
            start.0,
            start.1,
            end.0,
            end.1,
            base,
            &CorridorPrefs::default(),
        )
        .expect("a short route has room to widen");

        assert!(wider.len() > base);
    }

    #[test]
    fn a_diagonal_route_widens_after_the_widest_step_is_rejected() {
        // A diagonal long enough that the widest step blows the cell cap: a
        // single fixed scale gave up here and left the route disconnected.
        // The distance is chosen against `CorridorPrefs::max_cells`, so it
        // grew when routing stopped borrowing the viewport's 512-cell ceiling.
        let start = (35.0, -88.0);
        let end = (35.6, -87.4);
        let prefs = CorridorPrefs::default();
        let base = base_cell_count(start, end);
        let (lat_margin, lon_margin) =
            corridor_margins_deg(start.0, start.1, end.0, end.1, &prefs);

        assert!(
            cells_at_scale(start.0, start.1, end.0, end.1, lat_margin, lon_margin, 2.5, CorridorPrefs::default().max_cells).is_err(),
            "this case exists because the widest step is rejected",
        );

        let wider = widened_corridor_cells(start.0, start.1, end.0, end.1, base, &prefs)
            .expect("a smaller widening must still be found");
        assert!(wider.len() > base);
        assert!(wider.len() <= prefs.max_cells);
    }

    #[test]
    fn a_route_with_no_room_left_reports_no_widening() {
        let start = (35.0, -88.0);
        let end = (36.4, -86.6);

        assert!(
            widened_corridor_cells(start.0, start.1, end.0, end.1, 1, &CorridorPrefs::default())
                .is_none()
        );
    }

    #[test]
    fn the_longitude_margin_holds_its_width_in_metres_going_north() {
        let prefs = CorridorPrefs::default();
        let width_m = |lat: f64| {
            let (_, lon_margin) = corridor_margins_deg(lat, -88.0, lat, -88.0, &prefs);
            lon_margin * M_PER_DEG_LAT * math::cos(lat * (core::f64::consts::PI / 180.0))
        };
        // A fixed degree margin loses ~45% of its metres between these two.
        assert!((width_m(35.0) - prefs.end_cap_m).abs() < 1.0);
        assert!((width_m(60.0) - prefs.end_cap_m).abs() < 1.0);
    }

    #[test]
    fn a_polar_request_does_not_blow_up_the_margin() {
        let (_, lon_margin) = corridor_margins_deg(89.9, 0.0, 89.9, 0.0, &CorridorPrefs::default());
        assert!(lon_margin.is_finite() && lon_margin < 1.0);
    }

    #[test]
    fn the_snap_radius_falls_back_per_profile() {
        assert_eq!(default_snap_radius_m(RouteProfile::Driving), 250.0);
        assert_eq!(default_snap_radius_m(RouteProfile::Foot), 120.0);
    }

    fn seg(class: &str, coords: Vec<[f64; 2]>, oneway: Option<&str>) -> RoadSegment {
        RoadSegment {
            osm_id: 1,
            road_class: String::from(class),
            coords,
            name: None,
            ref_tag: None,
            oneway: oneway.map(String::from),
            speed_limit_kmh: Some(50),
            lanes: None,
            surface: None,
            bridge_tunnel: None,
        }
    }

    fn foot() -> RoutePrefs {
        RoutePrefs { profile: RouteProfile::Foot, ..Default::default() }
    }

    // A path running east, and a motorway alongside it 100 m north. Both
    // reach the same longitudes, so whichever the profile allows is the one
    // that can carry a route.
    fn path_and_motorway() -> Vec<RoadSegment> {
        vec![
            seg("path", vec![[-86.80, 36.0], [-86.79, 36.0]], None),
            seg("motorway", vec![[-86.80, 36.0009], [-86.79, 36.0009]], None),
        ]
    }

    #[test]
    fn a_walker_routes_along_a_path_a_driver_cannot_use() {
        let roads = path_and_motorway();
        let zm = vec![false; roads.len()];
        let on_foot = route_roads_with(&roads, &zm, 36.0, -86.7995, 36.0, -86.7905, 60.0, foot())
            .expect("the path carries a walk");
        assert!(on_foot.distance_m > 700.0, "distance {}", on_foot.distance_m);
        // ~800 m at 5 km/h, before the 0.85 factor: minutes, not seconds.
        assert!(on_foot.duration_s > 400.0, "duration {}", on_foot.duration_s);

        // Driving cannot snap to the path at all -- only the motorway 100 m
        // north is routable, and that is past the snap radius.
        assert!(
            route_roads_with(&roads, &zm, 36.0, -86.7995, 36.0, -86.7905, 60.0, RoutePrefs::default())
                .is_none(),
            "a car has no business on a footpath"
        );
    }

    #[test]
    fn a_walker_is_kept_off_the_motorway() {
        let roads = vec![seg("motorway", vec![[-86.80, 36.0], [-86.79, 36.0]], None)];
        let zm = vec![false];
        assert!(
            route_roads_with(&roads, &zm, 36.0, -86.7995, 36.0, -86.7905, 60.0, foot()).is_none(),
            "motorway is the one class walking is actually prohibited on"
        );
    }

    #[test]
    fn oneway_and_speed_limits_are_driving_concerns_only() {
        // A one-way path, tagged 50 km/h, walked against its direction.
        let roads = vec![seg("footway", vec![[-86.80, 36.0], [-86.79, 36.0]], Some("yes"))];
        let zm = vec![false];
        let back = route_roads_with(&roads, &zm, 36.0, -86.7905, 36.0, -86.7995, 60.0, foot())
            .expect("a walker may go either way");
        // If the 50 km/h tag had been used, this would take about a minute.
        assert!(back.duration_s > 400.0, "walked at driving speed: {}", back.duration_s);
    }

    #[test]
    fn trails_convert_to_segments_and_trailheads_are_dropped() {
        use crate::trails::TrailFeature;
        let trails = vec![
            TrailFeature {
                osm_id: 7,
                trail_type: String::from("path"),
                geom_type: 0,
                coords: vec![[-86.80, 36.0], [-86.79, 36.0]],
                surface: String::from("compacted"),
                sac_scale: String::from("hiking"),
                name: Some(String::from("Greenway")),
            },
            TrailFeature {
                osm_id: 8,
                trail_type: String::from("trailhead"),
                geom_type: 1,
                coords: vec![[-86.795, 36.0]],
                surface: String::new(),
                sac_scale: String::new(),
                name: Some(String::from("North Gate")),
            },
        ];
        let segs = trail_segments(&trails);
        assert_eq!(segs.len(), 1, "a point is not an edge");
        assert_eq!(segs[0].road_class, "path", "trail type is what the profile reads");
        assert_eq!(segs[0].name.as_deref(), Some("Greenway"));
        assert_eq!(segs[0].surface.as_deref(), Some("compacted"));
        assert!(segs[0].speed_limit_kmh.is_none(), "a trail has no posted limit");

        let zm = vec![false; segs.len()];
        assert!(
            route_roads_with(&segs, &zm, 36.0, -86.7995, 36.0, -86.7905, 60.0, foot()).is_some(),
            "converted trails route on foot"
        );
    }

    // --- RoutePrefs ---------------------------------------------------
    //
    // Layout for both: A --- B, reachable two ways.
    //   motorway  A -> M -> B   (fast, direct)
    //   residential A -> R -> B (slower, longer)
    // With no preference the motorway wins; avoid_highways must flip it.
    fn two_route_choices() -> alloc::vec::Vec<RoadSegment> {
        vec![
            // Motorway: straight east along 36.0.
            seg("motorway", vec![[-86.00, 36.0], [-85.98, 36.0]], None),
            seg("motorway", vec![[-85.98, 36.0], [-85.96, 36.0]], None),
            // Residential: a detour north and back, same endpoints.
            seg("residential", vec![[-86.00, 36.0], [-85.98, 36.004]], None),
            seg("residential", vec![[-85.98, 36.004], [-85.96, 36.0]], None),
        ]
    }

    fn max_detour_lat(r: &RouteResult) -> f64 {
        r.path.iter().fold(f64::MIN, |m, p| if p[0] > m { p[0] } else { m })
    }

    #[test]
    fn default_prefs_take_the_motorway() {
        let roads = two_route_choices();
        let r = route_roads(&roads, &[], 36.0, -86.0, 36.0, -85.96, 500.0).unwrap();
        // Straight down the motorway: never leaves the 36.0 line.
        assert!(max_detour_lat(&r) < 36.001, "expected the direct motorway, got {:?}", r.path);
    }

    #[test]
    fn avoid_highways_takes_the_slower_surface_street() {
        let roads = two_route_choices();
        let prefs = RoutePrefs { avoid_highways: true, ..Default::default() };
        let r = route_roads_with(&roads, &[], 36.0, -86.0, 36.0, -85.96, 500.0, prefs).unwrap();
        assert!(
            max_detour_lat(&r) > 36.003,
            "avoid_highways should route via the residential detour, got {:?}",
            r.path
        );
    }

    #[test]
    fn avoid_highways_still_routes_when_the_highway_is_the_only_link() {
        // Penalty, not prohibition: with no alternative a route must still exist.
        let roads = vec![seg("motorway", vec![[-86.0, 36.0], [-85.98, 36.0]], None)];
        let prefs = RoutePrefs { avoid_highways: true, ..Default::default() };
        let r = route_roads_with(&roads, &[], 36.0, -86.0, 36.0, -85.98, 500.0, prefs);
        assert!(r.is_some(), "a penalty must not make the only road unusable");
    }

    #[test]
    fn avoid_intersections_costs_more_through_a_junction() {
        // A cross: the east-west road is crossed by a north-south one at the
        // midpoint, making that node a 4-arm junction.
        let roads = vec![
            seg("residential", vec![[-86.00, 36.0], [-85.99, 36.0]], None),
            seg("residential", vec![[-85.99, 36.0], [-85.98, 36.0]], None),
            seg("residential", vec![[-85.99, 35.99], [-85.99, 36.0]], None),
            seg("residential", vec![[-85.99, 36.0], [-85.99, 36.01]], None),
        ];
        let plain = route_roads(&roads, &[], 36.0, -86.0, 36.0, -85.98, 500.0).unwrap();
        let avoid = route_roads_with(
            &roads, &[], 36.0, -86.0, 36.0, -85.98, 500.0,
            RoutePrefs { avoid_intersections: true, ..Default::default() },
        )
        .unwrap();
        // Same geometry either way -- there is no detour available -- but the
        // junction now carries a time cost, which is what steers a real route.
        assert!(
            avoid.duration_s > plain.duration_s,
            "junction should cost time: {} vs {}",
            avoid.duration_s,
            plain.duration_s
        );
    }

    #[test]
    fn prefs_default_to_no_penalty() {
        let p = RoutePrefs::default();
        assert!(!p.avoid_highways && !p.avoid_intersections);
        let roads = two_route_choices();
        let a = route_roads(&roads, &[], 36.0, -86.0, 36.0, -85.96, 500.0).unwrap();
        let b = route_roads_with(&roads, &[], 36.0, -86.0, 36.0, -85.96, 500.0, RoutePrefs::default()).unwrap();
        assert_eq!(a.path, b.path, "default prefs must match the old entry point");
        assert_eq!(a.duration_s, b.duration_s);
    }

    #[test]
    fn two_segments_share_endpoint() {
        // ~111m north then east of (36.0, -86.0)
        let a = seg("residential", vec![[-86.0, 36.0], [-86.0, 36.001]], None);
        let b = seg(
            "residential",
            vec![[-86.0, 36.001], [-85.999, 36.001]],
            None,
        );
        let r = route_roads(&[a, b], &[], 36.0, -86.0, 36.001, -85.999, 50.0).expect("route");
        assert!(r.path.len() >= 2);
        // path should be longer than pure crow between ends of first leg only
        assert!(
            r.distance_m > 150.0 && r.distance_m < 400.0,
            "d={}",
            r.distance_m
        );
    }

    #[test]
    fn middle_drops_residential_but_ends_connect_via_arterial() {
        // A residential spur → primary spine → B residential spur
        let end_a = seg("residential", vec![[-86.0, 36.0], [-86.0, 36.001]], None);
        let mid = seg("primary", vec![[-86.0, 36.001], [-85.998, 36.001]], None);
        let end_b = seg(
            "residential",
            vec![[-85.998, 36.001], [-85.998, 36.002]],
            None,
        );
        // zone: end, middle, end
        let r = route_roads(
            &[end_a, mid, end_b],
            &[false, true, false],
            36.0,
            -86.0,
            36.002,
            -85.998,
            80.0,
        )
        .expect("route through arterial middle");
        assert!(r.path.len() >= 3);
    }

    #[test]
    fn oneway_blocks_reverse() {
        let only = seg(
            "primary",
            vec![[-86.0, 36.0], [-85.999, 36.0]],
            Some("forward"),
        );
        let ok = route_roads(&[only.clone()], &[], 36.0, -86.0, 36.0, -85.999, 50.0);
        assert!(ok.is_some());
        let bad = route_roads(&[only], &[], 36.0, -85.999, 36.0, -86.0, 50.0);
        assert!(bad.is_none());
    }

    #[test]
    fn empty_roads_none() {
        assert!(route_roads(&[], &[], 36.0, -86.0, 36.1, -86.1, 50.0).is_none());
    }

    #[test]
    fn diagnostic_distinguishes_empty_snap_and_disconnected_graphs() {
        let prefs = RoutePrefs::default();
        assert_eq!(
            route_roads_diagnostic(&[], &[], 36.0, -86.0, 36.1, -86.1, 50.0, prefs),
            Err(RouteFailure::EmptyGraph)
        );

        let near_a = seg("residential", vec![[-86.0, 36.0], [-85.999, 36.0]], None);
        assert_eq!(
            route_roads_diagnostic(
                &[near_a.clone()], &[], 37.0, -87.0, 36.0, -85.999, 50.0, prefs,
            ),
            Err(RouteFailure::StartNotSnapped)
        );
        assert_eq!(
            route_roads_diagnostic(
                &[near_a.clone()], &[], 36.0, -86.0, 37.0, -87.0, 50.0, prefs,
            ),
            Err(RouteFailure::EndNotSnapped)
        );

        let island = seg("residential", vec![[-85.9, 36.0], [-85.899, 36.0]], None);
        assert_eq!(
            route_roads_diagnostic(
                &[near_a, island], &[], 36.0, -86.0, 36.0, -85.899, 50.0, prefs,
            ),
            Err(RouteFailure::Disconnected)
        );
    }
}
