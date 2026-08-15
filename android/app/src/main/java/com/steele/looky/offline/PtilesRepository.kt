package com.steele.looky.offline

import android.content.Context
import com.steele.looky.model.GeoPoint
import com.steele.looky.model.MapFeature
import uniffi.ptiles_ffi.PtilesLayer
import uniffi.ptiles_ffi.PtilesStack
import uniffi.ptiles_ffi.AdminLayer
import uniffi.ptiles_ffi.BusinessInfo
import uniffi.ptiles_ffi.CameraInfo
import uniffi.ptiles_ffi.OfflineRouteMode
import uniffi.ptiles_ffi.RoadContext
import uniffi.ptiles_ffi.TrailInfo
import uniffi.ptiles_ffi.LatLon
import uniffi.ptiles_ffi.Navigator
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.cos

data class NearbyContext(
    val roadName: String?,
    val roadClass: String?,
    val roadDistanceM: Double?,
)

class PtilesRepository(context: Context) {
    private val appContext = context.applicationContext
    private val manager = PackManager(context)
    /** A sample centre on the global grid, and what was asked of it. */
    private data class CellKey(val y: Int, val x: Int, val flags: Int)

    private data class CachedLayer(
        val length: Long,
        val modified: Long,
        val layer: PtilesLayer,
    )

    // Cache by path, not logical layer. More than one state can be installed,
    // and caching the first file under "roads" made every other state query
    // use that state's graph.
    private val layers = ConcurrentHashMap<String, CachedLayer>()
    @Volatile private var adminLayer: AdminLayer? = null
    @Volatile private var stateHint: String? = null

    // Empty string is "looked, found nothing": a map cannot hold null, and
    // re-running a lookup that already came back empty is the expensive case.
    private val placeCache = ConcurrentHashMap<Pair<Long, Long>, String>()

