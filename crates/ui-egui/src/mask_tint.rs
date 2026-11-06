//! The mask overlay drawn over the photo when the loupe paints, not rendered into it.
//!
//! While a tint is shown, a job beside the photo's render answers with the alpha of the selected mask (or of every visible
//! one: [`MaskPlanes`], `RenderJob::for_mask_planes`). It stops before the photo is finished and finds the alphas the photo's
//! render kept, and it reads nothing back while the planes held here are still the right ones. From the plane and the view's
//! colour this makes a small premultiplied tint texture; the loupe draws it over the plain photo with the overlay's opacity as
//! the draw's tint. So `O`, the opacity slider, the colour and the view start no render of the photo, and with the overlay
//! off nothing is read back at all.
//!
//! Every view of the Overlay menu is the photo and one or two layers over it:
//! - Color Overlay: the colour at the mask's alpha, at the opacity.
//! - Color Overlay on B&W: the same over the photo in grey.
//! - Image on Black / on White: black / white at what the mask leaves out.
//! - White on Black: black over the photo, white at the mask's alpha.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, ColorImage, Painter, Rect, TextureHandle, TextureOptions, pos2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::media::MaskPlanes;
use lightcraft_pipeline::MaskView;
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk};

use crate::render::{Slot, Tex};

/// The mask whose alpha is tinted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The mask with this [`lightcraft_develop::Mask::id`].
    One(u32),
    /// Every visible mask at once.
    All,
}

/// What to draw: the mask, how, in which colour, at which opacity (0..100).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TintSel {
    pub pick: Pick,
    pub view: MaskView,
    pub color: [u8; 3],
    pub opacity: u8,
}

/// The planes held for a slot, and what they were asked for.
struct Held {
    photo: PhotoId,
    planes: Arc<MaskPlanes>,
    pick: Pick,
    /// The key of the photo render the planes were last found to be the right ones for.
    for_key: u64,
}

/// The last ask made for a slot (not made twice).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Asked {
    photo: PhotoId,
    /// The key of the photo render the ask was made beside.
    base: u64,
    pick: Pick,
    /// The key of the job that answers.
    job: u64,
}

/// The tint made for a slot's texture.
struct Built {
    /// The planes it was made from.
    signature: u64,
    pick: Pick,
    view: MaskView,
    color: [u8; 3],
    tint: TextureHandle,
    /// The photo in grey, under the tint of Color Overlay on B&W.
    grey: Option<TextureHandle>,
}

/// The slots that can carry a tint: the whole-frame loupe and its zoom window.
fn tinted(slot: Slot) -> bool {
    matches!(slot, Slot::Main | Slot::Region)
}

/// The masks' planes the answers brought, and the tint textures made from them.
#[derive(Default)]
pub struct MaskTints {
    held: HashMap<Slot, Held>,
    asked: HashMap<Slot, Asked>,
    built: HashMap<Slot, Built>,
    sel: Option<TintSel>,
    /// Keep the zoom window's pixels (Color Overlay on B&W makes its grey from them).
    pub keep_region: bool,
    /// Tint textures made so far (tests count them: a toggle or an opacity change makes none).
    #[cfg(test)]
    pub(crate) builds: usize,
    /// Answers to asks that came, and the ones that brought planes (tests: what showing, hiding and dragging ask for).
    #[cfg(test)]
    pub(crate) answers: usize,
    #[cfg(test)]
    pub(crate) delivered: usize,
}

impl MaskTints {
    /// The ask to make beside the photo render `base` (of `photo`) in `slot`, if it is not made already: the planes of `pick`,
    /// and the signature of the ones held, which need not come again.
    pub fn ask(&mut self, slot: Slot, photo: PhotoId, base: u64, pick: Pick) -> Option<PlanesAsk> {
        if !tinted(slot) {
            return None;
        }
        let held = self.held.get(&slot).filter(|h| h.photo == photo && h.pick == pick);
        // (held for this very render: nothing to ask)
        if held.is_some_and(|h| h.for_key == base) || self.asked.get(&slot).is_some_and(|a| (a.photo, a.base, a.pick) == (photo, base, pick)) {
            return None;
        }
        Some(PlanesAsk {
            want: match pick {
                Pick::One(id) => MaskWant::One(id),
                Pick::All => MaskWant::All,
            },
            known: held.map(|h| h.planes.signature),
        })
    }

