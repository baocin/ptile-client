//! What is actually inside the highways pack?
use ptiles_core::file::PtilesFile;
use ptiles_core::source::FileSource;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = PtilesFile::open(FileSource::open(&path).unwrap()).unwrap();
    let header = file.header();
    println!("version {}  index entries {}", header.version, file.index().len());
    let mut decoded_ok = 0usize;
    let mut segments = 0usize;
    let mut first_error: Option<String> = None;
    for entry in file.index().iter().take(40) {
        let Some(block) = file.read_block(entry.h3_cell).unwrap() else { continue };
        match ptiles_core::decode_road_block(&block, header.version) {
            Ok((roads, _)) => {
                decoded_ok += 1;
                segments += roads.len();
                if segments > 0 && decoded_ok == 1 {
                    let r = &roads[0];
                    println!(
                        "first: {} ({}) {} points",
                        r.name.clone().unwrap_or_else(|| "unnamed".into()),
                        r.road_class,
                        r.coords.len(),
                    );
                }
            }
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(format!("{e}"));
                }
            }
        }
    }
    println!("{decoded_ok}/40 blocks decoded, {segments} segments");
    if let Some(e) = first_error {
        println!("first decode error: {e}");
    }
    // Are the bytes even plausible? A dictionary that was not applied leaves
    // compressed noise; a layout difference leaves readable structure.
    if let Some(entry) = file.index().first() {
        if let Ok(Some(block)) = file.read_block(entry.h3_cell) {
            println!("first block {} bytes, head: {:02x?}", block.len(), &block[..block.len().min(24)]);
            let text: String = block
                .iter()
                .take(120)
                .map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' })
                .collect();
            println!("as text: {text}");
        }
    }
    // Try the versions the decoder knows, in case the header disagrees.
    for v in [1u8, 2, 3] {
        let Some(entry) = file.index().first() else { break };
        let Some(block) = file.read_block(entry.h3_cell).unwrap() else { break };
        match ptiles_core::decode_road_block(&block, v) {
            Ok((roads, _)) => println!("  as version {v}: {} segments", roads.len()),
            Err(e) => println!("  as version {v}: {e}"),
        }
    }
}