    /**
     * Decoded features for one sample centre, keyed on the global grid.
     *
     * Sample centres are snapped to a fixed lat/lon grid rather than laid out
     * around wherever the viewport happens to sit, so a pan asks for mostly the
     * same centres the last fetch already decoded and only the leading edge
     * costs anything. Without it every 220 m nudge re-decoded the whole screen,
     * which is what a fast pan outran.
     *
     * ponytail: an LRU counted in entries, not bytes. A dense city centre is a
     * few thousand features, so this bound is tens of megabytes at worst --
     * measure before raising it.
     */
    private val cellCache = object : LinkedHashMap<CellKey, List<MapFeature>>(32, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<CellKey, List<MapFeature>>) =
            size > MAX_CACHED_CELLS
    }

    /**
     * Jurisdiction rings for one grid cell.
     *
     * The admin pack holds 6,245 rings and the scan runs once per fetch, which
     * on a pan is the only work left after the feature cache answers.
     */
    private val adminCache = ConcurrentHashMap<Pair<Int, Int>, List<MapFeature>>()

    /** Park rings for one grid cell: 225 ring queries, so they are kept. */
    private val parkCache = ConcurrentHashMap<Pair<Int, Int>, List<Pair<String, List<GeoPoint>>>>()

    /** Where the previous fetch was centred, so the next one can lead the pan. */
    @Volatile private var lastFetchCenter: GeoPoint? = null

    data class RouteResult(
        val points: List<GeoPoint>,
        val distanceM: Double,
        val durationS: Double,
        val decodedSegments: Int,
    )

    /**
     * One searchable place, from whichever layer produced it.
     *
     * `brand` is the chain a place belongs to and is scored alongside the
     * primary name, so "Shell" finds "Shell Oil 41762". `note` is what the row
     * says about why this hit is here -- the park it stands in, in trail sort.
     * Both are absent on most records and on every hit from a name index.
     */
    data class BusinessResult(
        val name: String,
        val point: GeoPoint,
        val score: Int,
        val brand: String? = null,
        val note: String? = null,
    )

    /**
     * A search's results and the reach they were taken from.
     *
     * The reach is the promise the panel is allowed to make. `beyondReach` is
     * how many more hits the same search already holds further out -- the
     * difference between "there is nothing there" and "there is more, further
     * away", which an empty list on its own could never tell.
     */
    data class SearchOutcome(
        val hits: List<BusinessResult>,
        val reachM: Double,
        val beyondReach: Int,
        /**
         * Which rung of [SEARCH_LADDER_M] this reach is, so a caller offering
         * "search farther" knows which one comes next. Past the last rung there
         * is no ceiling left and this is the ladder's size.
         */
        val rungIndex: Int = 0,
    )

    /** A park polygon near the user, with the distance from them to it. */
    data class NearbyPark(val name: String, val ring: List<GeoPoint>, val distanceM: Double)

    private fun layer(suffix: String, lat: Double, lon: Double): PtilesLayer? {
        val state = currentStateCode(lat, lon)
        return layerCandidates(manager.packsDir.listFiles().orEmpty(), suffix, state)
            .asSequence()
            .mapNotNull(::openCached)
            .firstOrNull { it.covers(lat, lon) }
    }

    private fun openCached(file: File): PtilesLayer? {
        val path = file.absolutePath
        layers[path]?.takeIf {
            it.length == file.length() && it.modified == file.lastModified()
        }?.let { return it.layer }
        return runCatching { PtilesLayer.open(path) }.getOrNull()?.also {
            layers[path] = CachedLayer(file.length(), file.lastModified(), it)
        }
    }

    fun nearbyRoadContext(lat: Double, lon: Double): Pair<RoadContext?, NearbyContext> {
        val road = runCatching { layer("roads", lat, lon)?.nearestRoad(lat, lon) }.getOrNull()
        return if (road == null) {
            null to NearbyContext(null, null, null)
        } else {
            RoadContext(road.roadClass, road.distanceM, null) to
                NearbyContext(road.name, road.roadClass, road.distanceM)
        }
    }

    /** Snap a route endpoint to the installed offline network before routing. */
    fun snapForRoute(point: GeoPoint, trail: Boolean): GeoPoint? = runCatching {
        if (trail) {
            layer("trails", point.lat, point.lon)?.nearestTrail(point.lat, point.lon, 1u)?.let { GeoPoint(it.snapped.lat, it.snapped.lon) }
                ?: layer("roads", point.lat, point.lon)?.nearestRoad(point.lat, point.lon)?.let { GeoPoint(it.snappedLat, it.snappedLon) }
                ?: nearestVertex(layer("trails", point.lat, point.lon)?.trails(point.lat, point.lon, 1u)?.flatMap { it.geometry }, point)
        } else {
            layer("roads", point.lat, point.lon)?.nearestRoad(point.lat, point.lon)?.let { GeoPoint(it.snappedLat, it.snappedLon) }
                ?: nearestVertex(layer("roads", point.lat, point.lon)?.roads(point.lat, point.lon, 1u)?.flatMap { it.geometry }, point)
        }
    }.getOrNull()

    private fun nearestVertex(points: List<LatLon>?, target: GeoPoint): GeoPoint? = points
        ?.minByOrNull { (it.lat - target.lat) * (it.lat - target.lat) + (it.lon - target.lon) * (it.lon - target.lon) }
        ?.let { GeoPoint(it.lat, it.lon) }

    /**
     * Every renderable feature around a coordinate.
     *
     * A single ring-1 query covers about 4 km, well short of the roughly 8 km
     * the default viewport shows, so the map ran out of data mid-screen and
     * immediately when panned. The FFI caps `ring` at 1 (`validate_ring` in
     * `ffi/src/lib.rs`), and passing 2 does not widen the query -- it throws,
     * which the `runCatching` blocks below turn into an empty map. So coverage
     * is widened by querying ring 1 at several sample centres and merging.
     *
     * `spread` now comes from the viewport itself (`MapDetail.fetchSpread`),
     * wide enough to cover the visible bounds plus two res-7 rings past every
     * edge. That is only affordable because each centre is cached: a fetch a
     * step to the north is one new row of centres, not a new screenful.
     */
    fun featuresAround(
        lat: Double,
        lon: Double,
        trails: Boolean,
        developer: Boolean = false,
        places: Boolean = false,
        spread: Int = DEFAULT_SPREAD,
        skipMinorRoads: Boolean = false,
    ): List<MapFeature> {
        val started = System.currentTimeMillis()
        val here = GeoPoint(lat, lon)
        val centers = (sampleCenters(lat, lon, spread) + leadCenters(lastFetchCenter, here, spread)).distinct()
        lastFetchCenter = here
        val flags = cacheFlags(developer, places, skipMinorRoads)
        // Kept per centre rather than merged, so the draw budget can be shared
        // between cells instead of handed to whichever cell happens to hold
        // the longest geometry. See [capAcrossCells].
        val cells = mutableListOf<List<MapFeature>>()
        var decodes = 0
        centers.forEach { center ->
            val key = CellKey(gridY(center.lat), gridX(center.lon), flags)
            synchronized(cellCache) { cellCache[key] }
                ?.let { cells += it; return@forEach }
            val decoded = decodeAround(center, developer, places, skipMinorRoads)
            decodes++
            synchronized(cellCache) { cellCache[key] = decoded }
            cells += decoded
        }
        // State and county lines from the admin pack itself. It carries 6,245
        // rings; the bbox is load-bearing, since the whole table is 611k
        // vertices and every one of them would cross the FFI.
        //
        // Its own cell, so jurisdiction lines are budgeted like anywhere else
        // rather than competing with the roads of whichever cell they fall in.
        runCatching {
            val reach = ADMIN_REACH_DEG
            // ponytail: cleared wholesale rather than aged. A day's driving
            // is a few dozen entries; anything smarter needs a reason.
            if (adminCache.size > MAX_CACHED_CELLS) adminCache.clear()
            cells += adminCache.getOrPut(gridY(lat) to gridX(lon)) {
                adminBoundaries(lat - reach, lon - reach, lat + reach, lon + reach)
            }
        }
        val out = cells.flatten()
        val capped = capAcrossCells(cells)
        // One line per fetch: how much of the grid the pan re-used, and what
        // the miss cost. It is the only view of whether the cache is working
        // on a device, and it is cheap enough to leave switched on.
        runCatching {
            android.util.Log.d(
                "PtilesRepo",
                "fetch spread=$spread centres=${centers.size} decoded=$decodes " +
                    "raw=${out.size} drawn=${capped.size} footprints=${capped.count { it.kind == "building_area" }} " +
                    "ms=${System.currentTimeMillis() - started}",
            )
        }
        return capped
    }

    /** Everything one sample centre decodes. Cached by [featuresAround]. */
    private fun decodeAround(
        center: GeoPoint,
        developer: Boolean,
        places: Boolean,
        skipMinorRoads: Boolean,
    ): List<MapFeature> {
        val out = mutableListOf<MapFeature>()
        val c = center
        runCatching {
            layer("roads", c.lat, c.lon)?.roads(c.lat, c.lon, RING)?.forEach { road ->
                // Pavement and parking aisles are 82.5% of the segments a
                // city cell decodes, and at a zoom that hides them every
                // one of those objects is allocated, marshalled across the
                // FFI, deduped, ranked, and then thrown away.
                if (skipMinorRoads && road.roadClass in MINOR_ROAD_CLASSES) return@forEach
                out += MapFeature(
                    road.geometry.map { GeoPoint(it.lat, it.lon) },
                    road.roadClass,
                    road.name,
                )
            }
        }
        runCatching {
            layer("water", c.lat, c.lon)?.water(c.lat, c.lon, RING)?.forEach { water ->
                if (water.geometry.size < 2) return@forEach
                // geom_type 0 is a polygon -- a lake, a reservoir, a wide
                // river's bank -- and wants filling, not outlining.
                val kind = if (water.geomType == 0.toUByte()) "water_area" else "water"
                out += MapFeature(water.geometry.map { GeoPoint(it.lat, it.lon) }, kind, water.name)
            }
        }
        runCatching {
            layer("parks", c.lat, c.lon)?.parks(c.lat, c.lon, RING)?.forEach { park ->
                if (park.geometry.size > 1) out += MapFeature(park.geometry.map { GeoPoint(it.lat, it.lon) }, "park", park.name)
            }
        }
        runCatching {
            layer("trails", c.lat, c.lon)?.trails(c.lat, c.lon, RING)?.forEach { trail ->
                val points = trail.geometry.map { GeoPoint(it.lat, it.lon) }
                if (points.isEmpty()) return@forEach
                if (trail.isTrailhead) {
                    out += MapFeature(listOf(points.first()), "trailhead", trail.name)
                    return@forEach
                }
                out += MapFeature(points, "trail:${trail.trailType}", trail.name)
                // Where a path starts and stops is the thing a walker
                // squints for; the line alone does not say.
                out += MapFeature(listOf(points.first()), "trail_end", trail.name)
                if (points.size > 1) out += MapFeature(listOf(points.last()), "trail_end", trail.name)
            }
        }
        if (places || developer) runCatching {
            layer("business", c.lat, c.lon)?.businessesNear(c.lat, c.lon, RING, 1_500.0)?.forEach { business ->
                if (isFlightNode(business.name)) return@forEach
                out += MapFeature(
                    listOf(GeoPoint(business.location.lat, business.location.lon)),
                    "business:${business.categoryIdx}",
                    business.name,
                )
            }
        }
        // Camera is a national layer, so the normal state-aware selection
        // falls through to US.camera.ptiles. It is rendered in both Drive
        // and Trail whenever installed, not hidden behind developer mode.
        runCatching {
            layer("camera", c.lat, c.lon)?.cameras(c.lat, c.lon, RING)?.forEach { camera ->
                out += cameraMapFeature(camera)
            }
        }
        if (developer) {
            runCatching {
                layer("rail", c.lat, c.lon)?.rail(c.lat, c.lon, RING)?.forEach { rail ->
                    if (rail.geometry.isNotEmpty()) {
                        out += MapFeature(
                            rail.geometry.map { GeoPoint(it.lat, it.lon) },
                            if (rail.geomType == 1.toUByte()) "station" else "rail:${rail.railType}",
                            rail.name,
                        )
                    }
                }
            }
        }
        // Every footprint in the cell, not the ones a probe grid landed in.
        // `buildings_at` answers one building per probe point, so the old
        // whole-viewport grid drew 25 buildings for a town; `buildings` is the
        // same ring query the other layers use and returns the block.
        runCatching {
            layer("buildings", c.lat, c.lon)?.buildings(c.lat, c.lon, RING)
                ?.forEach { building ->
                    val outline = building.geometry.map { GeoPoint(it.lat, it.lon) }
                    out += if (outline.size > 2) {
                        MapFeature(outline, "building_area", building.name)
                    } else {
                        MapFeature(listOf(GeoPoint(building.centroid.lat, building.centroid.lon)), "building", building.name)
                    }
                }
        }
        // A centre keeps only what it owns. Ring-1 patches overlap by about
        // three to one, so merging them whole meant 62,000 features where the
        // ground held 20,000, and the de-duplication that followed cost more
        // than the decode did. Owning by first vertex is exact: every feature
        // has one, so it lands in exactly one centre and nothing is drawn
        // twice or lost -- bar features whose owner is outside the fetch, which
        // is ground the fetch was not covering anyway.
        return out.filter { owns(center, it.points.first()) }
    }

    /**
     * Business search: what is near you that resembles what you typed.
     *
     * Two passes, because neither alone answers the question a driver asks.
     * The spatial layer gives everything within a few kilometres regardless of
     * spelling, which is where a hit almost always is. The state name index
     * (`{STATE}.business_name_index.ptiles`, bucketed by first letter, not by
     * geography -- so deliberately not routed through [layer]) reaches the rest
     * of the state for the times the answer is genuinely further out.
     *
     * Both are then scored on name similarity and pulled toward you by
     * distance, so a misspelling down the road beats a perfect match across
     * the state -- which is what the name index alone kept returning.
     */
    fun searchBusinesses(
        query: String,
        origin: GeoPoint? = null,
        limit: Int = SEARCH_LIMIT,
    ): List<BusinessResult>? {
        if (query.isBlank() || limit <= 0) return emptyList()
        val indexes = nameIndexFiles()
        // Every pack covering the point, not the first: a user standing eight
        // miles from a state line has half of what is near them in the other
        // state's pack, and a search that consulted one of the two called the
        // rest of the map empty.
        val nearLayers = origin?.let { covering("business", it.lat, it.lon) }.orEmpty()
        // Null, not empty: "nothing installed to search" and "nothing matched"
        // are different answers, and only one of them is the user's to fix.
        if (indexes.isEmpty() && nearLayers.isEmpty()) return null

        val nearby = if (origin == null) emptyList() else nearLayers.flatMap { near ->
            near.businessesNear(origin.lat, origin.lon, RING, SEARCH_NEARBY_RADIUS_M)
                .map { BusinessResult(it.name, GeoPoint(it.location.lat, it.location.lon), score = 0, brand = it.brand) }
        }
        val perIndex = (limit * SEARCH_OVERFETCH).coerceAtMost(SEARCH_MAX_FETCH)
        val statewide = indexes.flatMap { file ->
            queryVariants(query).flatMap { variant ->
                runCatching {
                    openCached(file)
                        ?.searchBusiness(variant, perIndex.toUInt())
                        ?.map { BusinessResult(it.name, GeoPoint(it.location.lat, it.location.lon), it.score.toInt()) }
                        .orEmpty()
                }.getOrDefault(emptyList())
            }
        }
        return rankByNameAndDistance(query, nearby + statewide, origin, limit)
    }

    /**
     * What is around you, nearest first -- the list an empty search box shows.
     *
     * Straight off the spatial business layer, so it needs no query and no
     * name index; the same flight-node junk is filtered out of it.
     */
    fun businessesNearby(
        origin: GeoPoint,
        radiusM: Double = NEARBY_RADIUS_M,
        limit: Int = SEARCH_LIMIT,
    ): List<BusinessResult>? {
        // Null when no business layer covers here -- a decode failure is a
        // different thing again, and is left to throw.
        val layers = covering("business", origin.lat, origin.lon)
        if (layers.isEmpty()) return null
        val hits = layers.flatMap { layer ->
            layer.businessesNear(origin.lat, origin.lon, RING, radiusM)
                .map { BusinessResult(it.name, GeoPoint(it.location.lat, it.location.lon), score = 0, brand = it.brand) }
        }
        return mergeBusinessHits(hits, limit, origin)
    }

    /**
     * Named trails and trailheads around a point, nearest first.
     *
     * Trail mode's answer to the business search: the trails layer is
     * geographic, so this is the same query with or without a name typed --
     * `query` only narrows what came back. Unnamed ways are dropped; a list of
     * "path" repeated forty times is not a destination picker.
     *
     * The point offered is the vertex nearest the origin, which for a long
     * trail is the end you would actually walk in from.
     */
    fun trailsNearby(
        origin: GeoPoint,
        query: String = "",
        limit: Int = SEARCH_LIMIT,
    ): List<BusinessResult>? {
        val needle = query.trim()
        if (needle.isNotEmpty()) return trailsMatching(origin, needle, limit)
        val centers = sampleCenters(origin.lat, origin.lon, DEFAULT_SPREAD)
        val packs = layersFor("trails")
        val queries = centers.flatMap { c -> packs.filter { it.covers(c.lat, c.lon) }.map { c to it } }
        if (queries.isEmpty()) return null
        val hits = queries.flatMap { (c, layer) ->
            layer.trails(c.lat, c.lon, RING)
        }.mapNotNull { trail ->
            val name = trail.name?.takeIf { it.isNotBlank() } ?: return@mapNotNull null
            val nearest = nearestVertex(trail.geometry, origin) ?: return@mapNotNull null
            // Trailheads first among equals: they are where you park.
            BusinessResult(name, nearest, score = if (trail.isTrailhead) 1 else 0)
        }
        // One row per trail, not one per stretch of it: the same path decodes
        // as several segments, and "Chirt Pit Road" twice is not two choices.
        val nearestPerTrail = hits
            .sortedBy { flatDistance2(origin, it.point) }
            .distinctBy { it.name.lowercase() }
        // Typed queries go through the same scoring businesses use: a trail is
        // named once on disk and never by an alternate (see `build_trails.py`,
        // which stores `tags["name"]` and nothing else), so "greenway" against
        // "Stones River Greenwy" is the only spelling forgiveness available.
        // The trailhead-first tiebreak is given up with it; distance already
        // orders what a typed query returns.
        return mergeBusinessHits(nearestPerTrail, limit, origin)
    }

    /**
     * Named trails anywhere in the installed packs, not merely within a sweep.
     *
     * A ring sweep is the wrong instrument past a few kilometres and there is
     * no trail name index to widen it with, so a typed query scans the trails
     * pack whole (`core::search_trails`). Measured on the published 2.9 MB
     * Tennessee pack -- 7,246 index entries, 906 merged blocks -- that is
     * 46-62 ms per query on a desktop and about a fifth of a second on the
     * emulator, once per query rather than once per fix.
     *
     * The FFI has already kept one row per name at the vertex nearest the
     * origin and ranked them; [rankByNameAndDistance] re-ranks with the
     * spelling forgiveness the FFI's exact/prefix/substring scoring does not
     * have, which is what finds "Stones River Greenwy".
     */
    fun trailsMatching(
        origin: GeoPoint,
        query: String,
        limit: Int = SEARCH_LIMIT,
    ): List<BusinessResult>? {
        val layers = layersFor("trails")
        if (layers.isEmpty()) return null
        // Overfetched, because the FFI ranks on exact/prefix/substring alone
        // and a misspelling only survives the re-rank if it was returned.
        val fetch = (limit * SEARCH_OVERFETCH).coerceAtMost(SEARCH_MAX_FETCH).toUInt()
        val hits = layers.flatMap { layer ->
            queryVariants(query).flatMap { variant ->
                runCatching {
                    layer.searchTrails(variant, origin.lat, origin.lon, fetch).map {
                        BusinessResult(
                            it.name,
                            GeoPoint(it.location.lat, it.location.lon),
                            score = if (it.isTrailhead) 1 else 0,
                        )
                    }
                }.getOrDefault(emptyList())
            }
        }
        return rankByNameAndDistance(query, hits, origin, limit)
    }

    /**
     * The named park polygons around a point, scored by how far away they are.
     *
     * Decoded over a sweep of sample centres because one ring-1 query reaches
     * about 2 km and a park boundary worth scoping to is routinely further out
     * than that. Rings from overlapping centres repeat, so the same park is
     * dropped by name and first vertex before it is measured.
     *
     * The sweep now reaches as far as the rule claims: [PARK_SCOPE_SPREAD]
     * covers [PARK_SCOPE_RADIUS_M], so a park 14 miles out is ranked low rather
     * than missing. That costs 225 ring queries instead of 25, which is why the
     * packs are resolved once instead of per centre and the answer is held per
     * grid cell -- a search runs on every keystroke, and standing still asks
     * the same question every time.
     */
    fun parksAround(origin: GeoPoint): List<NearbyPark> {
        // The rings are what the sweep costs, and they are the same for anyone
        // standing in this grid cell. Scoping them is arithmetic, and is
        // redone against the true origin every call.
        if (parkCache.size > MAX_CACHED_CELLS) parkCache.clear()
        val rings = parkCache.getOrPut(gridY(origin.lat) to gridX(origin.lon)) {
            val packs = layersFor("parks")
            sampleCenters(origin.lat, origin.lon, PARK_SCOPE_SPREAD).flatMap { c ->
                runCatching {
                    packs.filter { it.covers(c.lat, c.lon) }.flatMap { it.parks(c.lat, c.lon, RING) }
                }.getOrDefault(emptyList())
            }.mapNotNull { park ->
                val name = park.name?.takeIf { it.isNotBlank() } ?: return@mapNotNull null
                val ring = park.geometry.map { GeoPoint(it.lat, it.lon) }
                if (ring.size < 3) null else name to ring
            }.distinctBy { (name, ring) -> Triple(name, ring.first().lat, ring.first().lon) }
        }
        return parkScope(rings, origin)
    }

    /**
     * The journey picker's results, in the order the chosen sort wants them.
     *
     * Drive is the business search it has always been. Trail puts named trails
     * and trailheads first and then keeps only the businesses standing inside a
     * park that [parkScope] considers yours -- the walk-in-the-park case the
     * whole sort exists for, rather than every shop within the same radius.
     *
     * Null means no layer covers here, which is the one outcome downloading
     * fixes; an empty list means the layers answered and matched nothing.
     */
    fun journeyResults(
        origin: GeoPoint,
        query: String,
        trailSort: Boolean,
        limit: Int = SEARCH_LIMIT,
        fromRung: Int = 0,
    ): SearchOutcome? {
        val places = if (query.isBlank()) businessesNearby(origin, limit = limit)
        else searchBusinesses(query, origin, limit)
        val ranked = if (!trailSort) {
            places ?: return null
        } else {
            val trails = trailsNearby(origin, query, limit)
            if (trails == null && places == null) return null
            rankTrailFirst(trails.orEmpty(), places.orEmpty(), parksAround(origin), limit)
        }
        // A blank box is a browse of what is around you, and its reach is the
        // sweep's, not the ladder's. Widening only makes sense against a query.
        if (query.isBlank()) return SearchOutcome(ranked, NEARBY_RADIUS_M, 0)
        return widenToEnough(ranked, origin, fromRung)
    }

    /**
     * Where a stretch of a recording was, when the fixes prove it stayed there.
     *
     * A stop is named from the buildings and business layers: the footprint the
     * fixes sit in, then the business registered at that footprint, which is the
     * name a person would use ("Cherokee Marina") where the building's own OSM
     * name is usually missing or a street number.
     *
     * Null is the normal answer. A stretch that only passed through, or one over
     * ground with no installed business layer, has no honest label, and a wrong
     * one on a day's history is worse than none.
     *
     * Cached by rounded stop position rather than by segment, so the six
     * stretches a day spends at home cost one lookup.
     */
    suspend fun placeLabel(points: List<GeoPoint>, durationS: Long? = null): String? {
        val stop = stopCentroid(points, durationS) ?: return null
        val key = round5(stop.lat) to round5(stop.lon)
        placeCache[key]?.let { return it.ifEmpty { null } }
        val name = withContext(Dispatchers.IO) { lookupPlace(stop) }
        placeCache[key] = name.orEmpty()
        return name
    }

    private fun lookupPlace(at: GeoPoint): String? {
        val building = runCatching {
            layer("buildings", at.lat, at.lon)?.buildingsAt(listOf(LatLon(at.lat, at.lon)))?.firstOrNull()
        }.getOrNull()
        if (building != null) {
            // Standing inside the footprint earns a wider search from the
            // building's centre: a supermarket's registered point sits well away
            // from where anyone parks or walks in. Merely near one does not.
            val inside = containsPoint(building.geometry.map { GeoPoint(it.lat, it.lon) }, at)
            val radius = if (inside) INSIDE_PLACE_RADIUS_M else BESIDE_PLACE_RADIUS_M
            val centre = GeoPoint(building.centroid.lat, building.centroid.lon)
            businessAt(centre, radius)?.name?.takeIf { it.isNotBlank() && !isFlightNode(it) }
                ?.let { return it }
            building.name?.takeIf { it.isNotBlank() }?.let { return it }
        }
        return businessesNearby(at, BESIDE_PLACE_RADIUS_M, limit = 1)?.firstOrNull()?.name
    }

    /**
     * The trail nearest a tap, with the attributes the layer carries.
     *
     * Distance is to the nearest vertex, which for a long path is the bit of
     * it you actually pointed at.
     */
    fun trailAt(point: GeoPoint, radiusM: Double = BUSINESS_TAP_RADIUS_M): TrailInfo? = runCatching {
        layer("trails", point.lat, point.lon)
            ?.trails(point.lat, point.lon, RING)
            ?.mapNotNull { trail ->
                val nearest = trail.geometry.minByOrNull { v ->
                    flatDistance2(point, GeoPoint(v.lat, v.lon))
                } ?: return@mapNotNull null
                trail to flatDistance2(point, GeoPoint(nearest.lat, nearest.lon))
            }
            ?.filter { (_, d2) -> d2 <= radiusM * radiusM }
            ?.minByOrNull { (_, d2) -> d2 }
            ?.first
    }.getOrNull()

    /**
     * The business record nearest to a tap, with its extended attributes.
     *
     * The map draws businesses from [featuresAround], which keeps only name and
     * category. Phone, website, status, and provenance live on [BusinessInfo],
     * so the detail card re-reads the layer for the one record that was tapped.
     */
    fun businessAt(point: GeoPoint, radiusM: Double = BUSINESS_TAP_RADIUS_M): BusinessInfo? = runCatching {
        layer("business", point.lat, point.lon)
            ?.businessesNear(point.lat, point.lon, RING, radiusM)
            // The FFI already bounded the set by radius; ranking only needs a
            // monotone distance, so a flat-earth squared metre suffices.
            ?.minByOrNull { hit ->
                val dLat = (hit.location.lat - point.lat) * 111_320.0
                val dLon = (hit.location.lon - point.lon) * 111_320.0 * cos(Math.toRadians(point.lat))
                dLat * dLat + dLon * dLon
            }
    }.getOrNull()

    /**
     * A [Navigator] over a computed route, with its turns named where roads
     * could be decoded.
     *
     * Naming reads the roads around points sampled along the path. Every
     * sample is a decode, so they are spaced [NAME_SAMPLE_STEP] apart and
     * capped: a turn on an unnamed lane is normal, and the queue is still
     * correct without a name.
     */
    fun navigatorFor(path: List<GeoPoint>): Navigator? {
        if (path.size < 2) return null
        val step = (path.size / NAME_SAMPLE_MAX).coerceAtLeast(NAME_SAMPLE_STEP)
        val roads = path.filterIndexed { index, _ -> index % step == 0 }
            .flatMap { point ->
                runCatching { layer("roads", point.lat, point.lon)?.roads(point.lat, point.lon, RING).orEmpty() }
                    .getOrDefault(emptyList())
            }
            .distinctBy { it.osmId }
        return runCatching {
            Navigator(path.map { LatLon(it.lat, it.lon) }, roads, TURN_NAME_RADIUS_M)
        }.getOrNull()
    }

    /**
     * Every installed pack of one kind, opened, newest version of each first.
     *
     * [layer] answers "which pack covers this point" and is the right call for
     * a lookup. This is for the searches that mean to read whole packs, where
     * re-resolving per query centre would list the pack directory hundreds of
     * times over.
     */
    private fun layersFor(suffix: String): List<PtilesLayer> =
        layerCandidates(manager.packsDir.listFiles().orEmpty(), suffix, stateHint)
            .mapNotNull(::openCached)

    /**
     * Every installed pack of one kind that covers a point.
     *
     * [layer] stops at the first, which is the right answer for "which road am
     * I on" and the wrong one for a search: state packs meet at a line the user
     * has no reason to know about, and near it half of what is nearby lives in
     * the neighbour's pack. Hits from two packs at the same place are one place,
     * and the searches dedupe on name and position before showing anything.
     */
    private fun covering(suffix: String, lat: Double, lon: Double): List<PtilesLayer> =
        layersFor(suffix).filter { it.covers(lat, lon) }

    private fun nameIndexFiles(): List<File> = manager.packsDir.listFiles()
        .orEmpty()
        .filter { it.isFile && it.name.endsWith(".business_name_index.ptiles", ignoreCase = true) }
        .sortedBy { it.name }

    /**
     * One leg, split in half as many times as the corridor cap demands.
     *
     * The FFI bounds a route's corridor at 512 H3 res-7 cells, and fails two
     * ways once a trip outgrows it: "bad bounding box" when the box itself is
     * over the cap, and "Disconnected" when it fits but the road joining the
     * ends arcs outside it. Both are facts about the corridor, not about the
     * trip being impossible, and halving fixes both -- each half's box hugs
     * the line more tightly, so the missing link falls inside it.
     *
     * Measured on the Tennessee roads pack: Savannah to Camden is
     * Disconnected whole, and routes as 70.9 km + 53.1 km split in half.
     *
     * ponytail: the split point is the geometric midpoint snapped to the
     * network, not a real waypoint. It can put a joint somewhere a router
     * would not have chosen. Good enough until someone drives one and says
     * otherwise.
     */
    fun offlineRoute(
        start: GeoPoint,
        end: GeoPoint,
        trail: Boolean,
        avoidHighways: Boolean,
        avoidIntersections: Boolean,
        splitsLeft: Int = MAX_ROUTE_SPLITS,
    ): RouteResult = try {
        routeLeg(start, end, trail, avoidHighways, avoidIntersections)
    } catch (e: Exception) {
        if (splitsLeft <= 0 || !isSplittableFailure(e)) throw e
        val midpoint = GeoPoint((start.lat + end.lat) / 2, (start.lon + end.lon) / 2)
        val split = snapForRoute(midpoint, trail) ?: midpoint
        joinLegs(
            listOf(
                offlineRoute(start, split, trail, avoidHighways, avoidIntersections, splitsLeft - 1),
                offlineRoute(split, end, trail, avoidHighways, avoidIntersections, splitsLeft - 1),
            )
        )
    }

    private fun routeLeg(
        start: GeoPoint,
        end: GeoPoint,
        trail: Boolean,
        avoidHighways: Boolean,
        avoidIntersections: Boolean,
    ): RouteResult {
        val stack = PtilesStack.withLayers(
            roads = layer("roads", start.lat, start.lon),
            buildings = layer("buildings", start.lat, start.lon),
            business = layer("business", start.lat, start.lon),
            trails = layer("trails", start.lat, start.lon),
            parks = layer("parks", start.lat, start.lon),
            water = layer("water", start.lat, start.lon),
            camera = layer("camera", start.lat, start.lon),
            addresses = null,
        )
        val route = stack.offlineRoute(
            start.lat, start.lon, end.lat, end.lon,
            if (trail) OfflineRouteMode.FOOT else OfflineRouteMode.DRIVING,
            avoidHighways,
            avoidIntersections,
            // Zero keeps the profile's own snap radius, which is what this
            // app has always routed with.
            0.0,
        )
        check(route.path.isNotEmpty()) { "No roads or trails in the downloaded maps here -- download this area in Offline maps" }
        return RouteResult(
            route.path.map { GeoPoint(it.lat, it.lon) },
            route.distanceM,
            route.durationS,
            route.decodedSegments.toInt(),
        )
    }

    /**
     * A multi-stop route, built by chaining single-leg [offlineRoute] calls.
     *
     * `PtilesStack.offline_route` takes exactly one start and one end, so a
     * waypoint route is N independent graph builds and costs roughly N times a
     * single route. `onLegDone(completed, total)` reports that progress so the
     * caller can show a real percentage instead of an indeterminate spinner.
     */
    fun offlineRouteVia(
        start: GeoPoint,
        waypoints: List<GeoPoint>,
        end: GeoPoint,
        trail: Boolean,
        avoidHighways: Boolean,
        avoidIntersections: Boolean,
        onLegDone: (Int, Int) -> Unit = { _, _ -> },
    ): RouteResult {
        val legs = routeLegs(start, waypoints, end)
        val results = legs.mapIndexed { index, (from, to) ->
            val snappedFrom = snapForRoute(from, trail) ?: from
            val snappedTo = snapForRoute(to, trail) ?: to
            offlineRoute(snappedFrom, snappedTo, trail, avoidHighways, avoidIntersections)
                .also { onLegDone(index + 1, legs.size) }
        }
        return joinLegs(results)
    }

    /**
     * Administrative rings overlapping a bounding box, as map features.
     *
     * Rings are closed on disk (first vertex repeated), so they are drawn as
     * polylines rather than filled: an administrative area has no colour, only
     * an edge.
     */
    fun adminBoundaries(
        minLat: Double,
        minLon: Double,
        maxLat: Double,
        maxLon: Double,
    ): List<MapFeature> = runCatching {
        openAdminLayer()
            ?.polygonsIn(minLat, minLon, maxLat, maxLon)
            ?.map { polygon ->
                MapFeature(
                    polygon.geometry.map { GeoPoint(it.lat, it.lon) },
                    // The level is optional now: a ring the pack cannot
                    // place exactly is reported unknown rather than
                    // misfiled as a state.
                    if (polygon.adminLevel == 4.toUByte()) "admin_state" else "admin_county",
                    polygon.name,
                )
            }
            .orEmpty()
    }.getOrDefault(emptyList())

    private fun openAdminLayer(): AdminLayer? {
        adminLayer?.let { return it }
        // Highest version wins, the same rule layerCandidates applies to state
        // packs: an older admin file left behind by a previous install must not
        // shadow the one that knows about boundary straddles.
        return newestAdminPack(manager.packsDir.listFiles().orEmpty().toList())
            ?.let { runCatching { AdminLayer.open(it.absolutePath) }.getOrNull() }
            ?.also { adminLayer = it }
    }

    fun installedLayers(): List<File> = manager.packs().flatMap { it.layers }

    /** True when a real installed roads pack covers the current coordinate. */
    fun mapsReadyAt(lat: Double, lon: Double): Boolean = layer("roads", lat, lon) != null

    /**
     * Which state a coordinate is in.
     *
     * `US.admin.ptiles` when it is installed, then the boundaries baked into
     * the APK, and only then [StateResolver]'s bounding boxes -- which overlap
     * at every border and were picking the wrong state on the strength of a
     * box centre.
     */
    fun currentStateCode(lat: Double, lon: Double): String? {
        val resolved = openAdminLayer()
            ?.let { runCatching { it.adminAt(lat, lon)?.state }.getOrNull() }
            ?.let(StateResolver::codeForName)
            ?: StateBoundaries.stateAt(appContext, lat, lon)
            ?: StateResolver.stateAt(lat, lon, stateHint)
        if (resolved != null) stateHint = resolved
        return resolved
    }

    companion object {
        private val VERSIONED_STEM = Regex("^(.+)_v(\\d+)$")

        /**
         * The only ring the FFI accepts. `validate_ring` in `ffi/src/lib.rs`
         * rejects anything above 1, so widening coverage means more query
         * centres, not a bigger ring.
         */
        internal val RING: UByte = 1u

        /** Extra sample centres in each direction. See [featuresAround]. */
        internal const val DEFAULT_SPREAD = 1

        /**
         * Sample centres held decoded at once.
         *
         * The widest fetch is 9x9 centres plus a lead row, so this holds one
         * full viewport and the ground a pan is about to cross. Past that the
         * eldest goes, which is the one furthest behind the pan.
         */
        internal const val MAX_CACHED_CELLS = 128

        /** True when a point falls in the box a sample centre owns. */
        internal fun owns(center: GeoPoint, point: GeoPoint): Boolean =
            gridY(point.lat) == gridY(center.lat) && gridX(point.lon) == gridX(center.lon)

        internal fun gridY(lat: Double): Int = Math.round(lat / SAMPLE_STEP_LAT).toInt()

        internal fun gridX(lon: Double): Int = Math.round(lon / SAMPLE_STEP_LON).toInt()

        /** Which decode a cached cell came from; a cell decoded without the
         * business layer is not the same answer as one decoded with it. */
        internal fun cacheFlags(developer: Boolean, places: Boolean, skipMinorRoads: Boolean): Int =
            (if (developer) 1 else 0) or (if (places) 2 else 0) or (if (skipMinorRoads) 4 else 0)

        /**
         * One extra row or column of centres ahead of the pan.
         *
         * The debounce means a fetch starts only once the viewport settles, so
         * a fast pan is always chasing its data. Rather than decode more often,
         * the fetch reaches further the way the user is already going: by the
         * time the next one runs, its leading edge is a cache hit.
         */
        internal fun leadCenters(previous: GeoPoint?, now: GeoPoint, spread: Int): List<GeoPoint> {
            if (previous == null) return emptyList()
            val dy = Integer.signum(gridY(now.lat) - gridY(previous.lat))
            val dx = Integer.signum(gridX(now.lon) - gridX(previous.lon))
            if (dy == 0 && dx == 0) return emptyList()
            val y0 = gridY(now.lat)
            val x0 = gridX(now.lon)
            val out = mutableListOf<GeoPoint>()
            if (dy != 0) {
                for (x in -spread..spread) out += gridPoint(y0 + dy * (spread + 1), x0 + x)
            }
            if (dx != 0) {
                for (y in -spread..spread) out += gridPoint(y0 + y, x0 + dx * (spread + 1))
            }
            return out
        }

        internal fun gridPoint(y: Int, x: Int): GeoPoint =
            GeoPoint(y * SAMPLE_STEP_LAT, x * SAMPLE_STEP_LON)

        /**
         * Spacing between sample centres, roughly one res-7 cell.
         *
         * A res-7 cell is about 1.4 km across; these are the degree equivalents
         * at mid-latitudes, close enough for query placement -- overlap is
         * deduped, and only a gap would show, as blank paper.
         */
        internal const val SAMPLE_STEP_LAT = 0.030
        internal const val SAMPLE_STEP_LON = 0.037

        internal const val SEARCH_LIMIT = 20

        /** Hits pulled per index before the distance sort trims them. */
        internal const val SEARCH_OVERFETCH = 5
        internal const val SEARCH_MAX_FETCH = 200

        /**
         * How many times a leg may be halved before the corridor cap is
         * accepted as real. Eight legs covers a cross-state drive; past that
         * the pack almost certainly does not hold the road anyway.
         */
        internal const val MAX_ROUTE_SPLITS = 3

        /**
         * True for the two corridor failures splitting fixes, false for a
         * missing layer, an unsnappable endpoint, or an empty graph -- none of
         * which a smaller corridor helps.
         *
         * Widening the corridor instead was tried and measured: on a
         * disconnected route the box is already at the cell cap, so there is
         * usually no room to widen, and where there was room the extra
         * segments did not bridge the gap.
         */
        internal fun isSplittableFailure(error: Throwable): Boolean {
            val message = generateSequence(error, Throwable::cause)
                .mapNotNull { it.message }
                .joinToString(" ")
                .lowercase()
            val overBudget = "bounding box" in message && ("too large" in message || "cells" in message)
            return overBudget || "disconnected" in message
        }

        /** How far a typed search sweeps the spatial layer before the index. */
        internal const val SEARCH_NEARBY_RADIUS_M = 8_000.0

        /**
         * The radii a search widens through, in metres: 25, 100, 400 and
         * 1,000 miles.
         *
         * The data behind a typed query is not swept -- the business name index
         * is statewide and the trail scan reads its pack whole -- so widening
         * is a matter of how far out results are worth showing, not of asking
         * again. Each rung is checked in turn and the first with
         * [ENOUGH_RESULTS] wins; failing that the ceiling does, and anything
         * past the ceiling waits for the user to ask for it.
         */
        internal val SEARCH_LADDER_M = listOf(40_234.0, 160_934.0, 643_738.0, 1_609_344.0)

        /** Hits inside a rung that make it worth stopping at. */
        internal const val ENOUGH_RESULTS = 5

        /** Below this, a name is not a match however lenient the scoring. */
        internal const val MIN_NAME_SIMILARITY = 0.55

        /** Distance at which the pull toward the user stops growing. */
        internal const val DISTANCE_FALLOFF_KM = 40.0

        /** Similarity points the full falloff distance costs a hit. */
        internal const val DISTANCE_WEIGHT = 0.45

        /** Similarity points each tenfold past the falloff costs a hit. */
        internal const val FAR_WEIGHT = 0.25

        /**
         * How far around the viewport administrative rings are fetched.
         *
         * Wide enough that a line already on screen survives a pan, narrow
         * enough that the ring scan stays trivial.
         */
        internal const val ADMIN_REACH_DEG = 0.6

        /** How far the default nearby list reaches. */
        internal const val NEARBY_RADIUS_M = 2_000.0

        /** Turn naming: how far a road may be from a manoeuvre to name it. */
        internal const val TURN_NAME_RADIUS_M = 30.0

        /** Path vertices between road decodes when naming turns, and a cap. */
        internal const val NAME_SAMPLE_STEP = 8
        internal const val NAME_SAMPLE_MAX = 40

        /**
         * Ceiling on features handed to the renderer in one viewport.
         *
         * Every polyline is its own `drawPath` stroke. Five ring-1 queries over
         * central Nashville return enough of them to block the render thread
         * past the 5 s input timeout -- an ANR whose main-thread stack sits in
         * `syncAndDrawFrame`, with no app code on it. Dense cities are the
         * normal case, not the edge one.
         *
         * ponytail: a flat cap with a length-based priority. The real fix is
         * per-zoom road-class filtering, worth building when someone complains
         * about a missing side street rather than before.
         */
        internal const val MAX_DRAWN_FEATURES = 3_000

        /**
         * Ceiling on footprints, which have their own budget. Two paths for
         * the layer whatever its size, so the cost left is projecting the
         * vertices -- which is why this is six thousand and not three hundred.
         */
        internal const val MAX_DRAWN_FOOTPRINTS = 6_000

        /**
         * The newest installed admin pack.
         *
         * Same rule [layerCandidates] applies to state packs: an unversioned
         * file left behind by an earlier install must not shadow the one that
         * knows which cells straddle a border.
         */
        internal fun newestAdminPack(files: List<File>): File? = files
            .filter { it.name.startsWith("US.admin") && it.name.endsWith(".ptiles") }
            .maxByOrNull { file ->
                VERSIONED_STEM.matchEntire(file.name.removeSuffix(".ptiles").removePrefix("US."))
                    ?.groupValues?.get(2)?.toIntOrNull() ?: 1
            }

        /** Road classes not worth carrying when the map will not draw them. */
        internal val MINOR_ROAD_CLASSES = setOf("footway", "service", "steps", "sidewalk", "path")

        /**
         * Draw priority when a viewport exceeds [MAX_DRAWN_FEATURES].
         *
         * Ranked by kind, not by vertex count: park and water polygons carry
         * far more points than a street does, so a purely length-based cap
         * kept the scenery and deleted the road network -- exactly backwards
         * for a map you navigate by.
         */
        internal fun featureRank(kind: String): Int = when {
            kind in setOf("motorway", "trunk", "primary", "secondary") -> 0
            // Every other road, ahead of trails. Ranking trails higher meant a
            // town's footways -- one per street, and there is a footway beside
            // most streets -- filled the cap and evicted the street grid they
            // run alongside, leaving highways floating on blank paper.
            kind in setOf("tertiary", "residential", "unclassified", "service", "living_street", "road") -> 1
            kind.startsWith("trail") || kind in setOf("path", "footway", "track", "steps") -> 3
            kind == "water_area" || kind == "water" -> 2
            kind == "park" -> 4
            kind == "building_area" || kind == "building" ||
                kind.startsWith("camera") || kind.startsWith("business") -> 5
            else -> 1 // anything else the roads layer classified
        }

        /**
         * Keep the most useful features when over the cap, without starving a
         * layer.
         *
         * A single global ranking looked right and was not: a city viewport
         * decodes thousands of roads, so ranking roads above trails meant a
         * trail screen in town got zero trails -- the layer was queried,
         * decoded, and then entirely evicted before it reached the canvas.
         * Each group now gets a share of the budget and only gives up what it
         * does not use.
         */
        internal fun capFeatures(
            features: List<MapFeature>,
            max: Int = MAX_DRAWN_FEATURES,
        ): List<MapFeature> {
            // Footprints are budgeted apart from everything else because they
            // no longer cost like everything else: the renderer draws the whole
            // layer in two paths, so the price of one more is a projection, not
            // a stroke. Inside the shared budget they took a tenth of it and a
            // town came out with three hundred buildings.
            val footprints = features.filter { it.kind == "building_area" }
            if (footprints.isNotEmpty()) {
                val rest = capFeatures(features.filter { it.kind != "building_area" }, max)
                return rest + footprints
                    .sortedByDescending { it.points.size }
                    .take(MAX_DRAWN_FOOTPRINTS)
            }
            if (features.size <= max) return features
            // Indices, not the features themselves: two stretches of the same
            // street decode to equal values, and a set would collapse them.
            val ranked = features.indices.sortedWith(
                compareBy<Int> { featureRank(features[it].kind) }
                    .thenByDescending { features[it].points.size }
            )
            val quota = GROUP_SHARE.mapValues { (_, share) -> (max * share).toInt() }.toMutableMap()
            val kept = mutableListOf<Int>()
            val spare = mutableListOf<Int>()
            ranked.forEach { index ->
                val group = featureRank(features[index].kind)
                val left = quota[group] ?: 0
                if (left > 0) {
                    quota[group] = left - 1
                    kept += index
                } else {
                    spare += index
                }
            }
            // Unused budget goes back to whoever still had features to draw.
            val out = kept + spare.take((max - kept.size).coerceAtLeast(0))
            return out.sorted().map(features::get)
        }

        /**
         * Share the draw budget between sample cells before sharing it between
         * layers.
         *
         * [capFeatures] ranks by kind and then by vertex count across the whole
         * fetch, which is spatially blind: with 49 cells decoding 17,000
         * features into a 3,000 budget, the survivors are simply the longest
         * geometry anywhere in the grid. Cells whose features are short lose
         * everything, so one H3 tile draws and the one beside it is blank
         * paper -- and a pan that brings in longer geometry evicts a cell that
         * was on screen a moment ago. The tile did not fail to load; it was
         * decoded, cached, and then outbid.
         *
         * Smallest cell first, each taking an equal share of what is left, so
         * a cell that cannot use its share hands the remainder to the dense
         * ones. Downtown still draws more than farmland, but nothing draws
         * nothing. Footprints keep their own budget: two paths for the layer
         * however many there are, so they do not compete for strokes.
         */
        internal fun capAcrossCells(
            cells: List<List<MapFeature>>,
            max: Int = MAX_DRAWN_FEATURES,
        ): List<MapFeature> {
            val footprints = cells.asSequence().flatten()
                .filter { it.kind == "building_area" }
                .sortedByDescending { it.points.size }
                .take(MAX_DRAWN_FOOTPRINTS)
                .toList()
            val rest = cells
                .map { cell -> cell.filter { it.kind != "building_area" } }
                .filter { it.isNotEmpty() }
            if (rest.sumOf { it.size } <= max) return rest.flatten() + footprints
            val out = mutableListOf<MapFeature>()
            var remaining = max
            rest.sortedBy { it.size }.forEachIndexed { index, cell ->
                val kept = capFeatures(cell, remaining / (rest.size - index))
                out += kept
                remaining -= kept.size
            }
            return out + footprints
        }

        /**
         * How much of the draw budget each rank may claim before the rest is
         * shared out. Roads dominate because they are what a map is for.
         */
        internal val GROUP_SHARE = mapOf(
            0 to 0.20, // motorways and trunks
            1 to 0.40, // every other road
            2 to 0.10, // water
            3 to 0.15, // trails
            4 to 0.05, // parks
            5 to 0.10, // buildings, pins
        )

        /** How close a tap must land to count as hitting a business pin. */
        internal const val BUSINESS_TAP_RADIUS_M = 60.0

        /**
         * What makes a run of fixes a stop rather than a slow stretch of road.
         *
         * Five fixes because fewer cannot distinguish a stop from GPS noise on
         * a moving track; 60 m because a phone parked in one spot still wanders
         * tens of metres, while a car crawling through a drive-through covers
         * far more than that; two minutes because everything shorter -- a light,
         * a queue, a stop sign -- is traffic, not a visit.
         *
         * Duration is optional: a file mid-write may have no `<time>` yet, and
         * the fix count still bounds how brief a stop can be.
         */
        internal const val MIN_STOP_FIXES = 5
        internal const val STOP_SPREAD_M = 60.0
        internal const val MIN_STOP_SECONDS = 120L

        /** Search radius from a footprint centre when the stop is inside it. */
        internal const val INSIDE_PLACE_RADIUS_M = 120.0

        /** And when it is merely beside one, or there is no footprint at all. */
        internal const val BESIDE_PLACE_RADIUS_M = 50.0

        /**
         * The middle of a stop, or null when these fixes are not one.
         *
         * Spread is measured from the mean rather than end to end: a track that
         * leaves and returns has a small end-to-end distance and is still a
         * journey.
         */
        internal fun stopCentroid(points: List<GeoPoint>, durationS: Long? = null): GeoPoint? {
            if (points.size < MIN_STOP_FIXES) return null
            if (durationS != null && durationS < MIN_STOP_SECONDS) return null
            val centre = GeoPoint(points.sumOf { it.lat } / points.size, points.sumOf { it.lon } / points.size)
            val worst = points.maxOf { flatDistance2(centre, it) }
            return if (worst <= STOP_SPREAD_M * STOP_SPREAD_M) centre else null
        }

        /**
         * Whether a point falls inside a closed ring, by ray casting.
         *
         * The FFI already picks the building containing a point, but not
         * whether it contained it or merely had a centroid within 50 m, and
         * those deserve different confidence. Park scoping asks the same
         * question of a much bigger ring. Degrees are compared directly: the
         * test is a parity count, and scaling longitude scales both sides of
         * every comparison equally.
         *
         * A ring is treated as simple: a park with an inholding punched out of
         * it reads as solid, because the layer hands back one ring per feature
         * and says nothing about which are holes.
         */
        internal fun containsPoint(ring: List<GeoPoint>, point: GeoPoint): Boolean {
            if (ring.size < 3) return false
            var inside = false
            var j = ring.size - 1
            for (i in ring.indices) {
                val a = ring[i]
                val b = ring[j]
                if ((a.lat > point.lat) != (b.lat > point.lat)) {
                    val x = (b.lon - a.lon) * (point.lat - a.lat) / (b.lat - a.lat) + a.lon
                    if (point.lon < x) inside = !inside
                }
                j = i
            }
            return inside
        }

        /**
         * How wide the park sweep reaches, in sample-centre steps.
         *
         * Two steps is about 7 km, which covers the 8 km a typed business
         * search sweeps and costs 25 ring queries. See [parksAround].
         */
        internal const val PARK_SCOPE_SPREAD = 2

        /** The 15 miles the product asks for, in metres. */
        internal const val PARK_SCOPE_RADIUS_M = 24_140.0

        /**
         * Which parks count as "the park you are in or near".
         *
         * Measured to the polygon, never to the centroid. A centroid answers a
         * question about a park's middle, and the parks worth scoping to are
         * the ones whose middle is nowhere near you: stand at a trailhead on
         * the edge of a national park and its centroid is twenty miles off,
         * while a pocket park across the street wins on a measure that has
         * nothing to do with where either park's edge is.
         *
         * So: inside beats near, and near is a straight radius.
         *
         * - Standing inside any park, that park is the scope and nothing else
         *   is. Distance is zero there, so a large park does not lose to a
         *   small neighbour, and a walker in a national park is not offered the
         *   town's businesses.
         * - Otherwise every park within [PARK_SCOPE_RADIUS_M] of its boundary
         *   is in scope, nearest first. This is deliberately one rule for
         *   national and regional parks alike: a small county park two miles
         *   away is in scope because it is two miles away, and a national park
         *   twenty miles away is out for the same reason. Park size is not
         *   consulted, because the layer does not classify parks reliably
         *   enough to make it mean anything, and boundary distance already
         *   captures what "closest" was reaching for.
         *
         * Distance is to ring vertices rather than to the segments between
         * them, so a boundary drawn as a long straight run reads as further
         * away than it is, by up to half the vertex spacing. OSM park rings are
         * dense; a heavily simplified one is the case to watch.
         */
        internal fun parkScope(
            parks: List<Pair<String, List<GeoPoint>>>,
            origin: GeoPoint,
            radiusM: Double = PARK_SCOPE_RADIUS_M,
        ): List<NearbyPark> {
            val measured = parks.map { (name, ring) -> NearbyPark(name, ring, ringDistanceM(ring, origin)) }
            val inside = measured.filter { it.distanceM == 0.0 }
            if (inside.isNotEmpty()) return inside
            return measured.filter { it.distanceM <= radiusM }.sortedBy { it.distanceM }
        }

        /** Metres from a point to a ring: zero inside it, else the nearest vertex. */
        internal fun ringDistanceM(ring: List<GeoPoint>, point: GeoPoint): Double {
            if (ring.isEmpty()) return Double.MAX_VALUE
            if (containsPoint(ring, point)) return 0.0
            return kotlin.math.sqrt(ring.minOf { flatDistance2(point, it) })
        }

        /** The scoped park a point stands in, when it stands in one. */
        internal fun parkContaining(scope: List<NearbyPark>, point: GeoPoint): String? =
            scope.firstOrNull { containsPoint(it.ring, point) }?.name

        /**
         * Trail sort's order: paths first, then places inside a scoped park.
         *
         * Trails keep the order [trailsNearby] gave them -- trailheads ahead of
         * plain paths on a blank query, resemblance on a typed one. Businesses
         * follow in the order the business search ranked them, minus every one
         * standing outside the parks [parkScope] chose, and each labelled with
         * the park it is in so a row says why it is on a trail list at all.
         */
        internal fun rankTrailFirst(
            trails: List<BusinessResult>,
            businesses: List<BusinessResult>,
            scope: List<NearbyPark>,
            limit: Int,
        ): List<BusinessResult> {
            val inPark = businesses.mapNotNull { hit ->
                parkContaining(scope, hit.point)?.let { hit.copy(note = "in $it") }
            }
            return (trails + inPark)
                .distinctBy { Triple(it.name, round5(it.point.lat), round5(it.point.lon)) }
                .take(limit)
        }

        /**
         * Query centres for a viewport: the grid node nearest the coordinate
         * plus `spread` steps out along each axis.
         *
         * A grid, not a plus. The arms of a plus are a step apart and each ring
         * reaches about 2 km, so the corners of the viewport sat 2.7 km from
         * any sample centre and came back empty -- which is why the map read as
         * one tile of detail with blank paper around it.
         *
         * Centres are snapped to a fixed global grid rather than hung off the
         * coordinate, so the same ground always has the same centre and a pan
         * re-uses what the last fetch decoded. Unsnapped, every fetch invented
         * a fresh set of centres and nothing was ever a second time.
         */
        internal fun sampleCenters(lat: Double, lon: Double, spread: Int): List<GeoPoint> {
            val y0 = gridY(lat)
            val x0 = gridX(lon)
            if (spread <= 0) return listOf(gridPoint(y0, x0))
            // Centre first: it is where the user is, and it is the one cell
            // whose absence would be noticed immediately.
            val out = mutableListOf(gridPoint(y0, x0))
            for (y in -spread..spread) {
                for (x in -spread..spread) {
                    if (x == 0 && y == 0) continue
                    out += gridPoint(y0 + y, x0 + x)
                }
            }
            return out
        }

        /**
         * Rank and de-duplicate hits gathered from several state indexes.
         *
         * Score is the FFI's match quality (2 exact, 1 prefix, 0 substring),
         * so it sorts descending and name breaks ties. Two states can hold the
         * same chain at the same spot only when their bounding boxes overlap at
         * a border, so identity is name plus coordinate rounded to 5 decimal
         * places -- about a metre, well under the spacing of two real stores.
         */
        /**
         * Flight numbers that ride along in the business layer.
         *
         * OSM aeroway import leaves nodes named `AA 1234`, `DL2201`, `UA 45`:
         * a gate's departure, not a place. They are never what a search for a
         * business meant, and they crowd out real hits near an airport.
         */
        private val FLIGHT_NODE = Regex("""^[A-Z]{2,3}\s?\d{1,4}[A-Z]?$""")

        internal fun isFlightNode(name: String): Boolean = FLIGHT_NODE.matches(name.trim())

        /**
         * How much a name looks like what was typed, 0 (not at all) to 1.
         *
         * Exact beats prefix beats substring beats a word that merely starts
         * the same; below that it is edit distance, so "wafle huse" still
         * finds Waffle House. Anything under [MIN_NAME_SIMILARITY] is not a
         * match at all -- fuzzy without a floor returns the whole layer.
         */
        /**
         * Spellings of the same word, collapsed to one token.
         *
         * American place and trail names are written both ways and searched for
         * either -- "St Marys" for "Saint Mary's", "Mt Leconte" for "Mount
         * LeConte", "Cedar Crk" for "Cedar Creek". Each pair maps to a single
         * token so both sides of a comparison land on it.
         *
         * `st` is deliberately both Saint and Street. They collapse together,
         * which makes "Main Street" and "Main St" the same and "Saint Marys"
         * and "St Marys" the same; the cost is that "Main Saint" would match
         * "Main Street", and no such place is being searched for.
         */
        private val NAME_ALIASES = mapOf(
            "saint" to "st", "street" to "st",
            "mount" to "mt", "mountain" to "mtn",
            "fort" to "ft", "creek" to "cr", "crk" to "cr",
            "north" to "n", "south" to "s", "east" to "e", "west" to "w",
            "road" to "rd", "drive" to "dr", "lane" to "ln", "avenue" to "ave",
            "trail" to "trl", "highway" to "hwy",
            "parkway" to "pkwy", "junction" to "jct", "national" to "natl",
        )

        /**
         * A name reduced to what a person means by it: lower case, `&` spelt
         * out, punctuation gone, and every word in [NAME_ALIASES] on its
         * canonical spelling.
         *
         * Query-side only, and applied to both strings of a comparison, so it
         * needs nothing rebuilt. The name-index sidecar's buckets are keyed on
         * `core::business_search::fold_name`'s rule and a pack already on the
         * phone cannot be re-bucketed; what this changes is which of the hits
         * that come back rank as matches, never which bucket is opened.
         */
        internal fun normalizeName(value: String): String = value
            .lowercase()
            .replace("&", " and ")
            .map { if (it.isLetterOrDigit()) it else if (it == '\'' || it == '’') "" else " " }
            .joinToString("")
            .split(' ')
            .filter { it.isNotEmpty() }
            .joinToString(" ") { NAME_ALIASES[it] ?: it }

        /**
         * The spellings of a query worth asking the layers for.
         *
         * Scoring forgives spelling, but only among rows that came back at all,
         * and both far-reaching paths match literally: the name index is a
         * bucketed substring match and the trail scan a substring match over
         * the whole layer. So "St Marys" has to be asked as "Saint Marys" too,
         * or the row is never in the list to be scored. Two probes at most --
         * each one is a block fetch, or on trails a scan.
         */
        internal fun queryVariants(query: String): List<String> {
            val trimmed = query.trim()
            if (trimmed.isEmpty()) return emptyList()
            val expanded = trimmed.split(' ').joinToString(" ") { word ->
                NAME_ALIASES.entries.firstOrNull { (long, short) ->
                    word.equals(short, ignoreCase = true) && long.length > short.length
                }?.key ?: word
            }
            return if (expanded.equals(trimmed, ignoreCase = true)) listOf(trimmed)
            else listOf(trimmed, expanded)
        }

        internal fun nameSimilarity(query: String, name: String): Double {
            val q = normalizeName(query)
            val n = normalizeName(name)
            if (q.isEmpty() || n.isEmpty()) return 0.0
            if (q == n) return 1.0
            if (n.startsWith(q)) return 0.92
            if (n.contains(q)) return 0.85
            val words = n.split(' ', '-', '/', ',').filter { it.isNotEmpty() }
            if (words.any { it.startsWith(q) }) return 0.8
            // Typos: the whole string, then the single best word, since
            // "huse" against "waffle house" is mostly a distance to `waffle`.
            val whole = editRatio(q, n)
            val best = words.maxOfOrNull { editRatio(q, it) } ?: 0.0
            return maxOf(whole, best * 0.95)
        }

        /**
         * The better of what was typed against the name and against the brand.
         *
         * A chain's records are named for the site -- "Shell Oil 41762",
         * "Kroger Fuel Center #705" -- and searched for by the chain. The brand
         * is the only alternative name a business record carries, so matching
         * it is the difference between finding a station and finding nothing.
         *
         * Recall is not uniform, and cannot be made so here: the statewide name
         * index (`{STATE}.business_name_index.ptiles`) is bucketed by the first
         * letter of the *primary* name and carries no brand at all, so brand
         * matching only reaches hits from the spatial sweep around the user --
         * about 8 km. "Shell" finds the badly-named station down the road and
         * misses its twin across the state.
         */
        internal fun bestSimilarity(query: String, name: String, brand: String?): Double =
            maxOf(nameSimilarity(query, name), brand?.let { nameSimilarity(query, it) } ?: 0.0)

        /** 1 minus normalised Levenshtein distance. */
        internal fun editRatio(a: String, b: String): Double {
            if (a.isEmpty() || b.isEmpty()) return 0.0
            val longer = maxOf(a.length, b.length)
            return 1.0 - editDistance(a, b).toDouble() / longer
        }

        private fun editDistance(a: String, b: String): Int {
            var previous = IntArray(b.length + 1) { it }
            var current = IntArray(b.length + 1)
            for (i in 1..a.length) {
                current[0] = i
                for (j in 1..b.length) {
                    val substitution = previous[j - 1] + if (a[i - 1] == b[j - 1]) 0 else 1
                    current[j] = minOf(current[j - 1] + 1, previous[j] + 1, substitution)
                }
                val swap = previous
                previous = current
                current = swap
            }
            return previous[b.length]
        }

        /**
         * How much distance costs a hit, in similarity points.
         *
         * Two parts. Up to [DISTANCE_FALLOFF_KM] it is linear and worth
         * [DISTANCE_WEIGHT] -- a perfect match forty kilometres away scores
         * below a close-enough one on this street. Past that it keeps creeping,
         * [FAR_WEIGHT] per tenfold, because a flat penalty made every hit
         * beyond the falloff equally distant and let an exact match six hundred
         * miles out sit above a prefix match down the road. It never becomes a
         * cutoff: the far hit ranks last, which is the point of searching
         * exhaustively rather than merely widely.
         */
        internal fun distancePenalty(km: Double): Double {
            val near = (km.coerceAtMost(DISTANCE_FALLOFF_KM) / DISTANCE_FALLOFF_KM) * DISTANCE_WEIGHT
            val over = (km - DISTANCE_FALLOFF_KM).coerceAtLeast(0.0)
            return near + FAR_WEIGHT * kotlin.math.log10(1.0 + over / DISTANCE_FALLOFF_KM)
        }

        /**
         * Rank by resemblance, pulled toward the user by [distancePenalty].
         */
        internal fun rankByNameAndDistance(
            query: String,
            hits: List<BusinessResult>,
            origin: GeoPoint?,
            limit: Int,
        ): List<BusinessResult> = hits
            .asSequence()
            .filterNot { isFlightNode(it.name) }
            .map { it to bestSimilarity(query, it.name, it.brand) }
            .filter { (_, similarity) -> similarity >= MIN_NAME_SIMILARITY }
            .map { (hit, similarity) ->
                val km = origin?.let { kotlin.math.sqrt(flatDistance2(it, hit.point)) / 1_000.0 } ?: 0.0
                Triple(hit, similarity - distancePenalty(km), km)
            }
            .sortedWith(compareByDescending<Triple<BusinessResult, Double, Double>> { it.second }.thenBy { it.third })
            .map { it.first }
            .distinctBy { Triple(it.name, round5(it.point.lat), round5(it.point.lon)) }
            .take(limit)
            .toList()

        internal fun mergeBusinessHits(
            hits: List<BusinessResult>,
            limit: Int,
            origin: GeoPoint? = null,
        ): List<BusinessResult> {
            // Nearest first when we know where the user is -- the web demo
            // reaches the same order by walking cells outward from the origin,
            // but the name index answers in one block, so the sort is the whole
            // job. Match quality only breaks ties between equidistant hits.
            val order = if (origin == null) {
                compareByDescending<BusinessResult> { it.score }.thenBy { it.name }
            } else {
                compareBy<BusinessResult> { flatDistance2(origin, it.point) }
                    .thenByDescending { it.score }
                    .thenBy { it.name }
            }
            return hits
                .filterNot { isFlightNode(it.name) }
                .sortedWith(order)
                .distinctBy { Triple(it.name, round5(it.point.lat), round5(it.point.lon)) }
                .take(limit)
        }

        /**
         * Widen a ranked list until it has enough to be worth showing.
         *
         * Near first: the rungs are walked in order and the first holding
         * [ENOUGH_RESULTS] is the answer, so a town's worth of matches is never
         * buried under a better-spelt one three hundred miles away. Nothing is
         * discarded on the way -- what falls outside the chosen rung is counted
         * and reported, because "no results" and "no results near you" are
         * different answers and only one of them is true.
         *
         * `fromRung` is where the walk starts, which is how "search farther"
         * works: past the last rung there is no ceiling left, and the whole of
         * the installed packs answers.
         */
        internal fun widenToEnough(
            hits: List<BusinessResult>,
            origin: GeoPoint,
            fromRung: Int = 0,
            ladder: List<Double> = SEARCH_LADDER_M,
            enough: Int = ENOUGH_RESULTS,
        ): SearchOutcome {
            val rungs = ladder.drop(fromRung).ifEmpty { listOf(Double.MAX_VALUE) }
            val measured = hits.map { it to distanceM(origin, it.point) }
            rungs.forEachIndexed { step, reach ->
                val within = measured.count { it.second <= reach }
                if (within >= enough || step == rungs.lastIndex) {
                    return SearchOutcome(
                        measured.filter { it.second <= reach }.map { it.first },
                        reach,
                        measured.size - within,
                        (fromRung + step).coerceAtMost(ladder.size),
                    )
                }
            }
            // Unreachable: the loop returns on its last iteration.
            return SearchOutcome(hits, rungs.last(), 0, ladder.size)
        }

        /**
         * Metres between two points, on a sphere.
         *
         * [flatDistance2] is enough to sort by and is what ranking uses, but a
         * reach the panel prints as "within 25 miles" is a promise, and flat
         * earth is off by percent at the far end of a ladder that runs to a
         * thousand.
         */
        internal fun distanceM(from: GeoPoint, to: GeoPoint): Double {
            val r = 6_371_000.0
            val dLat = Math.toRadians(to.lat - from.lat)
            val dLon = Math.toRadians(to.lon - from.lon)
            val a = kotlin.math.sin(dLat / 2).let { it * it } +
                cos(Math.toRadians(from.lat)) * cos(Math.toRadians(to.lat)) *
                kotlin.math.sin(dLon / 2).let { it * it }
            return 2 * r * kotlin.math.asin(kotlin.math.sqrt(a).coerceAtMost(1.0))
        }

        /** Squared metres, flat-earth. Ranking never needs the real thing. */
        internal fun flatDistance2(from: GeoPoint, to: GeoPoint): Double {
            val dLat = (to.lat - from.lat) * 111_320.0
            val dLon = (to.lon - from.lon) * 111_320.0 * cos(Math.toRadians(from.lat))
            return dLat * dLat + dLon * dLon
        }

        private fun round5(value: Double): Long = Math.round(value * 100_000.0)

        /**
         * Consecutive (from, to) pairs for a start, ordered waypoints, and end.
         *
         * No waypoints yields the single original leg, so the waypoint path and
         * the plain path are the same code.
         */
        internal fun routeLegs(
            start: GeoPoint,
            waypoints: List<GeoPoint>,
            end: GeoPoint,
        ): List<Pair<GeoPoint, GeoPoint>> {
            val stops = listOf(start) + waypoints + end
            return stops.zipWithNext()
        }

        /**
         * Concatenate leg results into one route.
         *
         * Each leg starts where the previous ended, so the shared joint point
         * is dropped rather than drawn twice.
         */
        internal fun joinLegs(legs: List<RouteResult>): RouteResult {
            require(legs.isNotEmpty()) { "a route needs at least one leg" }
            val points = mutableListOf<GeoPoint>()
            legs.forEach { leg ->
                if (points.isEmpty()) points += leg.points
                else points += leg.points.drop(1)
            }
            return RouteResult(
                points,
                legs.sumOf { it.distanceM },
                legs.sumOf { it.durationS },
                legs.sumOf { it.decodedSegments },
            )
        }

        internal fun layerCandidates(
            files: Array<out File>,
            suffix: String,
            preferredState: String? = null,
        ): List<File> = files
            .asSequence()
            .filter { it.isFile && it.extension.equals("ptiles", ignoreCase = true) }
            .filterNot(PackManager::isBundledConformanceSlice)
            .mapNotNull { file ->
                val layerStem = file.nameWithoutExtension.substringAfter('.', "")
                val match = VERSIONED_STEM.matchEntire(layerStem)
                val logical = match?.groupValues?.get(1) ?: layerStem
                val version = match?.groupValues?.get(2)?.toIntOrNull() ?: -1
                if (logical == suffix) Triple(file, version, file.lastModified()) else null
            }
            // Prefer the newest format-named candidate within a state. Across
            // states the coverage check above, rather than directory order,
            // decides which file answers the coordinate.
            .sortedWith(
                compareByDescending<Triple<File, Int, Long>> { it.second }
                    .thenByDescending { it.third }
                    .thenByDescending { it.first.length() }
                    .thenBy { it.first.name },
            )
            .map { it.first }
            .sortedByDescending { it.name.substringBefore('.') == preferredState }
            .toList()

        internal fun cameraMapFeature(camera: CameraInfo) = MapFeature(
            listOf(GeoPoint(camera.location.lat, camera.location.lon)),
            "camera:${camera.cameraType}",
            camera.name ?: camera.operator ?: camera.deviceType,
        )
    }
}
