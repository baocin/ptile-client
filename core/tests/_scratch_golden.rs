use std::fs;
use serde_json::Value;

fn fixt(name: &str) -> Vec<u8> {
    let p = format!("{}/../test-fixtures/golden/{}", env!("CARGO_MANIFEST_DIR"), name);
    fs::read(p).unwrap()
}
fn golden(name: &str) -> Value {
    let p = format!("{}/../test-fixtures/golden/{}", env!("CARGO_MANIFEST_DIR"), name);
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}

#[test]
fn water_golden() {
    let f = ptiles_core::water::decode_water(&fixt("water.block.bin")).unwrap();
    let g = golden("water.golden.json");
    let gf = g["features"].as_array().unwrap();
    println!("water decoded {} golden {}", f.len(), gf.len());
    assert_eq!(f.len(), gf.len(), "water count");
    for (i, (d, e)) in f.iter().zip(gf).enumerate() {
        assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap(), "water osm {i}");
        assert_eq!(d.coords.len(), e["coords"].as_array().unwrap().len(), "water coordlen {i}");
        assert_eq!(d.water_type, e["water_type"].as_str().unwrap(), "water type {i}");
        let c0 = &e["coords"][0];
        if let Some(a) = c0.as_array() {
            assert!((d.coords[0][0]-a[0].as_f64().unwrap()).abs()<1e-6, "water lon {i}");
            assert!((d.coords[0][1]-a[1].as_f64().unwrap()).abs()<1e-6, "water lat {i}");
        }
    }
}

#[test]
fn parks_golden() {
    let f = ptiles_core::parks::decode_parks(&fixt("parks.block.bin")).unwrap();
    let g = golden("parks.golden.json");
    let gf = g["features"].as_array().unwrap();
    println!("parks decoded {} golden {}", f.len(), gf.len());
    assert_eq!(f.len(), gf.len(), "parks count");
    for (i,(d,e)) in f.iter().zip(gf).enumerate() {
        assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap(), "parks osm {i}");
        assert_eq!(d.coords.len(), e["coords"].as_array().unwrap().len(), "parks coordlen {i}");
        assert_eq!(d.park_type, e["park_type"].as_str().unwrap(), "parks type {i}");
    }
}

#[test]
fn rail_golden() {
    let f = ptiles_core::rail::decode_rail(&fixt("rail.block.bin")).unwrap();
    let g = golden("rail.golden.json");
    let gf = g["features"].as_array().unwrap();
    assert_eq!(f.len(), gf.len(), "rail count");
    for (i,(d,e)) in f.iter().zip(gf).enumerate() {
        assert_eq!(d.osm_id, e["osm_id"].as_i64().unwrap(), "rail osm {i}");
        assert_eq!(d.rail_type, e["rail_type"].as_str().unwrap(), "rail type {i}");
        assert_eq!(d.coords.len(), e["coords"].as_array().unwrap().len(), "rail coordlen {i}");
    }
}
