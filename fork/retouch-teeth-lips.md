# Portrait retouching: Teeth and Lips

List: the Portrait tab's Teeth and Lips section: Teeth Whitening, Lip Color (a picked colour or a lipstick shade, keeping the lips' shading), Lip Hue and Lip Saturation, per face, on the people parser's Teeth and Lips masks. Branch `feat/retouch-teeth-lips`.

## What it does

- **Teeth Whitening** (0…100): takes the yellow out of the face's Teeth mask (OkLab chroma toward neutral; a cool
  cast is kept) and lifts lightness toward a ceiling below white (OkLab 0.93), by less on dark pixels (the gaps
  between teeth, the mouth's shade) and not at all on clearly red ones (lips and gums the mask's soft edge reaches).
  The teeth keep their gaps and shading and never turn paper white.
- **Lip Color** (0…100, with the colour as `lipColorR/G/B`, sRGB 0…255, a rose red by default): the lips take the
  colour the way Recolor does (`fork/recolour.md`): their main colour is measured (lightness weighted by the mask,
  hue and chroma also by chroma), and each pixel keeps its lightness and chroma in proportion to it and its own small
  hue offset, so lines, shading and highlights stay. The panel has a swatch with the colour picker and six lipstick
  shades (nude, rose, red, coral, berry, plum); a shade turns Lip Color on at 60 when it was off, as one undo step; the feature lists Lips and Teeth as the masks it reads, so the face search keeps them.
- **Lip Hue** (−100…+100, up to 30° each way) turns the lips' hue; **Lip Saturation** (−100…+100: chroma × 0.3 to
  × 1.5) scales it.
- **What counts as lips**: the Lips mask less the Teeth mask, held inside the face mesh's outer lip outline and
  outside its inner one (a parser's lips can take in the open mouth, and spill onto the moustache at a small face),
  less pixels of a clearly other hue or bright near-grey ones (teeth the Teeth mask missed). Without the face mesh,
  the mask and the hue test alone.
- Each slider is off at 0; the colour is read only while Lip Color is above 0 (`is_off` ignores it). Per face like
  every portrait feature. Sizes (the outline's soft edge) are fractions of the face. Pixels outside the masks never
  change; a closed mouth (no Teeth mask) gets no whitening.
- Out of gamut: chroma is reduced until no channel drops below where it was (or 0), so a strong colour loses
  saturation, not hue.
- Speed (Apple M4 Pro, release, all four sliders on, the largest face of the test photos, median of 15):
  1.2–2.4 ms at draft (1936 × 1291), 2.1–4.2 ms at loupe (2560 × 1707). CPU only: it reads only the lips' and teeth's
  boxes. No model of its own: it uses the masks and mesh of the people parse (`fork/retouch-masks.md`).
- Not done: the lips' main colour is measured on the pixels the render holds, so a zoomed window that cuts the lips
  measures the part it shows.
- Tuning outside the app: `cargo run --release -p lightcraft-retouch --example teeth_lips -- in.jpg out.jpg --face
  x0,y0,x1,y1 --mask lips=lips.png --mask teeth=teeth.png [--mesh x,y;…] --teeth 60 --lip-color 80 --color 178,52,74
  --hue 0 --saturation 20 [--size WxH] [--time]` (grey mask pictures over the whole photo).

## Branch

`feat/retouch-teeth-lips`, from `custom` (with PR #122, part masks reaching the retouch step).

## Own files

`crates/retouch/src/teeth_lips.rs`, `crates/retouch/examples/teeth_lips.rs`, `crates/retouch/tests/teeth_lips.rs`,
`crates/ui-egui/src/panels/portrait/teeth_lips.rs`, `crates/ui-egui/src/tests/portrait_teeth_lips.rs`,
`fork/retouch-teeth-lips.md`, `fork/tests.d/retouch-teeth-lips.txt`

## Hooks in official files

- `crates/ui-egui/src/tests.rs`: `mod portrait_teeth_lips;`.
- Fork files: `crates/retouch/tests/foundation.rs` and `crates/engine/src/retouch_input/tests.rs` used the stub's
  `retouch.teethLips.amount`; they now use `retouch.teethLips.teethWhitening`.

## Tests

`fork/tests.d/retouch-teeth-lips.txt`. They cover: off by default, every amount at 0 with another colour set, bit
for bit; nothing outside the masks changes; whitening takes the yellow out, lifts a tooth below the ceiling, lifts a
gap far less and keeps the shaded row darker, and leaves the lips; a closed mouth (no Teeth mask) unchanged; Lip
Color lands on the picked hue, keeps the shading across the lips and leaves the teeth; Lip Hue turns by 30° at
+100 at the same chroma, Lip Saturation halves or raises chroma; the face mesh keeps a dark-red open mouth out of
the lips (fails without it); no faces, 50 faces, a face cut by the edge, a 1 × 1 and a flat grey photo (no colour
from nothing); a draft and a 4× export agree after scaling; the panel's shade colouring the picked face only, one
undo step; the feature lists Lips and Teeth as the masks it reads, so the face search keeps them.

## What's New entry

### Teeth and Lips
- The Portrait tab whitens teeth and colours lips, per face: Teeth Whitening takes the yellow out and brightens the
  teeth while keeping their gaps and shading; Lip Color gives the lips a picked colour or one of six lipstick shades
  and keeps their lines and highlights; Lip Hue and Lip Saturation fine-tune them.

## Parity rows

| LR-EDIT-PORTRAIT-TEETH-LIPS | Portrait ▸ Teeth and Lips | — | ✅ | `ctl:retouch.teethLips.teethWhitening`, `ctl:retouch.teethLips.lipColorAmount`, `ctl:retouch.teethLips.lipHue`, `ctl:retouch.teethLips.lipSaturation`, `crates/retouch/src/teeth_lips.rs`, `crates/ui-egui/src/panels/portrait/teeth_lips.rs` | an extra (the reference app has none): Teeth Whitening, Lip Color (picked colour or lipstick shade, shading kept), Lip Hue, Lip Saturation, per face, on the people parse's Teeth and Lips masks and face mesh. See `fork/retouch-teeth-lips.md` |
