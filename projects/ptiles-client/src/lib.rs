// LEGACY SEED — superseded by core/ + wasm/, kept until wasm parity is confirmed in the demo.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

// Decoded structures matching JS GeoJSON convention
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoadSegment {
    pub osm_id: u64,
    pub road_class: String,
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
    pub speed_limit_kmh: Option<u8>,
    pub lanes: Option<u8>,
    pub surface: Option<String>,
    pub oneway: Option<String>,
    pub bridge_tunnel: Option<String>,
    pub ref_tag: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WaterFeature {
    pub osm_id: u64,
    pub geom_type: u8,
    pub water_type: u8,
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ParkFeature {
    pub osm_id: u64,
    pub park_type: String,
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RailFeature {
    pub osm_id: u64,
    pub geom_type: u8,
    pub rail_type: u8,
    pub coords: Vec<[f64; 2]>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Building {
    pub osm_id: u64,
    pub building_type: String,
    pub coords: Vec<[f64; 2]>,
    pub name: Option<String>,
    pub category: Option<String>,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Business {
    pub uid: u64,
    pub name: String,
    pub category: u8,
    pub lat: f64,
    pub lon: f64,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub address: Option<String>,
    pub brand: Option<String>,
    pub chain_count: u8,
}

// --- helpers ---

fn u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}
fn u64(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
}
fn u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(data[off..off + 2].try_into().unwrap())
}
fn i16(data: &[u8], off: usize) -> i16 {
    i16::from_le_bytes(data[off..off + 2].try_into().unwrap())
}
fn i32(data: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}
fn f32(data: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn read_packed(data: &[u8], off: usize, len: usize) -> u64 {
    let mut r = 0u64;
    for i in 0..len {
        if off + i < data.len() {
            r |= (data[off + i] as u64) << (i * 8);
        }
    }
    r
}

fn decode_varint(data: &[u8], start: usize) -> (u64, usize) {
    let mut r = 0u64;
    let mut s = 0u64;
    let mut p = start;
    while p < data.len() {
        let b = data[p] as u64;
        p += 1;
        r |= (b & 0x7f) << s;
        if (b & 0x80) == 0 {
            break;
        }
        s += 7;
    }
    (r, p - start)
}

fn zigzag_decode(n: u64) -> i64 {
    if n & 1 == 0 {
        (n >> 1) as i64
    } else {
        -(((n + 1) >> 1) as i64)
    }
}

fn zigzag_i32(n: u64) -> i32 {
    let n = n as u32;
    ((n >> 1) as i32) ^ (-((n & 1) as i32))
}

fn decode_string(data: &[u8], off: usize, len: usize) -> String {
    String::from_utf8_lossy(&data[off..off + len]).to_string()
}

// --- Road decoder ---

#[wasm_bindgen]
pub fn decode_roads(data: &[u8]) -> JsValue {
    let mut roads = Vec::new();
    let mut p = 0usize;
    let mut prev_osm = 0u64;

    while p + 4 <= data.len() {
        let rl = u32(data, p) as usize;
        p += 4;
        if rl == 0 || p + rl > data.len() {
            break;
        }
        let rec = &data[p..p + rl];
        let mut rp = 0usize;

        let (dv, consumed) = decode_varint(rec, rp);
        rp += consumed;
        let osm_id = prev_osm.wrapping_add(dv as u64);
        prev_osm = osm_id;

        if rp + 2 > rec.len() {
            p += rl;
            continue;
        }
        let vc = rec[rp] as u16 | ((rec[rp + 1] as u16) << 8);
        rp += 2;
        if vc == 0 || rp + 8 > rec.len() {
            p += rl;
            continue;
        }
        let first_lon = i32(rec, rp);
        let first_lat = i32(rec, rp + 4);
        rp += 8;

        let mut coords = vec![[first_lon as f64 / 100000.0, first_lat as f64 / 100000.0]];
        let mut plon = first_lon;
        let mut plat = first_lat;
        for _ in 1..vc {
            if rp >= rec.len() {
                break;
            }
            let (r1, c1) = decode_varint(rec, rp);
            rp += c1;
            let (r2, c2) = decode_varint(rec, rp);
            rp += c2;
            plon += zigzag_i32(r1);
            plat += zigzag_i32(r2);
            coords.push([plon as f64 / 100000.0, plat as f64 / 100000.0]);
        }

        let road_class = if rp < rec.len() {
            let flags = rec[rp];
            rp += 1;
            if rp < rec.len() {
                let rc = rec[rp];
                rp += 1;
                if rc == 255 {
                    if rp < rec.len() {
                        let clen = rec[rp] as usize;
                        rp += 1 + clen;
                    }
                    8
                } else {
                    rc as u32
                }
            } else {
                8
            }
        } else {
            8
        };

        // Optional name (flag-based after road_class)
        let mut name = None;
        if rp + 1 <= rec.len() {
            let flags2 = rec[rp];
            rp += 1;
            if flags2 & 0x01 != 0 && rp < rec.len() {
                let nlen = rec[rp] as usize;
                rp += 1;
                if rp + nlen <= rec.len() {
                    name = Some(decode_string(rec, rp, nlen));
                    rp += nlen;
                }
            }
        }

        roads.push(RoadSegment {
            osm_id,
            road_class: format!("{}", road_class),
            coords,
            name,
            speed_limit_kmh: None,
            lanes: None,
            surface: None,
            oneway: None,
            bridge_tunnel: None,
            ref_tag: None,
        });
        p += rl;
    }

    serde_wasm_bindgen::to_value(&roads).unwrap()
}

// --- Water decoder ---

#[wasm_bindgen]
pub fn decode_water(data: &[u8]) -> JsValue {
    let mut features = Vec::new();
    let mut p = 0usize;
    let mut prev_osm = 0u64;

    while p < data.len() {
        let (dv, consumed) = decode_varint(data, p);
        p += consumed;
        let osm_id = prev_osm.wrapping_add(zigzag_decode(dv) as u64);
        prev_osm = osm_id;

        if p >= data.len() {
            break;
        }
        let geom_type = data[p];
        p += 1;

        let mut coords = Vec::new();
        if geom_type == 2 {
            // reference to large feature - skip (4 bytes ref_id)
            p += 4;
        } else {
            if p + 2 > data.len() {
                break;
            }
            let vc = data[p] as u16 | ((data[p + 1] as u16) << 8);
            p += 2;
            if vc > 0 && p + 8 <= data.len() {
                let first_lon = i32(data, p);
                let first_lat = i32(data, p + 4);
                p += 8;
                coords.push([first_lon as f64 / 100000.0, first_lat as f64 / 100000.0]);
                let mut plon = first_lon;
                let mut plat = first_lat;
                for _ in 1..vc {
                    if p >= data.len() {
                        break;
                    }
                    let (r1, c1) = decode_varint(data, p);
                    p += c1;
                    let (r2, c2) = decode_varint(data, p);
                    p += c2;
                    plon += zigzag_i32(r1);
                    plat += zigzag_i32(r2);
                    coords.push([plon as f64 / 100000.0, plat as f64 / 100000.0]);
                }
            }
        }

        if p + 2 > data.len() {
            break;
        }
        let flags = data[p];
        p += 1;
        let water_type = data[p];
        p += 1;

        if flags & 0x01 != 0 {
            if p + 2 > data.len() {
                break;
            }
            let nlen = data[p] as u16 | ((data[p + 1] as u16) << 8);
            p += 2 + nlen as usize;
        }
        if flags & 0x02 != 0 {
            p += 2;
        }
        if flags & 0x04 != 0 {
            p += 2;
        }

        if coords.len() >= 2 {
            features.push(WaterFeature {
                osm_id,
                geom_type,
                water_type,
                coords,
                name: None,
            });
        }
    }

    serde_wasm_bindgen::to_value(&features).unwrap()
}

// --- Parks decoder ---

#[wasm_bindgen]
pub fn decode_parks(data: &[u8]) -> JsValue {
    let mut features = Vec::new();
    let mut p = 0usize;
    let mut prev_osm = 0u64;

    while p < data.len() {
        let (dv, consumed) = decode_varint(data, p);
        p += consumed;
        let osm_id = prev_osm.wrapping_add(zigzag_decode(dv) as u64);
        prev_osm = osm_id;

        if p >= data.len() {
            break;
        }
        let mut vc = data[p] as usize;
        p += 1;
        if vc == 255 {
            if p + 2 > data.len() {
                break;
            }
            vc = data[p] as usize | ((data[p + 1] as usize) << 8);
            p += 2;
        }

        if p + 8 > data.len() {
            break;
        }
        let first_lon = i32(data, p);
        let first_lat = i32(data, p + 4);
        p += 8;
        let mut coords = vec![[first_lon as f64 / 100000.0, first_lat as f64 / 100000.0]];
        let mut plon = first_lon;
        let mut plat = first_lat;
        for _ in 1..vc {
            if p >= data.len() {
                break;
            }
            let (r1, c1) = decode_varint(data, p);
            p += c1;
            let (r2, c2) = decode_varint(data, p);
            p += c2;
            plon += zigzag_i32(r1);
            plat += zigzag_i32(r2);
            coords.push([plon as f64 / 100000.0, plat as f64 / 100000.0]);
        }

        if p >= data.len() {
            break;
        }
        let pt_len = data[p] as usize;
        p += 1;
        let park_type = if p + pt_len <= data.len() {
            decode_string(data, p, pt_len)
        } else {
            String::new()
        };
        p += pt_len;

        if p >= data.len() {
            break;
        }
        let flags = data[p];
        p += 1;
        if flags & 0x01 != 0 {
            if p + 2 > data.len() {
                break;
            }
            let nlen = data[p] as u16 | ((data[p + 1] as u16) << 8);
            p += 2 + nlen as usize;
        }

        if coords.len() >= 3 {
            features.push(ParkFeature {
                osm_id,
                park_type,
                coords,
                name: None,
            });
        }
    }

    serde_wasm_bindgen::to_value(&features).unwrap()
}

// --- Rail decoder ---

#[wasm_bindgen]
pub fn decode_rail(data: &[u8]) -> JsValue {
    let mut features = Vec::new();
    let mut p = 0usize;
    let mut prev_osm = 0u64;

    while p < data.len() {
        let (dv, consumed) = decode_varint(data, p);
        p += consumed;
        let osm_id = prev_osm.wrapping_add(zigzag_decode(dv) as u64);
        prev_osm = osm_id;

        if p >= data.len() {
            break;
        }
        let geom_type = data[p];
        p += 1;

        let mut coords = Vec::new();
        if geom_type == 1 {
            // point/station
            if p + 8 > data.len() {
                break;
            }
            let first_lon = i32(data, p);
            let first_lat = i32(data, p + 4);
            p += 8;
            coords.push([first_lon as f64 / 100000.0, first_lat as f64 / 100000.0]);
        } else {
            // linestring
            if p + 2 > data.len() {
                break;
            }
            let vc = data[p] as u16 | ((data[p + 1] as u16) << 8);
            p += 2;
            if p + 8 > data.len() {
                break;
            }
            let first_lon = i32(data, p);
            let first_lat = i32(data, p + 4);
            p += 8;
            coords.push([first_lon as f64 / 100000.0, first_lat as f64 / 100000.0]);
            let mut plon = first_lon;
            let mut plat = first_lat;
            for _ in 1..vc {
                if p >= data.len() {
                    break;
                }
                let (r1, c1) = decode_varint(data, p);
                p += c1;
                let (r2, c2) = decode_varint(data, p);
                p += c2;
                plon += zigzag_i32(r1);
                plat += zigzag_i32(r2);
                coords.push([plon as f64 / 100000.0, plat as f64 / 100000.0]);
            }
        }

        if p >= data.len() {
            break;
        }
        let rail_type = data[p];
        p += 1;
        if p >= data.len() {
            break;
        }
        let flags = data[p];
        p += 1;

        if flags & 0x01 != 0 {
            if p + 2 > data.len() {
                break;
            }
            let nlen = data[p] as u16 | ((data[p + 1] as u16) << 8);
            p += 2 + nlen as usize;
        }
        if flags & 0x02 != 0 {
            if p >= data.len() {
                break;
            }
            let olen = data[p] as usize;
            p += 1 + olen;
        }
        if flags & 0x04 != 0 {
            p += 2;
        }
        if flags & 0x08 != 0 {
            p += 1;
        }

        features.push(RailFeature {
            osm_id,
            geom_type,
            rail_type,
            coords,
        });
    }

    serde_wasm_bindgen::to_value(&features).unwrap()
}

// --- Building decoder (buildings_v8 format) ---

#[wasm_bindgen]
pub fn decode_buildings(data: &[u8], cell_center_lat: f64, cell_center_lon: f64) -> JsValue {
    let mut buildings = Vec::new();
    let cx = (cell_center_lon * 100000.0).round() as i32;
    let cy = (cell_center_lat * 100000.0).round() as i32;

    let str_count = data[0] as usize;
    let mut p = 1usize;
    let mut strings = Vec::new();
    for _ in 0..str_count {
        if p >= data.len() {
            break;
        }
        let slen = data[p] as usize;
        p += 1;
        if p + slen > data.len() {
            break;
        }
        strings.push(decode_string(data, p, slen));
        p += slen;
    }

    let mut prev_osm = 0i64;
    while p + 4 <= data.len() {
        let rl = u32(data, p) as usize;
        p += 4;
        if p + rl > data.len() {
            break;
        }
        let rec = &data[p..p + rl];
        let mut rp = 0usize;

        let (dv, consumed) = decode_varint(rec, rp);
        rp += consumed;
        let osm_delta = zigzag_decode(dv);
        let osm_id = prev_osm.wrapping_add(osm_delta);
        prev_osm = osm_id;

        if rp >= rec.len() {
            p += rl;
            continue;
        }
        let flags = rec[rp];
        rp += 1;
        let mut vc = ((flags >> 4) & 0x0f) as usize;
        if vc == 0x0f {
            if rp >= rec.len() {
                p += rl;
                continue;
            }
            vc = rec[rp] as usize;
            rp += 1;
        } else {
            vc += 4;
        }

        if vc == 0 || rp + 4 > rec.len() {
            p += rl;
            continue;
        }
        let fl = i16(rec, rp) as i32;
        let fa = i16(rec, rp + 2) as i32;
        rp += 4;

        let mut coords = vec![[(cx + fl) as f64 / 100000.0, (cy + fa) as f64 / 100000.0]];
        let mut prev_lon = cx + fl;
        let mut prev_lat = cy + fa;
        for _ in 1..vc {
            if rp >= rec.len() {
                break;
            }
            let (r1, c1) = decode_varint(rec, rp);
            rp += c1;
            let (r2, c2) = decode_varint(rec, rp);
            rp += c2;
            prev_lon += zigzag_i32(r1);
            prev_lat += zigzag_i32(r2);
            coords.push([prev_lon as f64 / 100000.0, prev_lat as f64 / 100000.0]);
        }

        let building_type = if rp < rec.len() {
            let bt = rec[rp];
            rp += 1;
            if bt == 0xff {
                if rp < rec.len() {
                    let slen = rec[rp] as usize;
                    rp += 1;
                    if rp + slen <= rec.len() {
                        let s = decode_string(rec, rp, slen);
                        rp += slen;
                        s
                    } else {
                        String::from("yes")
                    }
                } else {
                    String::from("yes")
                }
            } else {
                if (bt as usize) < strings.len() {
                    strings[bt as usize].clone()
                } else {
                    String::from("yes")
                }
            }
        } else {
            String::from("yes")
        };

        let mut name = None;
        let mut category = None;
        if rp < rec.len() {
            let f2 = rec[rp];
            rp += 1;
            if f2 & 0x01 != 0 && rp < rec.len() {
                let idx = rec[rp];
                rp += 1;
                if idx == 0xff {
                    if rp < rec.len() {
                        let slen = rec[rp] as usize;
                        rp += 1;
                        if rp + slen <= rec.len() {
                            name = Some(decode_string(rec, rp, slen));
                            rp += slen;
                        }
                    }
                } else {
                    name = if (idx as usize) < strings.len() {
                        Some(strings[idx as usize].clone())
                    } else {
                        None
                    };
                }
            }
            if f2 & 0x02 != 0 && rp < rec.len() {
                let idx = rec[rp];
                rp += 1;
                if idx == 0xff {
                    if rp < rec.len() {
                        let slen = rec[rp] as usize;
                        rp += 1;
                        if rp + slen <= rec.len() {
                            category = Some(decode_string(rec, rp, slen));
                            rp += slen;
                        }
                    }
                } else {
                    category = if (idx as usize) < strings.len() {
                        Some(strings[idx as usize].clone())
                    } else {
                        None
                    };
                }
            }
        }

        let (centroid_lon, centroid_lat) = {
            let mut sx = 0f64;
            let mut sy = 0f64;
            for c in &coords {
                sx += c[0];
                sy += c[1];
            }
            (sx / coords.len() as f64, sy / coords.len() as f64)
        };

        buildings.push(Building {
            osm_id: osm_id as u64,
            building_type,
            coords,
            name,
            category,
            centroid_lat,
            centroid_lon,
        });

        p += rl;
    }

    serde_wasm_bindgen::to_value(&buildings).unwrap()
}

// --- Business decoder ---

#[wasm_bindgen]
pub fn decode_business(data: &[u8]) -> JsValue {
    let mut records = Vec::new();
    let mut prev_uid = 0i64;
    let mut p = 0usize;

    while p + 4 <= data.len() {
        let rl = u32(data, p) as usize;
        p += 4;
        if p + rl > data.len() {
            break;
        }
        let rec = &data[p..p + rl];
        let mut rp = 0usize;

        let (dv, consumed) = decode_varint(rec, rp);
        rp += consumed;
        let uid = prev_uid.wrapping_add(zigzag_decode(dv));
        prev_uid = uid;

        if rp + 8 > rec.len() {
            p += rl;
            continue;
        }
        let biz_lon = i32(rec, rp) as f64 / 100000.0;
        let biz_lat = i32(rec, rp + 4) as f64 / 100000.0;
        rp += 8;

        if rp + 2 > rec.len() {
            p += rl;
            continue;
        }
        let nlen = u16(rec, rp) as usize;
        rp += 2;
        let name = if rp + nlen <= rec.len() {
            decode_string(rec, rp, nlen)
        } else {
            String::new()
        };
        rp += nlen;

        if rp >= rec.len() {
            p += rl;
            continue;
        }
        let cat_idx = rec[rp];
        rp += 1;
        if rp >= rec.len() {
            p += rl;
            continue;
        }
        let flags = rec[rp];
        rp += 1;

        let mut phone = None;
        let mut website = None;
        let mut address = None;
        let mut brand = None;
        let mut chain_count = 0u8;

        if flags & 0x01 != 0 && rp < rec.len() {
            let plen = rec[rp] as usize;
            rp += 1;
            if rp + plen <= rec.len() {
                phone = Some(decode_string(rec, rp, plen));
                rp += plen;
            }
        }
        if flags & 0x02 != 0 && rp < rec.len() {
            let wlen = rec[rp] as usize;
            rp += 1;
            if rp + wlen <= rec.len() {
                website = Some(decode_string(rec, rp, wlen));
                rp += wlen;
            }
        }
        if flags & 0x04 != 0 && rp + 2 <= rec.len() {
            let alen = u16(rec, rp) as usize;
            rp += 2;
            if rp + alen <= rec.len() {
                address = Some(decode_string(rec, rp, alen));
                rp += alen;
            }
        }
        if flags & 0x08 != 0 && rp < rec.len() {
            let blen = rec[rp] as usize;
            rp += 1;
            if rp + blen <= rec.len() {
                brand = Some(decode_string(rec, rp, blen));
                rp += blen;
            }
        }
        if flags & 0x80 != 0 && rp < rec.len() {
            chain_count = rec[rp];
            rp += 1;
        }

        records.push(Business {
            uid: uid as u64,
            name,
            category: cat_idx,
            lat: biz_lat,
            lon: biz_lon,
            phone,
            website,
            address,
            brand,
            chain_count,
        });

        p += rl;
    }

    serde_wasm_bindgen::to_value(&records).unwrap()
}
