# Sapiens2 people parts: light and dark clothes

List: a third people-parts model set, Sapiens2 (Meta's Sapiens2-seg 0.4B, about 0.8 GB, downloaded only after a yes), that finds light and dark clothes, bare skin and hair the two SegFormer sets miss, with each person parsed up close (issue #93). Branch `feat/93-sapiens2-clothes`.

## What it does

- **Settings ▸ Faces ▸ People Parts ▸ Model Set** has a third choice, **Sapiens2**, beside SegFormer and BiSeNet
  (`parsing.modelSet {set: "sapiens2"}`). Like the others it is asked first and downloaded only on Download
  (`parsing.download {set: "sapiens2", confirm: true}`): the SegFormer set's files plus Sapiens2's 813 MB file
  (814 MB to download when the SegFormer set is there, 1,022 MB when not), size and SHA-256 checked, resumable, one
  download at a time, into the folder shared with PhotoCraft.
- **What it fixes** (#81's sets, #93): a light T-shirt seen from behind, a dark navy coverall, black suits on a dark
  background, dark gym shirts, a dark tank top called a dress. Sapiens2 decides where the person, skin, hair and
  clothes are; the SegFormer-B3 clothes parser only names the garments inside Sapiens2's clothes (Top, Bottom, Dress,
  Hat, Bag, Others), which Sapiens2 has no classes for. Bare chest, arms, hands, legs and feet are Body Skin.
- **Faces** are parsed as in the SegFormer set (SegFormer-B0 face parser and face mesh): brows, eyes, iris, lips and
  teeth are the same, and beards count as facial skin.
- **Each person up close**: the first 8 people whose body box covers less than half the photo get a Sapiens2 pass of
  their own (it is unsure of people further back in a whole photo), all of them together at most twice the trained
  input size. Each cell takes the pass of the box it belongs to, never a mix of two; a person without a pass (a
  portrait that fills the frame, the ninth person) keeps the whole photo's pass, which runs only then. Sapiens2 has a
  say only inside the people's boxes, so a photo without people (a car at night) finds none. The neck below the chin
  is Body Skin.
- **Licence**: Sapiens2's weights are under Meta's
  [Sapiens2 License](https://github.com/facebookresearch/sapiens2/blob/main/LICENSE.md): free use, redistribution only
  with the licence, and it forbids surveillance, biometric processing and identifying people. Its masks may feed
  Select People: splitting a person into parts is not identifying them, and techneissa uses the app personally. They
  never reach face detection, face recognition or the People catalog (a parse only carries the person id of a face
  region the caller hands in). The weights are never shipped with LightCraft (`NOTICE`).
- How it does (2048 px photos, second parse with the models loaded, a test Mac while other builds ran):

  | Set | Download | Per photo with people | No people | Load |
  | --- | --- | --- | --- | --- |
  | SegFormer | 209 MB | 2.5 to 4.4 s | 2.9 s | 0.5 to 1.8 s |
  | BiSeNet | 141 MB | 1.9 to 2.8 s | 1.9 s | 0.3 to 1.2 s |
  | Sapiens2 | 1,022 MB (814 MB more) | 3.5 to 8.1 s | 2.5 s | 2.2 s (8.9 s the first time: the file's SHA-256) |

  Sapiens2 is the best of the three on clothes and body skin on every test photo (CC0: a light T-shirt from behind, a
  dark coverall, a dark suit on dark, a group of three; private test photos: groups indoors, portraits, a car at
  night). Still weak: a man whose face is hidden behind a megaphone is
  only partly found (no face, so no pass of his own); a patterned blanket worn as a scarf is Others.
- After the fix round (final build, Sapiens2 set, one run each, load about 30): a 26-person group photo (CC0, 960 px)
  17.8 s, of which 5.5 s the people's passes (the SegFormer set alone took about 20 s on it under similar load); the
  sofa group 4.8 s, of which 3.1 s the passes. Model load 2.5 s.
- Memory: the linear layers' weights stay bfloat16 on the GPU, widened to f32 per use: 0.8 GB resident instead of
  1.6 GB, masks the same to the bit. The process's peak footprint while parsing is 7 to 11 GB (SegFormer set: 2.8 GB):
  candle's Metal buffer pool keeps a pass's intermediate buffers until its next flush. Splitting the head's
  convolutions into bands of rows and smaller attention chunks did not lower it measurably.
- Speed: candle's Metal matrix multiply is the limit (a full 1024 × 768 pass is about 2.3 s of transformer blocks,
  half precision gives nothing; Core ML is the next step, in its own lane). The head's four 2× transposed
  convolutions run as 3 × 3 convolutions and a pixel shuffle (4 s → 0.7 s per pass); a person's pass is sized to the
  body map cells it fills.
- Try it: `cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/ --set sapiens2`.

## Branch

`feat/93-sapiens2-clothes`, from `custom` (it builds on `feat/81-people-parsing`).

## Own files

`crates/parsing/src/{sapiens,fuse,crops}.rs`, `fork/sapiens2-clothes.md`, `fork/tests.d/sapiens2-clothes.txt`

## Hooks in official files

- `NOTICE`: Sapiens2's licence, in the people parsing paragraph.
- The rest belongs to #81 (`fork/people-parsing.md`): `crates/parsing/src/{lib,models,parser}.rs` (the set, its
  file, the passes), `crates/parsing/src/tests/models.rs` (model tests read the shared folder),
  `crates/parsing/{README.md,examples/parts.rs}`, `crates/engine/src/cmd/parsing.rs` (the set's name in messages),
  `crates/engine/src/tests/people_parsing.rs` (three sets; the opt-in test that downloaded every set over the network
  is gone), `fork/people-parsing.md`.

## Tests

`fork/tests.d/sapiens2-clothes.txt`: the transformer pieces (bf16 rounding of the rotary table, rotary turns,
instance norm, the 3 × 3 convolution equals the transposed one), the fusion (Sapiens2 places clothes the clothes
parser calls background, the clothes parser names a dress or hat, the neck below the chin is Body Skin, the fused map
sums to one, no say away from the people), the people's boxes, the passes with a stand-in network (a pass lands on
the cells it saw at the photo's edge and in the middle; overlapping boxes never mix; a ninth person keeps the whole
photo's pass; the passes' budget), the set's files and size, and with the models in the shared folder a
Sapiens2 parse of a picture without people and of a 1 × 1 photo.

## What's New entry

### Sapiens2 People Parts
- A third model set for people's parts, Sapiens2, in Settings ▸ Faces ▸ People Parts. It finds clothes the other
  sets miss (a light shirt seen from behind, dark coveralls and suits, dark shirts), bare skin and hair, and parses
  each person up close. About 0.8 GB, downloaded only when you choose it.
