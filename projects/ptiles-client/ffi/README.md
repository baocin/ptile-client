# ptiles-ffi

UniFFI bindings (proc-macro mode, no `.udl`) exposing `ptiles-core` to Swift
and Kotlin. See `src/lib.rs` for the exported API surface (`PtilesLayer`,
`PtilesStack`, records, `PtilesError`).

Generated bindings are checked in under `bindings/`:
- `bindings/swift/` — `ptiles_ffi.swift`, `ptiles_ffiFFI.h`, `ptiles_ffiFFI.modulemap`
- `bindings/kotlin/` — `uniffi/ptiles_ffi/ptiles_ffi.kt`

Regenerate them any time `src/lib.rs`'s exported API changes.

## Regenerating bindings (either platform, from any host)

Bindings are derived from the built `cdylib`/`so`, not from source directly,
so you need a compiled library first — but it does **not** need to be for
the target platform; any successful build of `ptiles-ffi` produces a library
uniffi-bindgen can read.

```bash
cargo build -p ptiles-ffi
cargo run -p ptiles-ffi --bin uniffi-bindgen --features uniffi/cli -- \
  generate --library target/debug/libptiles_ffi.so \
  --language swift --out-dir ffi/bindings/swift

cargo run -p ptiles-ffi --bin uniffi-bindgen --features uniffi/cli -- \
  generate --library target/debug/libptiles_ffi.so \
  --language kotlin --out-dir ffi/bindings/kotlin
```

(On Linux the library is `libptiles_ffi.so`; on macOS it would be
`libptiles_ffi.dylib`.)

If `swift-format` / `ktlint` aren't installed, bindgen prints a
non-fatal warning and leaves the generated file unformatted but valid.

## Android (cross-compiled here, Linux host)

Requirements: Android NDK (works with r26d/r28+, tested against r28) and
`cargo-ndk`.

```bash
cargo install cargo-ndk   # small pure-Rust cargo subcommand, not a toolchain
rustup target add aarch64-linux-android armv7-linux-androideabi \
  i686-linux-android x86_64-linux-android

export ANDROID_NDK_HOME=/path/to/android-ndk-rXX   # NOT a symlink dir named
                                                     # after the version alone —
                                                     # point at the actual
                                                     # android-ndk-rXX directory

cargo ndk -t arm64-v8a -o ffi/target-android build -p ptiles-ffi --release
# add more -t flags for other ABIs, e.g. -t armeabi-v7a -t x86_64
```

Output: `ffi/target-android/<abi>/libptiles_ffi.so`, ready to drop into
`app/src/main/jniLibs/<abi>/` of an Android project alongside the generated
Kotlin bindings in `bindings/kotlin/`.

This step was verified on this (Linux) host: `arm64-v8a` build succeeds,
and the Kotlin bindings regenerated from the resulting `.so` are identical
to the ones checked into `bindings/kotlin/`.

## iOS / macOS — requires a Mac

Apple targets (`aarch64-apple-ios`, `aarch64-apple-ios-sim`,
`aarch64-apple-darwin`, `x86_64-apple-darwin`) cannot be compiled on Linux —
there is no Apple SDK/toolchain to target. This must be done on macOS.

On a Mac, from the workspace root:

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim \
  aarch64-apple-darwin x86_64-apple-darwin

cargo build -p ptiles-ffi --release --target aarch64-apple-ios
cargo build -p ptiles-ffi --release --target aarch64-apple-ios-sim
cargo build -p ptiles-ffi --release --target aarch64-apple-darwin
cargo build -p ptiles-ffi --release --target x86_64-apple-darwin

# Merge simulator slices with lipo, then assemble an XCFramework:
lipo -create \
  target/aarch64-apple-darwin/release/libptiles_ffi.a \
  target/x86_64-apple-darwin/release/libptiles_ffi.a \
  -output libptiles_ffi-macos.a

xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libptiles_ffi.a \
  -headers ffi/bindings/swift \
  -library target/aarch64-apple-ios-sim/release/libptiles_ffi.a \
  -headers ffi/bindings/swift \
  -library libptiles_ffi-macos.a \
  -headers ffi/bindings/swift \
  -output PtilesFFI.xcframework
```

Regenerate the Swift bindings first (see above) so the header/modulemap in
`ffi/bindings/swift` match the current API before building the xcframework.
Wrap `PtilesFFI.xcframework` plus `bindings/swift/ptiles_ffi.swift` in a
Swift Package (or drop directly into an Xcode project) to consume from
iOS/macOS app code.
