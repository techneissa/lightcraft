# lightcraft-parsing

People parsing: a photo in, people out, each with a face box, 478 face landmarks and part masks. Pure Rust on
candle (`=0.9.2`, Metal on macOS). On a Mac, the SegFormer face and clothes parsers and the face mesh run through
Core ML on the GPU when their converted models are in the models folder (`src/coreml.rs`, below). A leaf crate: no engine, develop,
pipeline, UI or retouch dependency.

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
| `sapiens2` | the SegFormer set's | Sapiens2-seg 0.4B, 29 body-part classes (`mlx-community/sapiens2-seg-0.4b-bf16`), with the SegFormer-B3 naming the garments | the same face mesh | 1,022 MB (813 MB more than `segformer`) |

Sapiens2's weights are under Meta's Sapiens2 License, which forbids identifying people. Its masks may feed Select
People (splitting a person into parts is not identifying them); they never reach face detection, face recognition or
the People catalog (`person_id` only passes through what the caller's face hints carry).

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

### The `sapiens2` set

Sapiens2 decides where the person, skin, hair and clothes are; the B3 clothes parser only names the garments inside
Sapiens2's clothes (Top, Bottom, Dress, Hat, Bag, Others), which Sapiens2 has no classes for (`src/fuse.rs`).

1. The B3 runs as above; its person area and the faces give each person a box (the body box around their face: four
   face widths across, down to eight face heights below it, grown to take in what the B3 saw of them close by).
2. Each of the first 8 people whose box covers less than half the photo gets a Sapiens2 pass of their own over the
   box (`src/crops.rs`): Sapiens2 is trained on pictures framed around one person, and in a whole photo it is unsure of
   the people further back. The box is sampled at 1.5 model pixels per body-map cell (long side at least 384, at most
   the trained 1024 × 768 patches); all the passes together see at most twice the trained patches, past which each is
   made smaller by the same factor (eight large people would otherwise take about 20 s).
3. Each body-map cell belongs to one box: the one whose say there is greatest (1 inside, fading at its inner edges),
   the smaller box on a tie (a person inside another's box). It takes that box's pass, or the whole photo's pass (half
   the trained patches) when that box had none (a portrait that fills the frame, the ninth person); the whole photo's
   pass runs only then. Passes never mix, and they are written straight into the one probability grid.
4. Sapiens2 has a say only inside the people's boxes, fading at their edges; elsewhere the B3's map stays. Sapiens2 is
   trained on pictures with people and calls much of a picture without any (a car at night) a person.
5. Each Sapiens2 class goes to its B3 class: background, hair, face (with lips, teeth and tongue; below a face's chin
   its face-and-neck class is the neck, Body Skin), eyeglasses (Others), shoes and socks (Shoes), arms, hands, legs,
   feet and bare torso (Body Skin). Upper clothing is shared
   among top, dress and scarf, lower clothing among trousers, skirt, dress and belt, and apparel among hat, bag,
   scarf, belt and the rest, by the B3's own probabilities there (plus 0.05 for the usual garment, so where the B3 sees
   background, as on dark and light clothes, upper clothing is a top).

The face parts (brows, eyes, iris, lips, teeth) still come from the face parser and the mesh: Sapiens2 has no eyes or
brows, and its lips match the face parser's.

The network call is a closure (`crops::Forward`), so the placement tests run a stand-in network without the model.

The network (`src/sapiens.rs`) is a plain ViT with 2-D rotary positions, RMS and query/key norms, grouped-query
attention in its middle blocks and SwiGLU feed-forwards, then four 2× transposed convolutions; each transposed
convolution runs as a 3 × 3 convolution and a pixel shuffle (the same sums, several times faster on Metal). The
linear layers' weights stay bfloat16 on the device, the checkpoint's own precision, and are widened to f32 for each
use (0.8 GB resident instead of 1.6 GB; the masks are the same to the bit). On the
same input it gives the same class as mlx-vlm's port at 99.8 % of the pixels.

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

`cargo run -p lightcraft-parsing --release --example coreml_compare -- in.jpg` parses with candle and with Core ML
(with the face mesh on candle, then on Core ML too) and prints each one's time, how well their masks agree per part
and how far their landmarks are apart.

## Core ML

`tools/convert-coreml-parsing.py` converts SegFormer-B0 (faces) and SegFormer-B3 (clothes) once, by hand, into
`<models>/<folder>/coreml/<folder>.mlpackage`; the app compiles each once into `<folder>.mlmodelc` beside it.
`Runtimes` picks the engine of each (Automatic: Core ML when it is there and loads, candle for good once it fails
on a photo; Core ML; Candle). The face parser takes up to 8 crops a call and gives the ten face groups at 512² (the
resize, softmax and group sums are inside it). The clothes parser takes the photo's grid in the top-left of one of
28 fixed canvases with a fourth channel marking the photo; every layer ignores the padding (zeros for the
convolutions, masked keys for attention, resizes clamped at the photo's edge), so the result equals the run on the
photo alone. Both run on the GPU: as fast as the Neural Engine for the face parser and closer to float32, and the
Neural Engine compiler takes over 15 minutes for one large clothes canvas. `tools/convert-coreml-facemesh.py` converts
the face mesh (every set's) the same way: up to 8 crops of 256² a call, float32 on the GPU (as fast as float16 for it,
and equal to candle to 0.001 px), one row per crop of the points and the face score. BiSeNet, SegFormer-B2 and
Sapiens2 stay on candle.

On the GPU, candle 0.9.2 gives every face after the first in a face-parser call other numbers than the face alone, so
candle's face parser takes one face a call there (the face mesh is not hit, and keeps its batches). On the CPU, candle's
convolution reads a contiguous input with as many channels as rows and columns as channels-last; `nn::Conv` runs such a
convolution with rows and columns swapped.

## Tests

`cargo test -p lightcraft-parsing --lib`. The Core ML tests (`src/tests/coreml.rs`) compare both engines on a made-up
picture (the parsers, the face mesh, Core ML failing on a photo's first faces, candle on the CPU against the GPU) and
check the fallback; they read the shared models folder through links and skip without the models. The model tests read the models from the shared folder (or
`LIGHTCRAFT_PARSING_MODELS`) and skip without them; no test downloads a model.
