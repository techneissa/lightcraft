# Preset Card Grid

Every preset list with a preview picture shows cards in a grid, one shared widget: `crates/ui-egui/src/widgets/preset_grid.rs` (#344).

- A card is the picture on top, the name and a label under it. Text wraps, never cut short. A card may have no picture (name and label only).
- Columns follow the width the list has: one when narrow, two, then three (`MAX_COLUMNS`); a card is at least `MIN_CARD_W` wide, and cards share the width evenly.
- Pictures keep the photo's shape (taken from the last picture painted, so the first frame uses 3:2).
- Only the rows in view are built, and a row's height (its wrapped text) is cached for one list and width, so a list of thousands of presets stays cheap.
- The closure given to `Grid::show` holds the list's own behaviour (picture, hover preview, click, menu, overlays such as the star or `+`). Ids registered for the control channel are the lists' own (`relight:preset:<id>`, `preset:<id>`, `profileCell:<id>`, `cameraColourCell:<key>`).

Lists on it: Light Presets (Relight), the Presets panel (with Show Thumbnails on; rows without pictures stay rows), the Profiles browser and its Camera Matching cards.
Not preset lists: Match Look's strip of photos and the loupe's look-ahead.
