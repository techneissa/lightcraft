use super::{Color32, LightcraftApp, PhotoId, Rect, Slot, pos2, vec2};

/// Whether an overlay can be rendered on a region tile without changing what the tile means. Mask (one or all)
/// overlays are evaluated from the same mask alpha as the full render; Point Color range and Visualize
/// HDR act pixel by pixel (the HDR rendition is rendered for the same window); Visualize Spots' high
/// pass reads about 3σ = 1.8 % of the long edge, inside the window's margin
/// ([`crate::region::margin_for`]: 4.5 %). Each is tested equal to the whole-frame render's pixels.
pub(crate) fn overlay_supports_window(overlay: lightcraft_pipeline::Overlay) -> bool {
    use lightcraft_pipeline::Overlay as O;
    match overlay {
        O::None | O::Mask { .. } | O::AllMasks { .. } | O::PointColorRange(_) | O::HdrRange | O::Spots(_) => true,
        // clipping and greyscale previews act pixel by pixel on the finished image; a Portrait area map is made for
        // each pixel from the faces, through the window's own view
        o if o.per_pixel() => true,
        _ => false,
    }
}

/// What a window render of the loupe needs to know about the view it belongs to.
pub(super) struct WindowCtx {
    pub(super) id: PhotoId,
    pub(super) overlay: lightcraft_pipeline::Overlay,
    /// The tint drawn over the After window ([`crate::mask_tint`]).
    pub(super) tint: Option<crate::mask_tint::TintSel>,
    /// Long edge of the frame the windows are cut from (`None`: no windows).
    pub(super) frame_edge: Option<usize>,
    pub(super) drawn_long: f32,
    pub(super) aspect: f32,
    pub(super) ppp: f32,
    pub(super) texture_side: usize,
    pub(super) crop_tool: bool,
    pub(super) interacting: bool,
    /// Hash of the photo's develop settings.
    pub(super) look: u64,
    /// A pinch or two-finger scroll is running: keep the windows there are.
    pub(super) holding: bool,
}

/// One side of the loupe that gets a window render.
#[derive(Clone, Copy)]
pub(super) struct WindowView {
    pub(super) slot: Slot,
    /// The Before side (the photo without its edits).
    pub(super) before: bool,
    /// The rect the photo is drawn in.
    pub(super) img: Rect,
    /// The rect it is requested for (the drawn rect without a click-zoom animation).
    pub(super) target: Rect,
    /// The area whose pixels are on screen.
    pub(super) visible: Rect,
}

/// Ask for the window of `v` that holds what is on screen; remember it so its texture can be
/// drawn where it belongs. `None`: no window is wanted (or possible) for this view.
pub(super) fn request_window(app: &mut LightcraftApp, c: &WindowCtx, v: &WindowView) -> Option<crate::region::RegionView> {
    let frame_edge = c.frame_edge?;
    // while a pinch or scroll runs the windows there are keep being drawn, magnified: no new one
    // per frame (zooming out would ask for ever wider ones)
    if c.holding {
        return if v.before { app.region_before_view } else { app.region_view }.filter(|w| w.photo == c.id);
    }
    let (fw, fh) =
        if c.aspect >= 1.0 { (frame_edge as f32, frame_edge as f32 / c.aspect) } else { (frame_edge as f32 * c.aspect, frame_edge as f32) };
    let (fw, fh) = (fw.round().max(1.0) as usize, fh.round().max(1.0) as usize);
    // what is on screen, in pixels of the window's frame (the drawn frame, scaled)
    let to_px = c.ppp * frame_edge as f32 / c.drawn_long.max(1.0);
    let visible = (
        to_px * (v.visible.left() - v.target.left()),
        to_px * (v.visible.top() - v.target.top()),
        to_px * (v.visible.right() - v.target.left()),
        to_px * (v.visible.bottom() - v.target.top()),
    );
    // a window already rendered of this frame that holds the view is drawn again (#174); else the one for the view
    let win = match held_window(app, c.id, v, (fw, fh), visible) {
        Some(win) => win,
        None => crate::region::window_for(fw, fh, visible, crate::region::max_span(c.texture_side))?,
    };
    // a window waits for the original another window job is decoding (see `defer_window`)
    let original_held = app.session.media.has_source(c.id, lightcraft_engine::SourceLevel::for_size(fw.max(fh)));
    let window_pending = app.renderer.is_pending(Slot::Region) || app.renderer.is_pending(Slot::RegionBefore);
    if crate::region::defer_window(window_pending, original_held) {
        return if v.before { app.region_before_view } else { app.region_view }.filter(|w| w.photo == c.id);
    }
    let job = if v.before {
        app.session.region_job_before(c.id, fw, fh, win, !c.crop_tool)
    } else {
        app.session.region_job(c.id, fw, fh, win, !c.crop_tool)
    };
    // refused: what the window reads (a spot's source, an Auto Mask stroke) is too far for one
    // render, so the whole frame is rendered at the drawn size instead (see `region::plan`)
    let Some(job) = job else {
        app.window_refused = Some((c.id, c.look, frame_edge));
        return None;
    };
    // a drag renders drafts of the window, as it does of the whole frame. The Before tile stays
    // plain; the After tile carries the same mask overlay as the whole-frame render.
    let job = if c.interacting { job.draft() } else { job };
    let overlay = if v.before { lightcraft_pipeline::Overlay::None } else { c.overlay };
    let mut job = job;
    super::give_area_faces(app, c.id, overlay, &mut job);
    let job = if v.before { job } else { job.with_hidden_grading(app.ui.grading_hidden_for(c.id)) };
    // the After window is proofed like the whole-frame render under it; the Before side is
    // never proofed (its whole-frame render isn't either)
    let proof = app.ui.soft_proof.then_some(app.ui.proof);
    let job = job.with_overlay(overlay).with_proof(if v.before { None } else { proof });
    // keyed as the renderer keys the texture (a display profile changes the key)
    let job = app.renderer.job_for(v.slot, job);
    // The overlay and the proof are part of the tile's identity too, so changing mask view/colour
    // or the proof never leaves a stale region tile on screen while its replacement renders.
    let proof_key = if v.before { 0 } else { proof.map_or(0, |p| p.key()) };
    // (the photo's remembered hash, with the wheels hidden for viewing: hashing the settings every frame writes every
    // stroke of every mask to JSON. The Before side renders other settings, hashed here: it is only on screen on request)
    let (look, shown_as) = if v.before {
        let shown_as = overlay.key().rotate_left(17) ^ proof_key.rotate_left(41);
        (job.settings.hash64() ^ shown_as, shown_as)
    } else {
        after_look(app, c)
    };
    let view = crate::region::RegionView { photo: c.id, before: v.before, key: job.key, full: (fw, fh), window: win, settings: look, view: shown_as };
    // windows of other photos, looks and zooms of this side can't be drawn any more; the one on screen stays while the
    // window of a new edit renders
    let on_screen = app.renderer.textures.get(&v.slot).map(|t| t.key);
    app.region_tiles.retain(|(before, key), t| {
        *before != v.before || t.is_current(c.id, (fw, fh), look, c.interacting) || (Some(*key) == on_screen && t.stands_in(c.id, (fw, fh), shown_as))
    });
    app.region_tiles.insert((v.before, job.key), view);
    if !v.before {
        super::request_planes(app, (Slot::RegionAlpha, Slot::Region), &job, c.tint);
    }
    app.renderer.request(v.slot, job, 99);
    Some(view)
}

