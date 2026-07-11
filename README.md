# ptile-client

Rust workspace for the [PTiles binary geospatial format](https://github.com/baocin/ptiles).
`no_std` decoder core, WASM browser bridge, native CLI, fuzz harness.

## Crates

| Crate  | What                                                                              |
| ------ | --------------------------------------------------------------------------------- |
| `core` | `no_std`-optional decoder library — zero-alloc block parser for all PTiles layers |
| `wasm` | wasm-bindgen bridge — decode PTiles in the browser via WebAssembly                |
| `cli`  | Native JSON bridge for Rookery — pipe lat/lon → JSON feature                      |
| `fuzz` | AFL/libfuzzer harness — crash-testing byte-level decoders                         |

## Quick Start

```bash
# Build everything
cargo build --workspace

# Run tests
cargo test --workspace

# Build WASM
cd wasm && wasm-pack build --target web
```

## Demo

Click any building in the US: https://steele.red/ptiles

## Live Tiles

https://maps.mydatatimeline.com/maps/v4-20260711/{ST}.{layer}.ptiles

Layers: `buildings_v9`, `business_v4`, `highways_v2`, `business_name_index`, `address_v1`, `water_v1`, `places_v1`, `parks_v1`, `rail_v1`

## License

MIT
