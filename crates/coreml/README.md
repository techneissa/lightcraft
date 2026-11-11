# lightcraft-coreml

Runs converted AI models through Apple's Core ML (Neural Engine, GPU, CPU) on the Mac. Layer 0, no workspace
dependencies; a stub that reports itself unavailable on every other system. All `unsafe` is in `src/macos.rs`.
The candle port of each model stays the reference and the fallback (`fork/coreml.md`).

API: `available()`, `compile(package, dest)` (an `.mlpackage` to a compiled `.mlmodelc`), `load_package(package,
compiled, units)` (compiles when the copy is missing or was made from another package, keyed by its `Manifest.json`
in `<compiled>.key`, and again once when a cached copy fails to load),
`Model::load(compiled, ComputeUnits)`, `Model::predict(input, shape, data, output)` on `f32` tensors. Every failure
is an `Error`, never a panic. `Model::load_function` and `load_package_function` load one function of a model with
several (`MLModelConfiguration.functionName`, macOS 15 or later: `setFunctionName:` is checked with
`respondsToSelector:` first, and its absence is a load error); the functions of one package share its weights.

**Objective-C exceptions are not caught.** Catching them needs `objc2`'s `exception` feature, which compiles an
Objective-C helper with `cc`, and LightCraft is pure Rust (CLAUDE.md). Core ML is never given a reason to throw:

- `available()` checks macOS 13 or later and, with `respondsToSelector:`, every selector used:
  `compileModelAtURL:error:`, `modelWithContentsOfURL:configuration:error:`, `predictionFromFeatures:error:`,
  `modelDescription`, `inputDescriptionsByName`, `outputDescriptionsByName`, `setComputeUnits:`,
  `initWithShape:dataType:error:`, `getBytesWithHandler:`, `getMutableBytesWithHandler:`,
  `initWithDictionary:error:`, `featureValueWithMultiArray:`.
- Every object a call may return as nil is received as an `Option` and checked; failures come back through an
  `NSError` out-parameter (`compileModelAtURL:error:`, `modelWithContentsOfURL:configuration:error:`,
  `initWithShape:dataType:error:`, `initWithDictionary:error:`, `predictionFromFeatures:error:`,
  `featureValueForName:`, `multiArrayValue`, `modelDescription`).
- Input and output names are checked against the model's description, and the shape against the data, before the
  call. Tensors have 1 to 5 dimensions and at most 64 M numbers.
- The byte handlers copy exactly the buffer size Core ML passes; the output is read through its own strides (the
  Neural Engine pads rows) and any offset past the buffer is an error.
- Every call runs in an autorelease pool. A loaded model is used by one thread at a time (a `Mutex`).

`compileModelAtURL:error:` is deprecated in favour of the asynchronous call but still works; it runs on a worker
thread, once per download.

## Apple Vision masks (`vision`, issue #115)

`vision::person_segmentation` (`VNGeneratePersonSegmentationRequest`, Accurate), `vision::person_instances`
(`VNGeneratePersonInstanceMaskRequest`, up to four people) and `vision::foreground_union`
(`VNGenerateForegroundInstanceMaskRequest`, every subject as one matte) take an 8-bit or float RGB `Image` and give
coverage mattes (0 to 1) at the photo's aspect; instance masks come at the photo's size. Instance masks are read one
at a time: the subject masks fold into one matte as they are read, and the per-person masks one call keeps add up to
at most 64 M pixels (an error past that). All `unsafe` is in `src/vision_macos.rs`;
off the Mac `src/stub.rs` says unsupported. The same rules as above:

- `vision::supported(request)` checks macOS 12 (segmentation) or 14 (instances) and, with `respondsToSelector:`,
  `initWithCVPixelBuffer:options:`, `performRequests:error:`, each request's `init` and `results`,
  `setQualityLevel:`, `setOutputPixelFormat:`,
  `VNPixelBufferObservation`'s `pixelBuffer`, `VNInstanceMaskObservation`'s `allInstances` and
  `generateScaledMaskForImageForInstances:fromRequestHandler:error:`, and `NSIndexSet`'s `firstIndex` and
  `indexGreaterThanIndex:`. Every entry point checks it first and the image's size (1 to 16,384 a side, 64 M pixels).
- The input is an IOSurface-backed 32-bit BGRA pixel buffer (the instance requests refuse any other). The float
  output format is set only when Vision lists it: `supportedOutputPixelFormatsAndReturnError:` is macOS 15, so it is
  not required, and is sent only where the request answers it (on macOS 14 Vision's own format is kept). Only instance indexes Vision listed are asked for (at most 16).
- Pixel buffers are read and written only while locked (unlocked on drop), row by row, after checking the format
  (one component: 8-bit, half or float), that the buffer is not planar, and that every row lies inside its size.

