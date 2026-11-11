# Performance: a portrait's analyses

List: opening a portrait waits less for its analyses: the original is decoded once, early, for every face's Wrinkles crop, and the Wrinkles search skips work that can't change its result (issue #253). Branch `perf/253-portrait-analyses`.

## What it does

- **One decode for the crops.** The Wrinkles search cuts a float crop of the original for each face. Each crop's
  render decoded the whole photo again: the crop job held the file, not its pixels. The face search's `Cropper` now
  decodes the original once (`Cropper::decode`, `retouch_input/parts/crop.rs`) and renders every crop from that decode.
  Stray Hair's full-size crops (off for now) read it too.
- **The decode starts early**, on a thread of its own, as soon as a face is known to be there
  (`PartsFinder::find`, `retouch_input/parts.rs`):
  - at once, when the parse-ahead index knows the photo's parse and it has a face with Facial Skin (the usual case:
    any photo parsed before or ahead). The picture and YuNet then run beside it;
  - else once YuNet finds a face (beside the parse);
  - else when the first crop is cut.

  A photo without people is never decoded at full size. A full-size source the session already holds (the loupe at
  1:1) is used as it is. A full-size decode the loupe runs at the same moment is shared (`media/shared_decode.rs`).
- **The parse picture keeps its own decode** at the preview level (2560 px). When the loupe decodes at that level,
  the two share one decode. The parse picture can't be cut from the full-size decode. A JPEG's preview-level decode
  is scaled inside the decoder and a raw's is binned, so the pixels would differ. The people parse is cached under a
  hash of those pixels, so every photo would be parsed again.
- **The Wrinkles search skips idle work.** The result is the same to the bit. On four CC0 portraits (p2, p4, p6, p7),
  the stored change and the 8-bit render with all four sliders at 100 are byte-identical to before.
  - The exclusions (ears, nostrils, eyes, the face's edge) are worked out only where an area and the skin let a line
    be. A 16 px tile well inside the face outline skips the outline's distance, which changes by at most a pixel per
    pixel. An eye adds nothing outside its box grown by the lid's margin.
  - The hair test measures its thin valley only on pixels greyer than the skin, and its wide one only where the thin
    line is strong enough to count.
  - The line filter works out a width's direction and its zone's fit only where that width can beat a finer one.
    The damping and the fit only lower a width's depth.
  - The evening takes its medians along the line only at the pixels its across-the-line step reads. Most of the skin
    is too far from any line to be read.
  - The mask sampling (`Scene::mask_plane`, every feature's), the lines' direction field and the glasses' three skin
    statistics run on all cores.
- **Profile lines**: `portrait search: the original decoded for the crops`, and under the wrinkles search one line per
  step (`wrinkles: lines (…)`, `wrinkles: evening`, …).

## Measured

`portrait_open_profile` on the CC0 face-filling portrait p6 (5760 × 3840, face about 1000 px), GPU, release, one face.
The test Mac had other builds running (load 10–40), so wall times swing by ±50 % between runs.

| Face search | Before | After |
|---|---|---|
| Parse cached, load about 10 | 788 ms | 474 ms (parse in the index, as when parsed ahead), 652 ms (cached, not in the index) |
| Parse cached, back to back, load about 15 | 990 ms | 825 ms (not in the index), 860 ms (in the index) |
| Parse not cached | 3316 ms (load 13), 2337 ms (load 15) | 3346 ms, 3119 ms: the parse (1.2–2.6 s) decides |

The steps that hold across runs:

| Step | Before | After |
|---|---|---|
| Wrinkles' float crop (902 × 1654) | 161–215 ms: decode 125 + render | 33–53 ms: render only (the decode, 127–158 ms, ran beside the picture or the parse) |
| Wrinkles search alone, p6, 12 threads (check program) | 341 ms | 251 ms |
| Wrinkles search alone, p4, one thread | 1716 ms | 1434 ms |
| One thread, p4: exclusions / hair test / lines | 226 / 417 / 923 ms | 73 / 86 / 435 ms |

Where the search's time goes now (p4, one thread): the evening 616 ms (43 %), lines 435 ms (30 %), presence 107 ms,
the hair test 86 ms, exclusions 73 ms. Stray Hair (57–100 ms) and the spots (12–80 ms) run beside Wrinkles and are
not on the critical path.

## Not done

- **A coarse-to-fine line search and evening.** Weighed: the wide line widths on a half-resolution picture, and the
  across-the-line median on a coarser grid. Both change the result and would need before/after sheets. They are the
  next step if the search must cost a fraction of today: the evening and the line filter are three quarters of it.
- **Filtered images shared between Wrinkles and Stray Hair.** Wrinkles reads the float crop of the original and Stray
  Hair reads the 2048 px parse picture, so they have no blur in common.
- A face big enough for the preview-level decode (the frame at the search's scale at most 2560 px: a face across more
  than half of a 5000 px photo) could take its crop from that decode and skip the full-size one. Rare, and the crop
  would differ slightly from today's.

## Branch

`perf/253-portrait-analyses` (from `custom`).

## Own files

`fork/perf-portrait-analyses.md`, `fork/tests.d/perf-portrait-analyses.txt`.

## Hooks in official files

None. In fork files:

- `crates/engine/src/retouch_input/parts.rs`: `PartsFinder::find` starts the decode (`Early`, `has_skin`).
- `crates/engine/src/retouch_input/parts/crop.rs`: `Cropper::decode`, the crops' job reads the decoded original.
- `crates/engine/examples/portrait_open_profile.rs`: `PARSE_FIRST=1` parses the photo before the search, as parsing
  ahead does.
- `crates/retouch/src/feature.rs`: `Scene::mask_plane` on all cores.
- `crates/retouch/src/wrinkles.rs` (`SKIP_IDLE`, tests only), `wrinkles/{exclude,flatten,glasses,hairlike,ridge,search}.rs`:
  the skips above.

## Tests

`fork/tests.d/perf-portrait-analyses.txt`.

- `the_wrinkle_crops_of_every_face_come_from_one_decode` (engine): a face search of three faces decodes the photo once
  for its picture and once for every crop. Before, every face's crop decoded it again. A photo without people gets no
  full-size decode.
- `skipping_the_idle_work_leaves_the_change_as_it_was` (retouch): the search on a lined synthetic face gives the same
  change, to the bit, with the skips on and off. The face has forehead lines, a crease by the eye, a grey hair,
  freckles, pores, an Others part and a neck. With one skip broken on purpose, it fails.

None measures time.
