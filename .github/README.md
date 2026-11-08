<p align="center">
  <img src="../assets/app-icon/hicolor/256x256/apps/ai.storyteller.lightcraft.png" width="128" height="128" alt="LightCraft icon: an engraved lynx on an amber tile">
</p>

<h1 align="center">LightCraft (techneissa's fork)</h1>

<p align="center">
  <b>A personal fork of <a href="https://github.com/storytold/lightcraft">LightCraft</a>, the open-source photo library and raw developer written in pure Rust.</b><br>
  The official app plus a few features of my own, built for macOS.
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Join the ArtCraft community on Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="https://getartcraft.com/apps/lightcraft"><b>LightCraft on getartcraft.com</b></a> ·
  <a href="https://github.com/storytold/lightcraft">Official repo</a> ·
  <a href="https://discord.gg/artcraft">Discord</a>
</p>

<p align="center">
  <img src="../docs/images/hero-tetons.jpg" alt="LightCraft's Edit view with The Tetons and the Snake River in the loupe and the Light and Effects panels open" width="100%">
</p>

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="../docs/images/curve-tetons.jpg" alt="The Tone Curve with a gentle S-curve on The Tetons and the Snake River" width="100%">
      <br><b>Tone Curve</b><br>
      Parametric and point curves, for RGB and each channel.
    </td>
    <td width="50%" valign="top">
      <img src="../docs/images/grading-migrant-mother.jpg" alt="Color Grading wheels for midtones, shadows and highlights next to Migrant Mother" width="100%">
      <br><b>Color Grading</b><br>
      Wheels for shadows, midtones and highlights.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="../docs/images/masking.jpg" alt="Masking panel with a linear Sky mask and a radial Sun glow mask on a lake scene" width="100%">
      <br><b>Masking</b><br>
      Brush, gradients, ranges and AI masks, each with its own adjustments.
    </td>
    <td width="50%" valign="top">
      <img src="../docs/images/presets-mixer.jpg" alt="Presets grouped by style beside the Color panel with the Color Mixer open" width="100%">
      <br><b>Presets and Color Mixer</b><br>
      One-click looks, and hue, saturation and luminance per colour.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="../docs/images/crop-tetons.jpg" alt="A 16 by 9 crop rotated slightly, with a rule-of-thirds overlay and the Geometry sliders" width="100%">
      <br><b>Crop and geometry</b><br>
      Crop, straighten and fix perspective.
    </td>
    <td width="50%" valign="top">
      <img src="../docs/images/bw-split.jpg" alt="Split before and after view of sand dunes: colour on the left, black and white on the right" width="100%">
      <br><b>Black &amp; White</b><br>
      B&amp;W mixes, grain and vignette, with a before/after split.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="../docs/images/grid-demo.jpg" alt="Photo Grid of demo photos with ratings and flags, and nested albums in the sidebar" width="100%">
      <br><b>Library</b><br>
      Albums, folders, ratings and flags.
    </td>
    <td width="50%" valign="top">
      <img src="../docs/images/info-earthrise.jpg" alt="Info panel for Earthrise with file, dimensions, rating, title and camera metadata" width="100%">
      <br><b>Info panel</b><br>
      File details, captions and camera metadata.
    </td>
  </tr>
</table>

<sub>Showcase photos are public domain (sources in the <a href="https://github.com/storytold/lightcraft#license-and-credits">official README</a>); the demo photos are made by LightCraft.</sub>

## What I added

**Develop**
- **macOS Colour:** macOS's own raw decoder makes a raw's colours; every slider still works.
- **Stronger sliders:** Contrast, Whites, Blacks, Shadows, Highlights, Sharpening and Vignette reach further.
- **⌥-drag previews:** hold ⌥ on a slider to see clipping, the sharpening mask or greyscale.
- **Clipping toggles:** the histogram's two triangles each switch their own overlay.
- **Camera profiles:** Camera Matching in the Profile picker, and your own profiles appear there.
- **Auto AI Denoise:** switches on once for raws at or above an ISO you choose.
- **Before/After split:** a line you can drag, and a Split button.
- **Sliders on one row:** label, groove and value on one line.
- **Arrange the Edit panel:** drag, hide or reset the Develop sections.

**Masks and AI**
- **Masking workflow:** Add, Subtract and Intersect, a button per kind of mask part.
- **Real Subject, Sky and Background:** proper SAM 3 selections.
- **BiRefNet masks:** fine-edged subject cut-outs.
- **Mask menu:** right-click to add any submask; edit several submasks at once.
- **Brush live preview:** see the mask's adjustment under the brush as you paint.
- **Find People and Find Wires:** outline and remove them in one click.
- **Reflection removal:** Remove ▸ Reflections takes a window's reflection out.
- **LaMa fill:** a second fill for Remove and Content-Aware spots.
- **Shared models:** every AI model is stored once, shared with PhotoCraft.

**Library**
- **Book:** lay out a photo book and export it as a PDF or JPEGs.
- **Map:** pins, drag to geotag, saved places and track logs.
- **Edit all selected:** edits apply to every selected photo.
- **Edit metadata:** change camera, lens and exposure values for one or many photos.
- **Quick filters:** stars and flags in the bottom bar.
- **Deleting:** Delete Rejected Photos, Trash for local files, ⌫ in Recently Deleted.
- **Renaming:** one way to rename everywhere; files move in the background.
- **Open as Layers in PhotoCraft:** send two or more photos to PhotoCraft as layers.
- **Export Animated GIF:** File ▸ Export Animated GIF, with numbered selection order.
- **Show in Finder after export.**

**Shortcuts, menus and settings**
- **Keyboard Shortcuts:** VS Code style, two keys per command, chords, conditions, one resizable window.
- **Command Palette:** on ⌘P.
- **Right-click menu:** arrange it, with presets and a backup.
- **Reorder the tool strip:** drag its icons.
- **Top bar items:** show or hide each one.
- **Searchable dropdowns:** type to filter an open list.
- **Settings sync:** move every setting to another Mac, or keep two in sync through a folder.
- **Hover help:** every icon button explains itself.
- **Settings:** a wider window with shorter text.
- **Larger file dialogs:** taller, resizable open and save windows.

**Fixes**
- Faster: less memory, fewer redraws, no UI freezes, sidecar writes in the background.
- No camera-JPEG flash while a raw renders.
- Sony A7 V colours and Mark II lens profiles fixed.
- Rejected photos look the same in every view.

## About this fork

- **Official code:** comes from [storytold/lightcraft](https://github.com/storytold/lightcraft) and is merged in often. For LightCraft itself (features, downloads, docs), see the official repo.
- **Branches:** `custom` (the default here) is the official `main` plus my features; `main` is a plain copy of the official one.
- **Mac only:** I build and use it on macOS. Other platforms may not build.
- **Full feature list:** run `fork/features.sh`, or read [features.md](../fork/features.md) and the pages in [`fork/`](../fork/).
- **How it works:** updating, building and adding a feature are in [FORK.md](../FORK.md).

Bugs in the official app belong in the [official issues](https://github.com/storytold/lightcraft/issues); this fork is a personal build.

## Build

```sh
git clone https://github.com/techneissa/lightcraft
cd lightcraft
cargo run --release -p lightcraft
```

## License

Dual-licensed under [MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE), like the official LightCraft. Notices are in [NOTICE](../NOTICE), bundled assets in [ATTRIBUTION.md](../assets/ATTRIBUTION.md).

## Star history (official repo)

[![Star History Chart](https://api.star-history.com/svg?repos=storytold/lightcraft&type=Date&legend=top-left)](https://www.star-history.com/?repos=storytold%2Flightcraft&type=date&legend=top-left)
