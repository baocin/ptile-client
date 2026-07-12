// Worker for parallel ZSTD decompression
// Each worker gets its own ZSTD WASM instance
// Caches DDict at init — never rebuilds per-decompress call

let mod = null;
let cachedDict = null;
let zstdReady = false;

async function init() {
  mod = await import("./index.web.js");
  await mod.init();
  zstdReady = true;
}

self.onmessage = async function (e) {
  const msg = e.data;

  if (msg.type === "init") {
    await init();
    self.postMessage({ type: "ready", id: msg.id });
    return;
  }

  // Cache dict for reuse
  if (msg.type === "set_dict") {
    if (!zstdReady) await init();
    cachedDict = new Uint8Array(msg.dict);
    self.postMessage({ type: "dict_ready", id: msg.id });
    return;
  }

  if (msg.type === "decompress") {
    try {
      const compressed = new Uint8Array(msg.compressed);
      var dict = cachedDict || new Uint8Array(0);
      var dctx = mod.createDCtx();
      var result;
      try {
        result = mod.decompressUsingDict(dctx, compressed, dict);
      } finally {
        mod.freeDCtx(dctx);
      }
      self.postMessage(
        { type: "result", id: msg.id, jobId: msg.jobId, result: result.buffer },
        [result.buffer],
      );
    } catch (e) {
      self.postMessage({
        type: "error",
        id: msg.id,
        jobId: msg.jobId,
        error: e.message,
      });
    }
    return;
  }
};
