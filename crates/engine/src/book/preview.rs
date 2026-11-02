//! One page of a book as screen pixels, for the full-screen preview.
//!
//! The page is composed by the export's own code ([`render::prepare`] with the book cut down to
//! that one page), so the preview shows the layout, the crops and the colour that print; only the
//! resolution differs: the page's long side is `long_edge` pixels (kept between the Print module's
//! lowest and highest resolution). A page with no photo is plain paper.
use super::Book;
use super::model::{MAX_PPI, MIN_PPI};
use super::render::{self, BookJob};
use crate::Session;
use crate::print::output::page_to_srgb8;

/// Largest long side a preview page is asked for, pixels.
pub const MAX_EDGE: usize = 4096;

/// A page as 8-bit sRGB, four bytes (red, green, blue, opaque) a pixel, rows from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct PagePixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

enum Kind {
    /// No photo on the page: its background.
    Blank {
        width: usize,
        height: usize,
        background: [u8; 3],
    },
    Render(Box<BookJob>),
}

/// A page set up for rendering on another thread.
pub struct PageJob(Kind);

/// The pixels per inch that make a page of `inches` (long side) `long_edge` pixels across.
fn ppi_for(long_edge: usize, inches: f64) -> f32 {
    let ppi = long_edge.clamp(16, MAX_EDGE) as f64 / inches.max(0.1);
    (ppi as f32).clamp(MIN_PPI, MAX_PPI)
}

/// Sets page `page` of `book` up to render with its long side near `long_edge` pixels. Fails when
/// the page doesn't exist or a photo on it is gone from the catalog.
pub fn prepare_page(session: &mut Session, book: &Book, page: usize, long_edge: usize) -> Result<PageJob, String> {
    let p = book.pages.get(page).ok_or_else(|| format!("the book has no page {}", page + 1))?;
    let (pw, ph) = book.settings.page_size()?;
    let mut one =
        Book { id: book.id, name: book.name.clone(), settings: book.settings.clone(), cover: false, pages: vec![p.clone()], unplaced: Vec::new() };
    one.settings.ppi = ppi_for(long_edge, pw.max(ph));
    one.validate()?;
    if p.slots.iter().all(Option::is_none) {
        let ppi = f64::from(one.settings.ppi);
        let px = |inches: f64| ((inches * ppi).round() as usize).max(1);
        return Ok(PageJob(Kind::Blank { width: px(pw), height: px(ph), background: one.settings.background }));
    }
    render::prepare(session, &one).map(|job| PageJob(Kind::Render(Box::new(job))))
}

impl PageJob {
    /// Renders the page. `cancelled` is asked before each photo; `true` stops with
    /// [`render::CANCELLED`].
    pub fn run(self, cancelled: &dyn Fn() -> bool) -> Result<PagePixels, String> {
        match self.0 {
            Kind::Blank { width, height, background } => {
                let n = width.checked_mul(height).ok_or("page too large")?;
                let px = [background[0], background[1], background[2], 255];
                Ok(PagePixels { width, height, rgba: std::iter::repeat_n(px, n).collect::<Vec<_>>().into_flattened() })
            }
            Kind::Render(job) => {
                let raster = job.run_rasters(&mut |_, _| !cancelled())?.into_iter().next().ok_or("no page was made")?;
                let img = page_to_srgb8(&raster)?;
                Ok(PagePixels { width: img.width, height: img.height, rgba: img.data.into_flattened() })
            }
        }
    }
}
