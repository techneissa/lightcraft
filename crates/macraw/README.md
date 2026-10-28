# lightcraft-macraw

macOS's own raw decoder (Core Image `CIRAWFilter`) as a linear Rec.2020 base for the macOS Colour
profile choice. Layer 0, no workspace dependencies; a stub that reports itself unavailable on every
other system. All `unsafe` is in `src/macos.rs`.

**Objective-C exceptions are not caught.** Catching them needs `objc2`'s `exception` feature, which
compiles an Objective-C helper with `cc`, and LightCraft is pure Rust (CLAUDE.md). Instead Core Image
is never given a reason to throw. These calls were checked:

- `respondsToSelector:` before every call: the required recipe setters and getters
  (`setBoostAmount:`, `setShadowBias:`, `setExtendedDynamicRangeAmount:`, `setGamutMappingEnabled:`,
  the noise / sharpness / contrast / detail / moiré setters, `neutralChromaticity`,
  `neutralTemperature`, `neutralTint`, `decoderVersion`, `outputImage`), the optional ones
  (`setBoostShadowAmount:`, `setDespeckleAmount:`, `isLensCorrectionSupported` +
  `setLensCorrectionEnabled:`, `isLocalToneMapSupported` + `setLocalToneMapAmount:`), the class
  methods `supportedCameraModels` and `lanczosScaleTransformFilter`, and
  `imageByApplyingTransform:highQualityDownsample:`.
- Only documented keys with their documented value types: `kCIContextWorkingColorSpace` /
  `kCIContextOutputColorSpace` (a `CGColorSpace`), `kCIContextUseSoftwareRenderer` (an `NSNumber`),
  and `kCIInputImageKey` / `kCIInputScaleKey` / `kCIInputAspectRatioKey` on `CILanczosScaleTransform`.
- Every object return is sent as optional and checked for nil (`filterWithImageURL:`,
  `filterWithImageData:identifierHint:`, `outputImage`, `decoderVersion`, `contextWithOptions:`,
  `supportedCameraModels`, the scale filters): the binding's non-null types would panic, and
  `decoderVersion` did come back nil on bad data.
- Empty, non-finite or oversized extents and non-finite white points are refused before drawing;
  the name, size (±2 %) and pixel (128 MP) gates run before any bitmap is allocated; each
  `render:toBitmap:` gets a buffer of exactly `width × rows × 16` bytes.

Mac-only tests read `LIGHTCRAFT_MACRAW_ARW` (a bright landscape ARW) and `LIGHTCRAFT_MACRAW_PORTRAIT`
(a portrait ARW), else the first `.arw` in the corpus, and skip without them.
