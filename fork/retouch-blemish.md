# Portrait retouching: Blemishes

List: the Portrait tab's Blemishes section: Amount removes spots and acne from each face's skin, healed from the skin around them with its pores; Keep Moles and Freckles (on by default) leaves those; spots are found once per photo with the people parse. Branch `feat/retouch-blemish`.

## What it does

The Blemishes feature of portrait retouching (`fork/retouch-foundation.md`), on the Facial Skin mask of the people
parse (`fork/retouch-masks.md`). For every face or the picked one:

- **Amount** (0…100, off at 0) removes the spots found on the face's skin, the most visible first (darkness times
  size), each one fully as its turn comes: at 25 about the most visible quarter is gone, at 50 every obvious spot,
  at 100 every spot found. Each step of the slider shows (#204).
- **Keep Moles and Freckles** (on by default) leaves moles and freckles where they are. Off, they are removed too.
- Under them, how many spots the settings remove, the share of the skin repaired, and how many moles and freckles
  they keep.

How (`crates/retouch/src/blemish.rs`, `blemish/find.rs`):

- **Finding, once per photo.** On the Facial Skin mask's box, with the face at most 640 pixels: skin-only blurs
  (the picture weighted by the mask, divided by the blurred mask) of a mostly green channel at seven sizes, 0.3–3 %
  of the face across. A spot is a place darker than the skin around it by 6 % or more at its best size, darker than
  every neighbour, round (a line or a wrinkle is not), with skin all around, standing out from the texture around
  it (2.5 times; a beard is too busy), on lit skin. A face under 160 pixels is not looked at. Then each spot's look
  is measured up close (`find/traits.rs`): its darkening per colour, how its darkness falls off (sharp or soft
  edge), a red halo, and the darkness 2–3.5 radii out. Not a spot: a crease or a hair through it (dark both ways),
  the edge of something larger and darker (a nostril, a fold), a large soft greyish dip (the face's shading), or a
  strand Stray Hair's search found under it (`find/strands.rs`; the engine now searches strands first).
- **Kinds** (`find/classify.rs`), the same rules on any skin tone (on dark or tanned skin every spot is brown, so
  no rule asks how brown a spot is):
  - **blemish (red)**: blood darkens green and blue and leaves red; or an inflamed halo. Always removed.
  - **mole**: round, sharp-edged, at least 0.44 % of the face across and much darker (1.7 times) than the spots
    around it.
  - **freckle**: one of a field of like spots (four or more within 9 % of the face, of about its size and
    darkness, the field's spots at least about 0.5 % of the face across), in a linked field of 20 or more. A smaller
    group of brown spots is an older face's age spots, removed; a field of smaller dots is pores or texture. On a
    freckled face the field is searched again, finer, so its faint freckles go too when the switch is off.
  - **not a spot, where stubble grows** (chin, jaw and upper lip, from the face's five landmarks): a field of dots
    smaller than freckles, a small spot much greyer than the face's others (a hair stump), and any spot on textured
    skin. A mole there is still a mole, a red spot still a blemish.
  - everything else is a **blemish**: dark marks, scabs, age spots.
- **Kept with the masks.** The engine finds the spots with the people parse, on the parse's picture (2048 px, in
  linear light, turned like the masks), and keeps them with each face (`Face::spots`). A render, the loupe and an
  export all repair the same spots; the Amount slider only changes each spot's weight.
- **Repair, every render.** Each spot is filled from a ring of skin samples three radii out (a smooth fill), times
  the texture of its donor patch (borrowed as a ratio, at whole-pixel offsets, so the pores stay sharp; without a
  clean donor, as inside a freckle field, the pixel's own fine grain against its 3 × 3 mean), faded in from
  1.7 to 2.6 radii and weighted by the mask. Every ring sample and donor pixel is weighted by the mask where it is
  read, and ring samples inside another spot count for nothing, so no colour from hair, brows, lips or the
  background comes in at the hairline or the jaw. Spots are repaired in parallel and blended in, in order.
- **Zoom never changes a repair.** A spot's radius is held to 1.6 % of the face and a donor farther than 3.6 radii
  is not used, so a repair reads at most 0.11 of the face around a spot; Blemishes declares
  `REACH` 0.14, and a zoomed loupe renders what the face's repairs read (`crates/pipeline/src/retouch_step.rs`).

Speed (a test Mac, release, measured while other builds loaded it, so rough): finding the spots of one face on the
2048 px parse picture 20–400 ms on the seven CC0 audit portraits (#204; the freckled face, 800 spots, 86–370 ms),
after Stray Hair's strand search, which the face search runs anyway. Repair (the slider): 0.7–4 ms on a
1936 px draft of those portraits. The repair walks only each spot's neighbourhood, never the whole face. No model
runs.

A Refine brush per feature comes later (#99): `Blemish::weights` gives one weight per spot, which its strokes will
scale.

Not done: spots smaller than about 2 pixels of the 2048 px parse picture (about 0.3 % of a head-and-shoulders face)
are not looked for, nor faces under 160 pixels there. Raised skin-coloured bumps (lighter than the skin) are not
spots. A small group of real freckles (under 20) is taken for age spots and removed. Stubble on the lower cheeks
outside the chin, jaw and upper lip is told only by Stray Hair's strands. Large soft brown patches (age spots
wider than 3 % of the face) are left to Even Tone.

## Branch

`feat/retouch-blemish`

## Own files

`crates/retouch/src/blemish.rs`, `crates/retouch/src/blemish/find.rs`, `crates/retouch/src/blemish/find/` (`traits.rs`,
`classify.rs`, `strands.rs`), `crates/retouch/examples/blemish.rs`,
`crates/retouch/tests/blemish.rs`, `crates/pipeline/src/tests_retouch_blemish.rs`,
`crates/ui-egui/src/panels/portrait/blemish.rs`, `fork/retouch-blemish.md`,
`fork/tests.d/retouch-blemish.txt`

## Hooks in official files

- `crates/pipeline/src/lib.rs`: `mod tests_retouch_blemish` (tests only).

In fork files:
- `crates/retouch/src/input.rs`: `Face::spots`, `Spot`, `SpotKind`, `Spots`, in the fingerprint.
- `crates/engine/src/retouch_input/parts.rs`: `with_spots` in `found` (the parse's faces get their spots, after
  Stray Hair's strands, which Blemishes reads);
  `crates/engine/src/retouch_input.rs`: `spots: None` for YuNet's faces and the spots in the store's byte count.
- `crates/retouch/README.md`: `Face::spots` in the input contract.
- `crates/ui-egui/src/tests/portrait.rs`: a test of the Blemishes section.
- `docs/parity-checklist.md`: the `LR-EDIT-PORTRAIT` row says Blemishes works.

## Tests

`fork/tests.d/retouch-blemish.txt`. They cover: off and Amount 0 give back the original bits (the switch alone
changes nothing); the switch on by default, a control, and on for settings saved without it; pimples, moles and
a field of freckles found and told apart on a synthetic face with pores; on dark skin, a group of brown marks and a
pimple removed and a freckle field kept with the switch on (#204), and pores not taken for spots; pimples removed and
moles and freckles kept with the switch on, removed with it off; the healed skin keeping about the pores' texture of
clean skin; pixels outside the mask keeping their bits, and a face without the mask left alone; a spot at a soft
skin edge next to black hair, its ring and donor reaching over the edge, repaired as if the hair were skin (no
halo); a huge spot or a far donor from a provider held within the reach; a zoomed window through a spot giving the
whole frame's pixels (`crates/pipeline/src/tests_retouch_blemish.rs`); a draft and an export four times its size
repairing the same spots the same way,
and the same spots found at both sizes; a face whose provider found no spots left alone (no detection in a render)
and a larger Amount removing at least as much; the report's counts and area; a flat grey photo staying flat; a face
cut by the photo's edge; no faces, 50 faces, a 1 × 1 and an empty photo; the parse's faces getting their spots
(also on a turned photo) and none without a skin mask; the section's slider, switch and report.
