# lightcraft-parsing

People parsing: a photo in, people out, each with a face box, 478 face landmarks and part masks. Pure Rust on
candle (`=0.9.2`, Metal on macOS). A leaf crate: no engine, develop, pipeline, UI or retouch dependency.

## Output (the contract)

Everything is plain data in `src/types.rs`. Positions are **fractions of the photo** (0 to 1, y down), so a mask means
the same at any resolution.

| Type | What it is |
| --- | --- |
| `Parsing` | `model_set`, `model_version`, the parsed picture's `width × height`, and `people` (largest first) |
| `Person` | `face: Option<NormRect>`, `face_score`, `landmarks: Vec<Landmark>`, `landmark_score`, `person_id` (passed through from the caller's `FaceHint`), `bounds`, `parts: BTreeMap<Part, Vec<Plane>>` |
| `Part` | `EntirePerson`, `FacialSkin`, `BodySkin`, `Hair`, `Eyebrows`, `EyeWhites`, `Iris`, `Lips`, `Teeth`, `ClothesTop`, `ClothesBottom`, `Dress`, `Shoes`, `Hat`, `Bag`, `Others`; ids `entirePerson` … via `Part::id` |
| `Plane` | one mask over `rect: NormRect`, `width × height` `f32` values 0 to 1, row-major, at the resolution the model saw it. A part's mask is the per-pixel maximum of its planes (`Person::sample`) |
| `Landmark` | `x`, `y` in fractions of the photo, `z` (depth, toward the camera negative) in fractions of the photo's width. MediaPipe order: 468 face points, then right iris 468–472 and left iris 473–477 (centre first) |
| `FaceHint` (input) | a face from a detector: `rect`, optional YuNet 5 landmarks (used to turn the face upright), `score`, `person_id` |

Background is everything that is not a person: `Parsing::background(x, y)`, or `render::Select::Background`.

A part that has nothing in it is left out of `parts`. Facial Skin is face skin, nose and ears; Body Skin is neck,
arms, hands and legs; Others is glasses, sunglasses, earrings, necklaces, belts and scarves.

### Masks at any size

`render::mask(&parsing, Select::Person(k, Part::Hair), w, h)` samples a mask over the whole photo at `w × h`
(bilinear). `render::refined(&parsing, sel, &photo_at_that_size)` also snaps its edges to the photo's own edges with a
guided filter whose radius is 1.5 of the mask's own cells (the area one model pixel covers), so a 24 MP export and a
draft preview draw the same mask, the export with finer edges.

### An adapter for `crates/retouch`

Retouch defines its own input types. Mapping is direct: face box ← `Person::face`, landmarks ← `Person::landmarks`
(`x`, `y` fractions), person id ← `Person::person_id`, a part mask at the render's size ←
`render::refined(..)` (or `render::mask(..)` for a draft).

## Model sets

| Set (`ModelSet`) | Face parser | Body parser | Landmarks | Download |
| --- | --- | --- | --- | --- |
| `segformer` (default) | SegFormer-B0, 19 CelebAMask-HQ classes (`alexkrz/segformer-b0-celebamask_hq`) | SegFormer-B3, 18 clothes classes (`sayeed99/segformer_b3_clothes`) | MediaPipe face mesh v2 (`SceneWorks/mp-facemesh-v2`) | 209 MB |
| `bisenet` | BiSeNet (ResNet-18), 19 classes (`mlx-community/face-parsing-bisenet-bf16`) | SegFormer-B2, 18 clothes classes (`mattmdjaga/segformer_b2_clothes`) | the same face mesh | 141 MB |

Each file is pinned by Hugging Face revision, size and SHA-256 in `src/models.rs` and lives in its own folder under
the models folder (on the Mac `~/Library/Application Support/ArtCraft/models/<folder>/`). Weights are never committed.

## How a parse runs

1. The body parser sees the whole photo (768 px on the long side, up to 1536 when the faces are small, so people in a
   group photo are more than a few pixels).
2. Its person area is split into connected pieces; each face claims its piece, touching people are split by the
   nearest face, a piece below a face (where its body should be) joins that face, and a faceless piece counts as a
   person only when it is big and shows hair, a face, arms or legs.
3. Each face is cut out upright (turned by its eye line) and parsed at 512 × 512; the mesh runs on a 256 × 256 crop.
   The eyes are cut to the mesh's eye openings and split by its iris discs; teeth are the mouth's bright, colourless
   pixels; the face parser's hat counts only where the body parser sees a hat too (it calls short dark hair a hat).
4. Probabilities become masks through a soft threshold (under 0.25 nothing, over 0.75 whole), and planes are trimmed
   to where they hold something.

Without face hints the body parser's face class gives the faces (YuNet is better for small faces).

## Cache

`cache::key(img, hints, set.version())` is a SHA-256 of the pixels, size and hints plus the set's version (its files'
hashes and `PARSE_VERSION`). `cache::store` / `cache::load` keep one file per parse (`LCPARSE1` + JSON header + 8-bit
PNG planes), written atomically; a damaged file is an error the caller treats as "not cached". `cache::prune` keeps
the folder under a size.

## Try it

```bash
cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/ [--set segformer|bisenet|sapiens2] [--models DIR]
```

writes `outdir/<part>.png` (one colour per person), `all.png`, `background.png`, `landmarks.png`, `people.json` and
prints the time of each step (`LIGHTCRAFT_PROFILE=1` for the parse's own steps). Faces come from YuNet in the shared
models folder when it is there.

## Tests

`cargo test -p lightcraft-parsing --lib`. The model tests run with `LIGHTCRAFT_PARSING_MODELS=<models folder>` and
skip without it.