    /// The ask was made as job `job`.
    pub fn asked(&mut self, slot: Slot, photo: PhotoId, base: u64, pick: Pick, job: u64) {
        self.asked.insert(slot, Asked { photo, base, pick, job });
    }

    /// The answer to the ask of `slot` (job `job`) came.
    pub fn note(&mut self, slot: Slot, job: u64, planes: Option<Arc<MaskPlanes>>) {
        #[cfg(test)]
        {
            self.answers += 1;
            self.delivered += usize::from(planes.as_ref().is_some_and(|p| !p.unchanged));
        }
        let Some(asked) = self.asked.get(&slot).copied().filter(|a| a.job == job) else { return };
        let Some(planes) = planes else { return };
        if planes.unchanged {
            // (still the planes it holds: valid for the render the ask was made beside)
            if let Some(h) = self.held.get_mut(&slot).filter(|h| h.planes.signature == planes.signature && h.photo == asked.photo) {
                h.for_key = asked.base;
            }
            return;
        }
        self.held.insert(slot, Held { photo: asked.photo, planes, pick: asked.pick, for_key: asked.base });
    }

    /// `slot` has no texture any more.
    pub fn release(&mut self, slot: Slot) {
        self.held.remove(&slot);
        self.asked.remove(&slot);
        self.built.remove(&slot);
    }

    /// Let go of everything (the loupe shows no masks).
    pub fn clear(&mut self) {
        self.held.clear();
        self.asked.clear();
        self.built.clear();
        self.sel = None;
        self.keep_region = false;
    }

    /// The planes held for the slot.
    #[cfg(test)]
    pub(crate) fn planes(&self, slot: Slot) -> Option<&Arc<MaskPlanes>> {
        self.held.get(&slot).map(|h| &h.planes)
    }

    /// Whether planes are held for the slot.
    pub fn has_masks(&self, slot: Slot) -> bool {
        self.held.contains_key(&slot)
    }

    /// Memory held (bytes): the planes and the tint textures.
    pub fn bytes(&self) -> usize {
        let planes: usize = self.held.values().map(|h| h.planes.bytes()).sum();
        let tints: usize = self.built.values().map(|b| b.tint.byte_size() + b.grey.as_ref().map_or(0, TextureHandle::byte_size)).sum();
        planes + tints
    }

    /// What the frame draws (`None`: hidden). Makes the tint textures it needs that aren't made yet.
    pub fn show(&mut self, ctx: &egui::Context, textures: &HashMap<Slot, Tex>, sel: Option<TintSel>) {
        self.sel = sel;
        let Some(sel) = sel else { return };
        for slot in [Slot::Main, Slot::Region] {
            let (Some(tex), Some(held)) = (textures.get(&slot), self.held.get(&slot)) else { continue };
            // (planes of another mask than the one selected now wait for their replacement)
            if held.photo != tex.photo || held.pick != sel.pick {
                continue;
            }
            let signature = held.planes.signature;
            let fresh = |b: &Built| b.signature == signature && b.pick == sel.pick && b.view == sel.view && b.color == sel.color;
            if self.built.get(&slot).is_some_and(fresh) {
                // (the grey under Color Overlay on B&W, once the photo's pixels are there)
                if sel.view == MaskView::ColorOnBw
                    && let Some(b) = self.built.get_mut(&slot).filter(|b| b.grey.is_none())
                {
                    b.grey = tex.pixels.as_ref().map(|p| ctx.load_texture(format!("{slot:?} grey"), grey_of(p), TextureOptions::LINEAR));
                }
                continue;
            }
            let Some(image) = tint_image(&held.planes, sel) else {
                self.built.remove(&slot);
                continue;
            };
            let tint = ctx.load_texture(format!("{slot:?} tint"), image, TextureOptions::LINEAR);
            let grey = (sel.view == MaskView::ColorOnBw)
                .then(|| tex.pixels.as_ref().map(|p| ctx.load_texture(format!("{slot:?} grey"), grey_of(p), TextureOptions::LINEAR)))
                .flatten();
            #[cfg(test)]
            {
                self.builds += 1;
            }
            self.built.insert(slot, Built { signature, pick: sel.pick, view: sel.view, color: sel.color, tint, grey });
        }
    }

