//! Rows through a whole [`RenderJob`], as the loupe's worker runs it at rest (not just `develop`):
//! `Quality::Full`, the display fit (Lanczos), the view cache, the histogram, and the pixels read
//! back to the CPU. Also masks that run on the CPU inside a GPU render.
//!
//! The job is built by a demo session's `loupe_job`, with the benchmark's picture as its source
//! and the settings of the row, so the keys, the view cache and the stage cache are the app's.
//! The texture upload and the `egui` colour conversion that follow in the window are not here.

use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_engine::Session;
use lightcraft_engine::media::{DecodedSource, RenderJob, SourceRef, fit_for_display};
use lightcraft_geom::Point;
use lightcraft_pipeline::{MaskView, Overlay, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;

use super::{best, push_stroke};

/// What the loupe rows render.
pub struct Loupe {
    /// The loupe's whole-frame render size (`LOUPE=3500x2200`; default 1920×1280).
    pub size: [usize; 2],
    /// The size it is drawn at: the render, a little smaller (the render is rounded up to a step).
    pub fit: [usize; 2],
    template: RenderJob,
}

impl Loupe {
    /// A job template for `source` (a loaded picture) at `LOUPE` size.
    pub fn new(source: &Arc<Rgb32f>, info: &SourceInfo) -> Option<Loupe> {
        let size: [usize; 2] = std::env::var("LOUPE")
            .ok()
            .and_then(|v| {
                let (w, h) = v.split_once('x')?;
                Some([w.trim().parse().ok()?, h.trim().parse().ok()?])
            })
            .unwrap_or([1920, 1280]);
        let fit = [size[0].saturating_sub(32).max(8), (size[1] * size[0].saturating_sub(32).max(8) / size[0].max(1)).max(8)];
        let mut session = Session::with_demo();
        let id = session.active()?;
        let mut template = session.loupe_job(id, size[0], size[1], true)?;
        template.source = SourceRef::Loaded(Box::new(DecodedSource::new(source.clone(), Some(info.clone()))));
        template.info = info.clone();
        Some(Loupe { size, fit, template })
    }

    /// The loupe's job at rest for `settings`: full quality, view cache, display fit, `stages`
    /// kept from one render to the next.
    fn at_rest(&self, settings: &DevelopSettings, stages: &Arc<StageCache>) -> RenderJob {
        let mut job = self.template.clone();
        job.settings = Arc::new(settings.clone());
        job.stages = Some(stages.clone());
        job.with_display_fit(self.fit[0], self.fit[1])
    }

    /// [`Self::at_rest`] with a mask overlay.
    fn with_overlay(&self, settings: &DevelopSettings, stages: &Arc<StageCache>) -> RenderJob {
        let overlay = Overlay::AllMasks { view: MaskView::Color, color: [255, 40, 40], opacity: 50 };
        self.at_rest(settings, stages).with_overlay(overlay)
    }
}

/// Run `job`; the result's image size says it rendered.
fn run(job: RenderJob) -> usize {
    let r = job.run();
    r.rendered.map_or(0, |r| r.image.width)
}

/// How the rows are run and printed.
pub struct Out<'a> {
    /// Runs per row (the minimum is reported).
    pub n: usize,
    /// `false`: the CPU pipeline, `true`: the GPU one.
    pub backends: &'a [bool],
    /// Whether the rows named so are wanted (`ONLY`).
    pub run_row: &'a dyn Fn(&str) -> bool,
    /// Formats the cpu and gpu times.
    pub columns: &'a dyn Fn(&[f64]) -> String,
}

