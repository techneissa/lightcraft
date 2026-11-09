# Portrait retouching: Blemishes

List: the Portrait tab's Blemishes section: Amount removes spots and acne from each face's skin, healed from the skin around them with its pores; Keep Moles and Freckles (on by default) leaves those; spots are found once per photo with the people parse. Branch `feat/retouch-blemish`.

## What it does

The Blemishes feature of portrait retouching (`fork/retouch-foundation.md`), on the Facial Skin mask of the people
parse (`fork/retouch-masks.md`). For every face or the picked one:

- **Amount** (0…100, off at 0) removes the spots found on the face's skin, the strongest first: at 50 a strong spot
  is gone and a faint one stays; at 100 every spot found is gone.
- **Keep Moles and Freckles** (on by default) leaves moles and freckles where they are. Off, they are removed too.
- Under them, how many spots the settings remove, the share of the skin repaired, and how many moles and freckles
  they keep.

How (`crates/retouch/src/blemish.rs`, `blemish/find.rs`):

- **Finding, once per photo.** On the Facial Skin mask's box, with the face at most 640 pixels: skin-only blurs
  (the picture weighted by the mask, divided by the blurred mask) of a mostly green channel at seven sizes, 0.3–3 %
  of the face across. A spot is a place darker than the skin around it by 6 % or more at its best size, darker than
  every neighbour, round (a line or a wrinkle is not), with skin all around, standing out from the texture around it
  (a beard or stubble is too busy), on lit skin (not a beard, the hairline or deep shadow), and with red darkening
  less than the rest (a shadow or a hair darkens every colour alike). A spot whose red darkens nearly as much as the
  rest is brown: brown, dark and round is a mole; small brown spots in a group are freckles when faint and moles
  when dark; the rest are blemishes. Each spot gets a strength and a donor: a clean patch of skin next to it. On a
  black-and-white photo colour can't tell spots from shadows; shape and contrast decide alone.
- **Kept with the masks.** The engine finds the spots with the people parse, on the parse's picture (2048 px, in
  linear light, turned like the masks), and keeps them with each face (`Face::spots`). A render, the loupe and an
  export all repair the same spots; the Amount slider only changes each spot's weight.
- **Repair, every render.** Each spot is filled from a ring of skin samples three radii out (a smooth fill), times
  the texture of its donor patch (borrowed as a ratio, at whole-pixel offsets, so the pores stay sharp), faded in from
  1.7 to 2.6 radii and weighted by the mask. Ring samples off the skin or inside another spot count for nothing, so
  no colour from hair, brows or lips comes in. Spots are repaired in parallel and blended in, in order.

Speed (Apple M4 Pro, release, the machine under load ~8): finding the spots of one face on the 2048 px parse
picture 23–124 ms (techneissa's A7 V portraits 23–64 ms; a face covered in 240 dark spots 124 ms). Repair (the slider):
0.16–0.91 ms on a 1936 px draft of a head-and-shoulders portrait, 4.2 ms with 242 spots, 6.7 ms for a face filling a
1780 × 1936 draft with 147 spots. No model runs.

A Refine brush per feature comes later (#99): `Blemish::weights` gives one weight per spot, which its strokes will
scale.

Not done: in a dense cluster of dark spots (one spot every few pores), the cluster reads as textured skin, so Keep
Moles and Freckles off removes only part of it. Spots smaller than about 2 pixels of the 2048 px parse picture
(about 0.3 % of a head-and-shoulders face) are not looked for.

## Branch

`feat/retouch-blemish`

## Own files

`crates/retouch/src/blemish.rs`, `crates/retouch/src/blemish/find.rs`, `crates/retouch/examples/blemish.rs`,
`crates/retouch/tests/blemish.rs`, `crates/ui-egui/src/panels/portrait/blemish.rs`, `fork/retouch-blemish.md`,
`fork/tests.d/retouch-blemish.txt`

## Hooks in official files

None. In fork files:
- `crates/retouch/src/input.rs`: `Face::spots`, `Spot`, `SpotKind`, `Spots`, in the fingerprint.
- `crates/engine/src/retouch_input/parts.rs`: `with_spots` in `found` (the parse's faces get their spots);
  `crates/engine/src/retouch_input.rs`: `spots: None` for YuNet's faces and the spots in the store's byte count.
- `crates/retouch/README.md`: `Face::spots` in the input contract.
- `crates/ui-egui/src/tests/portrait.rs`: a test of the Blemishes section.
- `docs/parity-checklist.md`: the `LR-EDIT-PORTRAIT` row says Blemishes works.

## Tests

`fork/tests.d/retouch-blemish.txt`. They cover: off and Amount 0 give back the original bits (the switch alone
changes nothing); the switch on by default, a control, and on for settings saved without it; pimples, moles and
freckles found and told apart on a synthetic face with pores, and pores not taken for spots; pimples removed and
moles and freckles kept with the switch on, removed with it off; the healed skin keeping about the pores' texture of
clean skin; pixels outside the mask keeping their bits, and a face without the mask left alone; no darkening next
to black hair at the mask's edge; a draft and an export four times its size repairing the same spots the same way,
and the same spots found at both sizes; a face whose provider found no spots left alone (no detection in a render)
and a larger Amount removing at least as much; the report's counts and area; a flat grey photo staying flat; a face
cut by the photo's edge; no faces, 50 faces, a 1 × 1 and an empty photo; the parse's faces getting their spots
(also on a turned photo) and none without a skin mask; the section's slider, switch and report.
