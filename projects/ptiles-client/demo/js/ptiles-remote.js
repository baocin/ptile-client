// Thin range-request + framing glue around the ptiles-wasm exports.
//
// This file does NOT decode any PTILES record format -- every byte of
// feature/header/index decoding goes through wasm (ptiles-core). What's
// here is the part wasm deliberately doesn't do (see ptiles-wasm/src/lib.rs
// module doc: "no async, no I/O"): issuing HTTP Range requests and slicing
// the right byte ranges out of the responses before handing bytes to wasm.
//
// Loading pattern (see ../../docs/INTEGRATION.md "native path" section):
//   1. Range-fetch the fixed 256-byte header, parse it with wasm.parse_header.
//   2. Range-fetch the dict (if any) and the spatial index, parse the index
//      with wasm.parse_index_entries / look up cells with wasm.find_block_for_cell.
//   3. Per visible H3 cell: range-fetch just that cell's compressed block,
//      wasm.decompress_block it, then hand the raw bytes to the matching
//      wasm.decode_* export.
//
// Unlike the deployed corp-tiles demo (steele.red/ptiles), which does one
// whole-file fetch per layer, this fetches only the header + index + the
// blocks actually needed for the current viewport, matching how the native
// (CLI/FFI) HttpSource path behaves.

const HEADER_SIZE = 256;

async function rangeFetch(url, start, endInclusive) {
  const resp = await fetch(url, { headers: { Range: `bytes=${start}-${endInclusive}` } });
  if (!resp.ok) throw new Error(`HTTP ${resp.status} fetching ${url} bytes=${start}-${endInclusive}`);
  return new Uint8Array(await resp.arrayBuffer());
}

export class PtilesRemoteFile {
  constructor(wasm, url) {
    this.wasm = wasm;
    this.url = url;
    this.header = null;
    this.dict = new Uint8Array(0);
    this.indexBytes = null;
  }

  async open() {
    const headerBytes = await rangeFetch(this.url, 0, HEADER_SIZE - 1);
    this.header = this.wasm.parse_header(headerBytes);

    const dictLen = Number(this.header.dict_length);
    if (dictLen > 0) {
      const dictOff = Number(this.header.dict_offset);
      this.dict = await rangeFetch(this.url, dictOff, dictOff + dictLen - 1);
    }

    const idxOff = Number(this.header.index_offset);
    const idxLen = Number(this.header.index_length);
    this.indexBytes = await rangeFetch(this.url, idxOff, idxOff + idxLen - 1);
    return this;
  }

  /** Decompressed block bytes for `cellHex`, or `null` if the file has no
   * block for that cell (sparse spatial coverage -- not an error). */
  async blockForCell(cellHex) {
    const entry = this.wasm.find_block_for_cell(this.indexBytes, cellHex);
    if (!entry) return null;
    const blocksOffset = Number(this.header.blocks_offset);
    let off = Number(entry.block_offset);
    // Some real files store block_offset relative to blocks_offset rather
    // than absolute (observed by the deployed demo too, see its `relOff`
    // check) -- normalize to absolute here, once, so every caller downstream
    // just gets a byte range.
    if (off < blocksOffset) off += blocksOffset;
    const len = Number(entry.block_length);
    const compressed = await rangeFetch(this.url, off, off + len - 1);
    return this.wasm.decompress_block(compressed, this.dict);
  }
}

/** State postal code -> `{STATE}.{layer}.ptiles` URL on the real dataset host. */
export function stateLayerUrl(state, layer) {
  return `https://maps.mydatatimeline.com/maps/${state}.${layer}.ptiles`;
}
