// zstd-loader.js — zstd decompression for PTILES v8 files.
// Re-exports from @bokuweb/zstd-wasm's index.web.js which handles
// wasm loading and dictionary decompression properly.

export async function createZstd() {
  const mod = await import("./lib/index.web.js");
  await mod.init();
  return {
    decompressUsingDict: mod.decompressUsingDict,
  };
}