    /// Whether `slot`'s texture (the render `key`, of `photo`) has its tint drawn by [`Self::paint`]. The whole frame takes the
    /// planes it has (a late answer leaves the last tint up); a window only its own (it is cut from where it is).
    pub fn drawn(&self, slot: Slot, photo: PhotoId, key: u64) -> bool {
        let (Some(sel), Some(held), Some(b)) = (self.sel, self.held.get(&slot), self.built.get(&slot)) else { return false };
        held.photo == photo
            && b.signature == held.planes.signature
            && b.pick == sel.pick
            && b.view == sel.view
            && b.color == sel.color
            && (slot == Slot::Main || held.for_key == key)
    }

    /// Draw the tint of `slot`'s render `key` (of `photo`) into `dst`, over the photo drawn there.
    pub fn paint(&self, p: &Painter, slot: Slot, photo: PhotoId, key: u64, dst: Rect) {
        let Some(sel) = self.sel.filter(|_| self.drawn(slot, photo, key)) else { return };
        let Some(b) = self.built.get(&slot) else { return };
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        match (sel.view, &b.grey) {
            (MaskView::ColorOnBw, Some(grey)) => {
                p.image(grey.id(), dst, uv, Color32::WHITE);
            }
            (MaskView::WhiteOnBlack, _) => {
                p.rect_filled(dst, 0.0, Color32::BLACK);
            }
            _ => {}
        }
        // (only the colour views take the opacity)
        let alpha = match sel.view {
            MaskView::Color | MaskView::ColorOnBw => (f32::from(sel.opacity.min(100)) * 2.55).round() as u8,
            _ => 255,
        };
        p.image(b.tint.id(), dst, uv, Color32::from_white_alpha(alpha));
    }
}

/// `a`'s share of `c` (0..255 each), rounded.
fn scale(c: u8, a: u8) -> u8 {
    ((u16::from(c) * u16::from(a) + 127) / 255) as u8
}

/// The premultiplied tint of the selected alpha for the view, the size of the render (`None`: no such mask).
fn tint_image(planes: &MaskPlanes, sel: TintSel) -> Option<ColorImage> {
    let alpha: std::borrow::Cow<'_, [u8]> = match sel.pick {
        Pick::One(id) => planes.plane(id)?.into(),
        Pick::All => planes.union().into(),
    };
    if alpha.len() != planes.width.checked_mul(planes.height)? || alpha.is_empty() {
        return None;
    }
    let [r, g, b] = sel.color;
    let pixels = alpha
        .iter()
        .map(|&a| match sel.view {
            MaskView::Color | MaskView::ColorOnBw => Color32::from_rgba_premultiplied(scale(r, a), scale(g, a), scale(b, a), a),
            MaskView::ImageOnBlack => Color32::from_rgba_premultiplied(0, 0, 0, 255 - a),
            MaskView::ImageOnWhite => Color32::from_rgba_premultiplied(255 - a, 255 - a, 255 - a, 255 - a),
            MaskView::WhiteOnBlack => Color32::from_rgba_premultiplied(a, a, a, a),
        })
        .collect();
    Some(ColorImage { size: [planes.width, planes.height], source_size: egui::vec2(planes.width as f32, planes.height as f32), pixels })
}

/// The photo in grey, as the overlay's Grayscale view makes it (Rec. 709 luma of the 8-bit values).
fn grey_of(img: &ColorImage) -> ColorImage {
    let pixels = img
        .pixels
        .iter()
        .map(|p| {
            let g = (0.2126 * f32::from(p.r()) + 0.7152 * f32::from(p.g()) + 0.0722 * f32::from(p.b())).round().clamp(0.0, 255.0) as u8;
            Color32::from_rgb(g, g, g)
        })
        .collect();
    ColorImage { size: img.size, source_size: img.source_size, pixels }
}
