# Classic spots before the white balance

List: Remove, Heal, Clone and Content-Aware spots are drawn on the resampled photo, before the white balance, as LaMa spots are, so Temperature and Tint draw none of them again; a setting brings back the old order; `perf/184-spots-before-wb`.

## What it does

- **New order** (default): LaMa spots, Classic spots, then the camera's local tone mapping, white balance and defringe, then red eye. Old order: LaMa spots, local tone mapping, white balance, defringe, Classic spots, red eye.
- **Heal still matches its surroundings.** Heal and Clone blend in linear light (source + blur of target − source), and the white balance is a 3×3 matrix, so drawing a spot before it gives the same colours as after it, but for clipping at 0 (test: at most 2 levels apart). Remove's automatic source is now chosen on the image before the white balance; a zoom window already chose it there (`spots::pick_source`), so the whole frame and windows now pick alike. Content-Aware fills may pick other patches.
- **One switch for the process**, like the hot pixel limit: `spots_keep::spots_after_wb`, set by `app.spotOrder {afterWhiteBalance}`. A plan reads it once (`Plan::spots_after_wb`), so a render never mixes the orders. Tests use `spots_after_wb_here` (this thread only).
- **Keys.** `spots_keep::key` and `spots_scale::key` hold the white balance and local tone mapping only in the old order; `Plan::pre_retouch_key` and the Background analysis key hold the order. The engine's render keys (thumbnails, previews, loupe, the region tiles' job keys) take `files::render_key` = hot pixel key ^ `spot_order_key`; `app.spotOrder` forgets the remembered settings hashes. Photos without spots keep their keys.
- **GPU.** Spots are drawn on a copy of the resampled image, then `wb_k` and defringe. The boxes a spot edit rewrote grow by defringe's radius for the incremental render (`dirty`).
- **Speed of the old order:** with the setting on, Temperature and Tint draw every Classic spot again (Settings shows no help line for it: the label says what it does).
- **Background's analysis** (Portrait) keeps its proxy with its spots, before the white balance (`retouch_step::spotted_proxy`), so Temperature and Tint draw none there either.
- **Keys after the update:** `spot_order_key` is non-zero for photos with spots in either order, so a picture cached before this change (old order) is never shown. Flipping the order moves the rendered-preview cache to a new generation (`PreviewCache::advance`), so a render already running saves nothing; Hot Pixel Limit does the same.
- - **Where it is set:** Settings ▸ Performance ▸ Draw Spots After White Balance (`settings.spotsAfterWhiteBalance`, synced); CLI `render --spots-after-white-balance`; MCP and the control channel through `app.spotOrder`.

Issue #184.

## Own files

`crates/pipeline/src/tests_spot_order.rs`, `fork/tests.d/spot-order.txt`

## Hooks in official files

- `crates/pipeline/src/spots_keep.rs`: the switch, `key`, `before_spots`, `before_classic`, the order in `lin_cpu_pre_retouch`.
- `crates/pipeline/src/lib.rs`: `Plan::spots_after_wb`, `pre_retouch_key`, the order in `lin_cpu_pre_retouch`. `spots_scale.rs`: `key`. `retouch_step.rs`: the analysis key.
- `crates/gpu/src/render.rs`: `linear`. Test in `crates/gpu/tests/it/spot_reuse.rs`.
- `crates/engine/src/files.rs`: `spot_order_key`, `render_key`; `media/session.rs`, `media/settings_hash.rs` use `render_key`; `media/cache.rs`: `clear_settings_hashes`; `cmd/query.rs`: `app.spotOrder`; test in `tests.rs`.
- `crates/ui-egui`: `state/settings.rs`, `settings_sync/rules.rs`, `app/prefs.rs`, `lib.rs`, `app.rs`, `dialogs/settings/performance.rs`.
- `apps/lightcraft-cli/src/main.rs`: `render --spots-after-white-balance`.
