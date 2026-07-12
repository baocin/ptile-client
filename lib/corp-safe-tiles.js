// Tile proxy — Leaflet TileLayer that fetches via fetch() to avoid COEP blocking.
// Standard tile URLs are cross-origin and blocked by COEP: require-corp.
// This layer proxies them through same-origin fetch() + blob URLs.

L.TileLayer.CorpSafe = L.TileLayer.extend({
  createTile: function (coords, done) {
    var url = this.getTileUrl(coords);
    var tile = document.createElement("img");
    tile.alt = "";
    tile.setAttribute("role", "presentation");

    fetch(url, { mode: "cors", credentials: "omit" })
      .then(function (r) {
        if (!r.ok) throw new Error("HTTP " + r.status);
        return r.blob();
      })
      .then(function (blob) {
        tile.src = URL.createObjectURL(blob);
        done(null, tile);
      })
      .catch(function (err) {
        tile.src = url;
        done(err, tile);
      });

    return tile;
  },
});

L.tileLayer.corpSafe = function (url, opts) {
  return new L.TileLayer.CorpSafe(url, opts);
};
