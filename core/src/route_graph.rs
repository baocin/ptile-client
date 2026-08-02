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
const NODE_CAP: usize = 250_000;
const BI_ASTAR_MIN_NODES: usize = 50_000;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RouteResult {
    pub distance_m: f64,
    pub duration_s: f64,
    /// Leaflet order: `[lat, lon]`.
    pub path: Vec<[f64; 2]>,
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

fn build_graph(roads: &[RoadSegment], zone_middle: &[bool]) -> Option<Graph> {
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
        if !keep_road_class(&seg.road_class, middle) || seg.coords.len() < 2 {
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
                    return None;
                }
                coord_to_node.insert(k, id);
                node_micro.push(k);
                node_geo.push([c[0], c[1]]);
                ids.push(id);
            }
        }
        segs_for_geom.push((ids.clone(), seg.coords.clone()));
        let speed = seg
            .speed_limit_kmh
            .map(|s| s as f64)
            .unwrap_or_else(|| default_speed_kmh(&seg.road_class));
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
                weight_from_seconds(meters / ((speed * SPEED_FACTOR) / 3.6))
            };
            let ow = seg.oneway.as_deref();
            if ow != Some("reverse") {
                edges.push((from, to, w));
            }
            if ow != Some("forward") && ow != Some("yes") {
                edges.push((to, from, w));
            }
        }
    }

    if node_micro.is_empty() {
        return Some(Graph {
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
        return None;
    }
    let mut adj = vec![Vec::new(); final_n];
    for ((f, t), w) in adj_map {
        adj[f as usize].push((t, w));
    }
    Some(Graph {
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
pub fn route_roads(
    roads: &[RoadSegment],
    zone_middle: &[bool],
    lat1: f64,
    lon1: f64,
    lat2: f64,
    lon2: f64,
    snap_m: f64,
) -> Option<RouteResult> {
    let g = build_graph(roads, zone_middle)?;
    if g.adj.is_empty() {
        return None;
    }
    let src = nearest_node(&g, lat1, lon1, snap_m)?;
    let dst = nearest_node(&g, lat2, lon2, snap_m)?;
    let (nodes, w) = if g.adj.len() > BI_ASTAR_MIN_NODES {
        bi_astar(&g.adj, &g.coords_geo, src, dst, lat1, lon1, lat2, lon2)?
    } else {
        astar(&g.adj, &g.coords_geo, src, dst, lat2, lon2)?
    };
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
    Some(RouteResult {
        distance_m: dist_m,
        duration_s: weight_to_seconds(w),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;

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
}
