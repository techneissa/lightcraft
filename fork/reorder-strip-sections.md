# Reorder the tool strip and drag Edit sections

List: drag the tool strip's icons to reorder them (right-click: Move Up, Move Down, Reset Order) and drag the Edit panel's sections by their heading, both saved with the UI state; `feat/58-reorder-strip-sections`.

## What it does

- **Tool strip.** The ten icons (Presets, Edit, Crop, Remove, Masking, Red Eye, Versions, Activity, Keywords, Info) move by dragging an icon up or down; a line shows where it will land and releasing there moves it. The strip has fixed places (two, a divider, four, a divider, two, About, and two pinned to the bottom): the icons move between places, the dividers and About stay. Right-click an icon for Move Up, Move Down, Reset Order (and Assign Shortcut…). Esc, or releasing away from the strip, leaves the order. The order is saved in `ui.json` (`toolStrip`, a list of icon ids); a state saved before it loads in the usual order, and a damaged list is repaired (unknown and repeated ids dropped, missing ones last).
- **Edit sections.** Light, Color, Effects, Detail, Optics and Calibration are dragged by their heading, with the same drop line and floating name as the left side bar's sections. Move Up / Move Down in the heading's menu stay. The Solo and on/off buttons at the heading's right keep their own presses.
- A click still opens an icon or a heading; only a drag moves it.
- Commands: `view.toolStrip {order?}` (no params reads), `view.toolStripMove {tool, direction | before}`, `view.toolStripReset`; the Edit sections use the existing `view.editSectionMove {section, before}`.

## Branch

`feat/58-reorder-strip-sections`

## Own files

`crates/ui-egui/src/panels/tool_strip_order.rs` (+ `tool_strip_order/tests.rs`), `crates/ui-egui/src/panels/edit_sections/drag.rs`

## Hooks in official files

`panels/strip.rs` (draws the icons in the saved order, drag and menu), `panels.rs` (`mod`), `state.rs` (`tool_strip`, `dragging_tool`, `dragging_edit_section`), `menus/run.rs` and `menus/ui_commands.rs` (the commands), `widgets.rs` (`icon_button_sense`, a draggable `icon_button`; the section header senses drags), `panels/edit/sliders.rs` and `panels/edit.rs` (drag start and the drop line), `panels/edit_sections.rs` (`mod drag`, doc line), `panels/edit_sections/tests.rs` (drag test)

## Tests

`tool_strip_order` (order logic, saved and old state, damaged list, the drag with its line, click vs drag, Esc and release outside, the commands, the icon menu), `edit_sections` (`dragging_a_heading_moves_its_section`)

## What's New entry

### Arrange the tool strip and the Edit panel by dragging

Drag an icon in the tool strip, or a section heading in the Edit panel, up or down to put it where you want it. A line shows where it will land. The order is remembered. Right-click a strip icon for Move Up, Move Down and Reset Order.