/// Rows for `count` strokes: a commit, the tick that follows a drag, an overlay flip, and the
/// full-coverage render the brush tool asks for.
pub fn count_rows(loupe: &Loupe, out: &Out<'_>, base: &DevelopSettings, extra: &[BrushStroke], count: usize) {
    let Out { n, backends, run_row, columns } = *out;
    let per_backend = |f: &dyn Fn(bool, &Arc<StageCache>) -> f64| -> Vec<f64> {
        let ms = backends
            .iter()
            .map(|&g| {
                lightcraft_engine::gpu::set_enabled(g);
                f(g, &Arc::new(StageCache::default()))
            })
            .collect();
        lightcraft_engine::gpu::set_enabled(true);
        ms
    };
    if run_row("loupe settle") {
        // the render after a slider stops: Exposure moved, every other stage warm
        let ms = per_backend(&|_, stages| {
            std::hint::black_box(run(loupe.at_rest(base, stages)));
            let mut k = 0.0;
            best(n, || {
                let mut t = base.clone();
                k += 1.0;
                t.light.exposure = 0.2 + k * 0.01;
                std::hint::black_box(run(loupe.at_rest(&t, stages)));
            })
        });
        println!("{count:>4} strokes  loupe settle    {}", columns(&ms));
    }
    if run_row("loupe commit") {
        // a committed stroke at rest, then undone: two renders, the first with the new stroke
        let ms = per_backend(&|_, stages| {
            std::hint::black_box(run(loupe.at_rest(base, stages)));
            let mut k = 0;
            best(n, || {
                let mut t = base.clone();
                if let Some(s) = extra.get(k % extra.len().max(1)) {
                    push_stroke(&mut t, s.clone());
                }
                k += 1;
                std::hint::black_box(run(loupe.at_rest(&t, stages)));
                std::hint::black_box(run(loupe.at_rest(base, stages)));
            })
        });
        println!("{count:>4} strokes  loupe commit+undo {}", columns(&ms));
    }
    if run_row("loupe overlay") {
        let ms = per_backend(&|_, stages| {
            std::hint::black_box(run(loupe.at_rest(base, stages)));
            let mut shown = false;
            best(2 * n, || {
                shown = !shown;
                let job = if shown { loupe.with_overlay(base, stages) } else { loupe.at_rest(base, stages) };
                std::hint::black_box(run(job));
            })
        });
        println!("{count:>4} strokes  loupe overlay flip {}", columns(&ms));
    }
    if run_row("loupe brush target") {
        // the brush tool's second render: the photo with the mask's brush covering everything
        let covered = covering(base);
        let ms = per_backend(&|_, stages| {
            std::hint::black_box(run(loupe.at_rest(&covered, stages)));
            let mut k = 0.0;
            best(n, || {
                let mut t = covered.clone();
                k += 1.0;
                t.light.exposure = 0.2 + k * 0.01;
                std::hint::black_box(run(loupe.at_rest(&t, stages)));
            })
        });
        println!("{count:>4} strokes  loupe brush target {}", columns(&ms));
    }
}

/// `base` with the first mask's first brush part replaced by one stroke over the whole frame.
fn covering(base: &DevelopSettings) -> DevelopSettings {
    let mut t = base.clone();
    if let Some(MaskShape::Brush { strokes }) = t.masks.first_mut().and_then(|m| m.components.first_mut()).map(|c| &mut c.shape) {
        let cover = BrushStroke { points: vec![Point::new(0.5, 0.33)], size: 2.0, ..BrushStroke::default() };
        *strokes = vec![cover];
    }
    t
}

/// What the loupe's job costs step by step at rest: the render with pixels read back, then the
/// view cache, then the display fit; and the fit alone on the result.
pub fn pieces(loupe: &Loupe, out: &Out<'_>, base: &DevelopSettings) {
    let Out { n, backends, columns, .. } = *out;
    println!("\n== loupe job at rest, step by step ({}×{}, drawn {}×{})", loupe.size[0], loupe.size[1], loupe.fit[0], loupe.fit[1]);
    let steps: [(&str, fn(&mut RenderJob)); 4] = [
        ("render + pixels read back", |j| {
            j.view_cache = None;
            j.display_fit = None;
        }),
        ("+ view cache", |j| j.display_fit = None),
        ("+ display fit (the loupe at rest)", |_| {}),
        ("draft (a drag tick)", |j| {
            *j = j.clone().draft();
        }),
    ];
    for (label, set) in steps {
        let ms: Vec<f64> = backends
            .iter()
            .map(|&g| {
                lightcraft_engine::gpu::set_enabled(g);
                let stages = Arc::new(StageCache::default());
                let mut k = 0.0;
                let mut tick = |t: &mut DevelopSettings| {
                    k += 1.0;
                    t.light.exposure = 0.2 + k * 0.01;
                };
                let mut warm = base.clone();
                tick(&mut warm);
                let mut job = loupe.at_rest(&warm, &stages);
                set(&mut job);
                std::hint::black_box(run(job));
                best(n, || {
                    let mut t = base.clone();
                    tick(&mut t);
                    let mut job = loupe.at_rest(&t, &stages);
                    set(&mut job);
                    std::hint::black_box(run(job));
                })
            })
            .collect();
        println!("{label:<36} {}", columns(&ms));
    }
    lightcraft_engine::gpu::set_enabled(true);
    // the fit alone, on a loupe-sized picture
    let img =
        lightcraft_raster::Rgba8 { width: loupe.size[0], height: loupe.size[1], data: vec![[90, 120, 150, 255]; loupe.size[0] * loupe.size[1]] };
    let ms = best(n, || drop(std::hint::black_box(fit_for_display(&img, loupe.fit[0], loupe.fit[1]))));
    println!("{:<36} {ms:8.1} ms (CPU)", "display fit alone (Lanczos)");
}

