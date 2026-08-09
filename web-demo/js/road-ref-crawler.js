// Discover a long-route spine by following one road reference through the
// spatial roads index. A named highway is many OSM way ids, so ids are useful
// for deduplication but not continuity; the stable signal is a normalised ref
// such as `I 24` appearing in nearby H3 cells.

export function normalizeRoadRefs(value) {
  return String(value || "")
    .split(";")
    .map((part) => part.trim().toUpperCase().replaceAll("-", " ").replace(/\s+/g, " "))
    .filter(Boolean);
}

/**
 * Follow a road ref from `start` to `end` by probing only nearby H3 cells.
 *
 * `h3` supplies `cellFor`, `gridDisk`, `gridPath`, and `center`; `readRefs`
 * asynchronously returns the normalised road refs present in one cell. The
 * returned `spine` is continuous even though matching cells may be two H3
 * steps apart (OSM ways can jump over a cell between recorded vertices).
 */
export async function crawlRoadRefCells({
  start,
  end,
  h3,
  readRefs,
  maxProbeCells = 900,
  neighborRadius = 2,
}) {
  const refsByCell = new Map();

  async function refsAt(cell) {
    let pending = refsByCell.get(cell);
    if (!pending) {
      if (refsByCell.size >= maxProbeCells) return [];
      pending = Promise.resolve(readRefs(cell)).then((refs) =>
        [...new Set((refs || []).flatMap(normalizeRoadRefs))]);
      refsByCell.set(cell, pending);
    }
    return pending;
  }

  function disk(cell) {
    try { return h3.gridDisk(cell, neighborRadius); }
    catch { return [cell]; }
  }

  async function refsNear(point) {
    const cells = disk(h3.cellFor(point[0], point[1]));
    const found = new Map();
    const values = await Promise.all(cells.map(refsAt));
    for (let i = 0; i < cells.length; i++) {
      for (const ref of values[i]) {
        let refCells = found.get(ref);
        if (!refCells) found.set(ref, refCells = []);
        refCells.push(cells[i]);
      }
    }
    return found;
  }

  const [startRefs, endRefs] = await Promise.all([refsNear(start), refsNear(end)]);
  const common = [...startRefs.keys()].filter((ref) => endRefs.has(ref));
  // Interstates first, then US/state refs, with deterministic lexical order.
  common.sort((a, b) => {
    const rank = (r) => r.startsWith("I ") ? 0 : r.startsWith("US ") ? 1 : 2;
    return rank(a) - rank(b) || a.localeCompare(b);
  });

  for (const ref of common) {
    const goals = new Set(endRefs.get(ref));
    const queue = [];
    const previous = new Map();
    const considered = new Set();
    for (const cell of startRefs.get(ref)) {
      queue.push(cell);
      previous.set(cell, null);
      considered.add(cell);
    }

    let head = 0;
    let goal = queue.find((cell) => goals.has(cell)) || null;
    while (!goal && head < queue.length && refsByCell.size < maxProbeCells) {
      const current = queue[head++];
      const candidates = disk(current).filter((cell) => !considered.has(cell));
      candidates.forEach((cell) => considered.add(cell));
      const values = await Promise.all(candidates.map(refsAt));
      for (let i = 0; i < candidates.length; i++) {
        if (!values[i].includes(ref)) continue;
        const next = candidates[i];
        previous.set(next, current);
        queue.push(next);
        if (goals.has(next)) { goal = next; break; }
      }
    }
    if (!goal) continue;

    const path = [];
    for (let cell = goal; cell != null; cell = previous.get(cell)) path.push(cell);
    path.reverse();

    const spine = [];
    const seen = new Set();
    for (let i = 0; i < path.length; i++) {
      let leg = [path[i]];
      if (i) {
        try { leg = h3.gridPath(path[i - 1], path[i]); }
        catch { leg = [path[i - 1], path[i]]; }
      }
      for (const cell of leg) {
        if (!seen.has(cell)) { seen.add(cell); spine.push(cell); }
      }
    }
    return {
      ref,
      path,
      spine,
      probedCells: refsByCell.size,
      matchedCells: queue.length,
      commonRefs: common,
    };
  }

  return {
    ref: null,
    path: [],
    spine: [],
    probedCells: refsByCell.size,
    matchedCells: 0,
    commonRefs: common,
  };
}
