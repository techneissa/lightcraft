# Shadow and Highlight Clipping toggles

## What it does

Shadow and Highlight Clipping toggles: the histogram's two triangles each switch their own overlay (blue shadows, red highlights; a real pixel overlay, solid where all channels clip, channel-tinted otherwise), hover previews it, the triangle colour shows which channels clip, J / a click on the plot toggle both; `view.clippingShadows` / `view.clippingHighlights` with View menu items; saved single `showClipping` loads as both.

## Branch

`claude/histogram-clipping-toggles`

## Own files

`crates/ui-egui/src/panels/clipping.rs`

## Hooks in official files

`crates/raster/src/histogram.rs` (`channel_clipping`), `panels/edit/histogram.rs` (the buttons take the tile and mark colours of official's `clip_button_colors`; a channel colour is used only where it stands 3:1 on the tile), `panels/detail.rs` (`clipping_overlay`), `render.rs` (`clip_overlay`, the loupe keeps its pixels), `state.rs` (`clip_shadows`, `clip_highlights`, legacy `show_clipping`), `menus/menubar.rs`, `menus/run.rs`, `menus/ui_commands.rs`, `panels.rs`, `docs/control-protocol.md`, `docs/parity-checklist.md` (LR-VIEW-CLIPPING), `docs/whats-new.md`

## Tests

`lightcraft-raster` `channel_clipping`, `clipping` in ui-egui