/// The look of the After window ([`crate::region::RegionView::settings`]) and the part of it that is how the photo
/// is viewed (`view`): the photo's remembered settings hash with the wheels hidden for viewing (hashing the settings
/// every frame writes every stroke of every mask to JSON), the mask overlay and the soft proof.
pub(super) fn after_look(app: &LightcraftApp, c: &WindowCtx) -> (u64, u64) {
    let hidden = app.ui.grading_hidden_for(c.id);
    let settings = c.look ^ hidden.iter().enumerate().fold(0u64, |b, (i, h)| b | ((*h as u64) << i)).wrapping_mul(0xd6e8_feb8_6659_fd93);
    let proof_key = app.ui.soft_proof.then_some(app.ui.proof).map_or(0, |p| p.key());
    let shown_as = c.overlay.key().rotate_left(17) ^ proof_key.rotate_left(41);
    (settings ^ shown_as, shown_as)
}

/// The window of a tile held for `v`'s side of photo `id` in a frame of `full` px that still covers `visible`
/// ([`crate::region::covers`]): the one on screen first. Its look may be older (an edit): the same window is then
/// rendered again, with the stage caches it left.
fn held_window(
    app: &LightcraftApp,
    id: PhotoId,
    v: &WindowView,
    full: (usize, usize),
    visible: (f32, f32, f32, f32),
) -> Option<lightcraft_pipeline::PixelWindow> {
    let on_screen = app.renderer.textures.get(&v.slot).filter(|t| t.photo == id).map(|t| t.key);
    let mut held: Vec<(&(bool, u64), &crate::region::RegionView)> = app
        .region_tiles
        .iter()
        .filter(|((before, _), t)| *before == v.before && t.photo == id && t.full == full && crate::region::covers(&t.window, full, visible))
        .collect();
    // (the tile on screen, then the smallest: a stable choice whatever the map's order)
    held.sort_by_key(|((_, key), t)| (Some(*key) != on_screen, t.window.w * t.window.h, t.window.x, t.window.y));
    held.first().map(|(_, t)| t.window)
}

/// Draw `v`'s window texture over the whole-frame render, if it is the one asked for (or, in a
/// drag, an earlier draft of it).
pub(super) fn draw_window(p: &egui::Painter, app: &LightcraftApp, c: &WindowCtx, v: &WindowView, wanted: &crate::region::RegionView) -> bool {
    let Some(tex) = app.renderer.textures.get(&v.slot).filter(|t| t.photo == c.id) else { return false };
    let pending = app.renderer.is_pending(v.slot);
    let fits = |t: &&crate::region::RegionView| {
        t.is_current(c.id, wanted.full, wanted.settings, c.interacting) || (pending && t.stands_in(c.id, wanted.full, wanted.view))
    };
    let Some(tile) = app.region_tiles.get(&(v.before, tex.key)).filter(fits) else {
        return false;
    };
    let (fw, fh) = (tile.full.0 as f32, tile.full.1 as f32);
    let at = v.img.min + vec2(tile.window.x as f32 / fw * v.img.width(), tile.window.y as f32 / fh * v.img.height());
    let dst = Rect::from_min_size(at, vec2(tile.window.w as f32 / fw * v.img.width(), tile.window.h as f32 / fh * v.img.height()));
    p.image(tex.id(), dst, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    // the mask overlay over the window, from the masks this window's render came with
    if app.renderer.tints.drawn(v.slot, c.id, tex.key) {
        app.renderer.tints.paint(p, v.slot, c.id, tex.key, dst);
    } else if !v.before
        && let Some(main) = app.renderer.textures.get(&Slot::Main).filter(|t| t.photo == c.id)
    {
        // (until the window's own planes come: the whole frame's, magnified, over the part of it the window covers)
        app.renderer.tints.paint(&p.with_clip_rect(dst.intersect(p.clip_rect())), Slot::Main, c.id, main.key, v.img);
    }
    true
}