/// A stored segmentation: an ellipse of logits on a `side × side` grid, centred at `(cx, cy)`.
fn blob(side: usize, cx: f32, cy: f32, rx: f32, ry: f32) -> SegMask {
    let mut logits = Vec::with_capacity(side * side);
    for y in 0..side {
        for x in 0..side {
            let (u, v) = ((x as f32 + 0.5) / side as f32 - cx, (y as f32 + 0.5) / side as f32 - cy);
            let d = (u / rx).hypot(v / ry);
            logits.push((1.0 - d) * 12.0);
        }
    }
    SegMask::from_logits(side, &logits)
}

/// One mask of each kind that runs on the CPU in a GPU render, each with an exposure effect:
/// (name, settings).
fn cpu_masks() -> Vec<(&'static str, DevelopSettings)> {
    let adjust = LocalAdjustments { exposure: 0.5, contrast: 15.0, ..LocalAdjustments::default() };
    let part = |shape| MaskComponent { name: None, op: MaskOp::Add, invert: false, shape };
    let seg = blob(512, 0.5, 0.33, 0.3, 0.2);
    let make = |name: &'static str, shape: MaskShape, refine: f64| {
        let mut s = DevelopSettings::default();
        s.light.exposure = 0.2;
        s.masks = vec![Mask { id: 1, components: vec![part(shape)], adjust, refine, ..Mask::default() }];
        (name, s)
    };
    vec![
        make(
            "radial (GPU kernel)",
            MaskShape::Radial { center: Point::new(0.5, 0.33), rx: 0.3, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
            0.0,
        ),
        make("subject (stored segmentation)", MaskShape::Subject { seg: Some(seg.clone()), detail: Vec::new(), edge: 0.0 }, 0.0),
        make(
            "object (stored segmentation)",
            MaskShape::Object { hint: vec![Point::new(0.5, 0.33)], exclude: Vec::new(), seg: Some(seg.clone()), detail: Vec::new(), edge: 0.0 },
            0.0,
        ),
        make("sky (stored segmentation)", MaskShape::Sky { seg: Some(seg), detail: Vec::new(), edge: 0.0 }, 0.0),
        make(
            "refine edges (radial, 60)",
            MaskShape::Radial { center: Point::new(0.5, 0.33), rx: 0.3, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
            60.0,
        ),
        make(
            "color range (3 samples)",
            MaskShape::ColorRange { samples: vec![[0.5, 0.02, 0.02], [0.45, 0.0, 0.05], [0.6, -0.02, 0.0]], refine: 50.0 },
            0.0,
        ),
    ]
}

/// Slider ticks on a photo with one mask of a kind that runs on the CPU, through the loupe's job:
/// the mask's own exposure (its alpha is unchanged) and a global exposure tick (a range mask's
/// alpha reads the exposure). The first render of each is timed too (the alpha built once).
pub fn cpu_mask_rows(loupe: &Loupe, out: &Out<'_>) {
    let Out { n, backends, columns, .. } = *out;
    println!("\n== masks that run on the CPU inside a render, loupe job at rest");
    for (name, base) in cpu_masks() {
        for (label, control) in [("first render", 0), ("mask exposure tick", 1), ("global exposure tick", 2)] {
            let ms: Vec<f64> = backends
                .iter()
                .map(|&g| {
                    lightcraft_engine::gpu::set_enabled(g);
                    let stages = Arc::new(StageCache::default());
                    if control != 0 {
                        std::hint::black_box(run(loupe.at_rest(&base, &stages)));
                    }
                    let mut k = 0.0;
                    best(n, || {
                        let mut t = base.clone();
                        k += 1.0;
                        let stages = if control == 0 { Arc::new(StageCache::default()) } else { stages.clone() };
                        match control {
                            1 => {
                                if let Some(m) = t.masks.first_mut() {
                                    m.adjust.exposure = 0.5 + 0.02 * k;
                                }
                            }
                            2 => t.light.exposure = 0.2 + 0.01 * k,
                            _ => {}
                        }
                        std::hint::black_box(run(loupe.at_rest(&t, &stages)));
                    })
                })
                .collect();
            println!("{name:<32} {label:<22} {}", columns(&ms));
        }
    }
    lightcraft_engine::gpu::set_enabled(true);
}
