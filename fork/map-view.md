# Map view

## What it does

View ▸ Map shows the photos in view as pins and clusters on a map. It is bitshiftza's open Map PR stack on storytold/lightcraft, taken early (issue #4), merged with current official `main`:

- [#797](https://github.com/storytold/lightcraft/pull/797) Map view: OpenStreetMap tiles within the OSM tile policy, pins, clusters, "© OpenStreetMap contributors" always painted.
- [#804](https://github.com/storytold/lightcraft/pull/804) A map of the photo's place in the Info panel.
- [#810](https://github.com/storytold/lightcraft/pull/810) Drag photos onto the map to geotag them, Lock Markers, Remove GPS.
- [#819](https://github.com/storytold/lightcraft/pull/819) Location filter bar (Visible on Map, Tagged, Untagged).
- [#906](https://github.com/storytold/lightcraft/pull/906) Saved locations; private ones are stripped on export.
- [#916](https://github.com/storytold/lightcraft/pull/916) GPS track logs drawn on the map.
- [#921](https://github.com/storytold/lightcraft/pull/921) Map keys, View ▸ Map menu, Settings ▸ Map, `docs/map.md`.

Network and cache (`docs/map.md` has the detail):
- **This fork starts Offline, and a switch changes that.** The official app starts on OpenStreetMap; here a library with no saved tile source, or a damaged one, starts Offline: the map is grey with "Map tiles are off (Settings ▸ Map)", and no tile server is asked. Settings ▸ Map ▸ **Start on OpenStreetMap** (off by default, kept with the app's settings in `ui.json`) makes every library with no saved source start on OpenStreetMap instead. A library's own saved source (picked in Settings ▸ Map, or by `map.tileSource`) always wins; a default is never written to `prefs.json`. On the grey map a small globe button, "Use OpenStreetMap", turns the switch on and loads tiles at once (this library then saves OpenStreetMap as its source).
- Once on, tiles are fetched only while a map is on screen (the Map view, or the Info panel's small map), only those in view plus one ring, 2 requests at a time, never at startup, never prefetched; backoff on errors and 429. No photos, positions or file names are sent; the server sees the places you look at, your IP address and `LightCraft/<version>` as the User-Agent.
- Tiles are cached in `~/Library/Caches/LightCraft/map-tiles/<source id>/` (500 MB cap, least recently used evicted, `Cache-Control`/`ETag` honoured, 7 days when the server says nothing). Settings ▸ Map shows the size and clears it. `LIGHTCRAFT_MAP_CACHE` overrides the folder.

## Branch

`feat/4-map-view`

## Own files

The whole stack: `crates/map/`, `crates/engine/src/{map,locations,tracklog}.rs`, `crates/engine/src/cmd/map.rs`, `crates/ui-egui/src/panels/map*.rs`, `crates/engine/src/tests/{location,saved_locations,tracklog}.rs`, `crates/ui-egui/src/tests/map*.rs`, `docs/map.md`.

## Hooks in official files

The stack's own hooks (view mode, menus, commands, Settings tab, Info panel, filmstrip, rules `gpsArea`/`gpsRadius`, export of private locations, sidecar GPS ownership, locale strings) are the official PRs' and go away with them. What this fork changed on top:
- `crates/engine/src/map.rs`: the default and fallback tile source is Offline, or OpenStreetMap when `set_start_on_osm(true)`; `source_is_default` keeps a default out of `prefs.json` (`library.rs`) and `cmd/map.rs` restores it when a save fails. `ui-egui`: `AppSettings::map_start_on_osm` (`state/settings.rs`), handed to the engine in `app/prefs.rs`, the switch in `panels/map_settings.rs`, the globe button in `panels/map.rs` (`controls`, `use_openstreetmap`) and `Icon::Globe` in `icons.rs`. Tests in `cmd/map.rs` and `tests/prefs.rs` pick OpenStreetMap first.
- `crates/ui-egui/src/panels/map.rs`: `map_canvas` is split into steps (`Frame`, `track_pin_drag`, `moved_viewport`, …), and `panels/right/info.rs`: `gps_map`, to meet the new clippy complexity limit (30).
- `crates/ui-egui/src/keybinds/model/conditions.rs` (`DECLARED`) and `keybinds/when/keys.rs` (the `view` value `map`): the Map view's keys share H, `\`, O, ⌘K, `=` and `-` with other commands by conditions (`view == map` / `view != map`), like the X and A pairs; `shortcuts::CONTEXTUAL` lists the pairs.
- `crates/engine/src/sidecar_queue.rs`: the background XMP writer carries `gps_owned` (the stack's `Session::gps_edited`) to `sidecar_contents`.
- `crates/catalog/src/rules.rs`: the GPS rule fields read the photo as shown (`meta.shown()`), as the other fields do.
- `docs/map.md`: the default tile source line, and the memory cap.
- Fixes on top of the stack: the decoded-tile cache (`crates/map/src/service.rs`, `MEMORY_BYTES`) and the tile textures (`panels/map.rs`, `MAX_TEXTURE_BYTES`) are capped by bytes, not tile counts; Settings ▸ Map's custom-server fields are refilled from the source in use after Apply (`map_settings.rs`); the map's own notes use "→" (the UI font has no ▸); Settings headings in Title Case. Tests: `decoded_tiles_are_capped_by_bytes`, `every_cluster_pin_shows_its_photo_count`, `no_tile_request_starts_after_the_map_leaves_the_screen`, `failing_tiles_are_not_retried_after_the_map_leaves_the_screen`, and the on-demand `map_frame_times_with_five_thousand_pins` (`-- --ignored --nocapture`).

## Tests

`cargo test -p lightcraft-map`, `cargo test -p lightcraft-meta gpx`, `cargo test -p lightcraft-catalog rules`, `cargo test -p lightcraft-engine --lib -- map location tracklog saved_locations prefs`, `cargo test -p lightcraft-ui-egui --lib -- map` (these never touch the network: a fake tile server). `fork/tests.sh` has the lines.

## When upstream merges

Official PRs [#797](https://github.com/storytold/lightcraft/pull/797), [#804](https://github.com/storytold/lightcraft/pull/804), [#810](https://github.com/storytold/lightcraft/pull/810), [#819](https://github.com/storytold/lightcraft/pull/819), [#906](https://github.com/storytold/lightcraft/pull/906), [#916](https://github.com/storytold/lightcraft/pull/916), [#921](https://github.com/storytold/lightcraft/pull/921). When they merge (one by one, or the stack at once):
1. Merge `origin/main` into `custom`; on a clash in a file the stack owns, take official's and drop the fork copy.
2. Keep the fork's changes listed above that official did not make: the Offline default (a one-line change back if techneissa wants OpenStreetMap on by default), the key conditions, `gps_owned`.
3. Drop the `map_canvas` and `gps_map` splits when official's version meets the complexity limit by itself.
4. Remove this file, its FORK.md line and its `fork/tests.sh` lines once nothing of the fork is left.
