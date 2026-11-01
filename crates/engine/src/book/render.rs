//! A book as a print job: the book's cells become the cells of a Custom Package layout, so the
//! Print module's composer renders the photos and its PDF writer produces the file. Photos that
//! show whole (not filled) get a cell of their own aspect inside their slot, so one composer
//! setting (fill the cell) serves both modes and each page can differ.
use super::geometry::{cells_for, crop_window, fitted};
use super::{Book, Crop, Template};
use crate::Session;
use crate::export::{ExportFormat, ExportOptions, encode_image, output_size};
use crate::media::RenderJob;
use crate::print::compose::{self, BlitRect, CellStyle, Image16, ImageSettings, PageStyle, PxRect, place, px_rect, rec2020_from_srgb8};
use crate::print::layout::{Cell, Layout, Margins, Orientation, PageSetup, Style};
use crate::print::template::PrintSettings;
use crate::print::{MAX_JOB_PIXELS, MAX_PAGE_PIXELS, PageRaster, PrintJob, output};
use lightcraft_catalog::PhotoId;
use lightcraft_pipeline::{OutputDepth, OutputSpace};

/// Photos kept rendered while a page is composed (a photo used twice in a row renders once).
const RENDER_CACHE: usize = 4;

/// What a stopped export says.
pub const CANCELLED: &str = "Book export cancelled";

/// The book as Print settings and the photos its cells show. `aspect(photo)` is the photo's
/// width over height as it exports; a photo it does not know fills its slot.
pub fn print_settings(book: &Book, aspect: &dyn Fn(u64) -> Option<f64>) -> Result<(PrintSettings, Vec<PhotoId>), String> {
    book.settings.validate()?;
    let (w, h) = book.settings.page_size()?;
    let mut layout = Layout::default();
    layout.style = Style::CustomPackage;
    layout.page =
        PageSetup { paper: "custom".into(), custom_width: w, custom_height: h, orientation: Orientation::Portrait, margins: Margins::uniform(0.0) };
    layout.pages = u32::try_from(book.pages.len().max(1)).map_err(|_| "too many pages")?;
    let mut ids = Vec::new();
    let mut id = 0u32;
    for (page, p) in book.pages.iter().enumerate() {
        let slots = cells_for(p.template, &book.settings)?;
        for (slot, photo) in slots.iter().zip(&p.slots) {
            let Some(photo) = *photo else { continue };
            let whole = !(book.settings.fill || p.template == Template::FullBleed);
            let rect = if whole { aspect(photo).map_or(*slot, |a| fitted(*slot, a)) } else { *slot };
            id = id.saturating_add(1);
            layout.cells.push(Cell { id, page: u32::try_from(page).map_err(|_| "too many pages")?, rect, photo: Some(photo) });
            ids.push(PhotoId(photo));
        }
    }
    if ids.is_empty() {
        return Err("the book has no photos on its pages".into());
    }
    let settings = PrintSettings {
        layout,
        image: ImageSettings { zoom_to_fill: true, rotate_to_fit: false, ..Default::default() },
        page: PageStyle { background: book.settings.background, ..Default::default() },
        guides: Default::default(),
        job: PrintJob { ppi: book.settings.ppi, ..Default::default() },
    };
    Ok((settings, ids))
}

/// The crop of each photo cell, in the order [`print_settings`] lays the cells out.
fn cell_crops(book: &Book) -> Vec<Option<Crop>> {
    book.pages.iter().flat_map(|p| p.slots.iter().enumerate().filter(|(_, s)| s.is_some()).map(|(c, _)| p.crop(c))).collect()
}

/// One photo to render and where it goes.
struct Item {
    page: usize,
    name: String,
    cell: PxRect,
    /// The part of the photo that shows (fractions of it), for a cropped cell.
    window: Option<(f64, f64, f64, f64)>,
    key: (u64, usize, usize),
    job: RenderJob,
}

/// A book set up from the session: one render per photo cell, ready to run on another thread.
pub struct BookJob {
    pages: Vec<(usize, usize)>,
    items: Vec<Item>,
    ppi: f32,
    background: [u16; 3],
    style: CellStyle,
    job: PrintJob,
    title: String,
}

/// Sets `book` up for export. Fails before anything renders when a photo is gone from the
/// catalog or the pages would be too large for memory (lower the book's resolution).
pub fn prepare(session: &mut Session, book: &Book) -> Result<BookJob, String> {
    let aspect = |photo: u64| {
        let p = session.catalog.photo(PhotoId(photo))?;
        let (w, h) = output_size(p, &ExportOptions::default());
        (w > 0 && h > 0).then(|| w as f64 / h as f64)
    };
    let (settings, _) = print_settings(book, &aspect)?;
    let ppi = f64::from(settings.job.effective_ppi());
    let (pw_in, ph_in) = settings.layout.page_size()?;
    let (pw, ph) = ((pw_in * ppi).round() as usize, (ph_in * ppi).round() as usize);
    let page_px = (pw as u64).saturating_mul(ph as u64);
    if pw == 0 || ph == 0 || page_px > MAX_PAGE_PIXELS {
        return Err(format!("a page of {pw} x {ph} pixels is too large; lower the book's resolution"));
    }
    if page_px.saturating_mul(book.pages.len() as u64) > MAX_JOB_PIXELS {
        return Err("the book is too large for memory; lower its resolution or export fewer pages".into());
    }
    let style = CellStyle::of(&settings, ppi);
    let mut items = Vec::new();
    for (cell, crop) in settings.layout.cells.iter().zip(cell_crops(book)) {
        let Some(id) = cell.photo else { continue };
        let Some(px) = px_rect(&cell.rect, ppi, pw, ph) else { continue };
        let photo = session.catalog.photo(PhotoId(id)).ok_or_else(|| format!("no photo {id}"))?;
        let name = photo.file_name.clone();
        let (nw, nh) = output_size(photo, &ExportOptions::default());
        let (rw, rh, window) = match crop {
            Some(crop) => {
                let window = crop_window(nw as f64 / nh.max(1) as f64, px.w as f64 / px.h as f64, &crop);
                let size = |cell: usize, part: f64| ((cell as f64 / part.max(1e-6)).round() as usize).max(cell);
                let (rw, rh) = (size(px.w, window.2), size(px.h, window.3));
                if (rw as u64).saturating_mul(rh as u64) > MAX_PAGE_PIXELS {
                    return Err(format!("a cropped photo would need {rw} x {rh} pixels; lower the book's resolution"));
                }
                (rw, rh, Some(window))
            }
            None => {
                let p = place(nw as f64, nh as f64, px.w, px.h, true, false).ok_or("a photo has no size")?;
                (p.w, p.h, None)
            }
        };
        let job = session.export_job(PhotoId(id), rw, rh, OutputSpace::Rec2020, OutputDepth::U16)?;
        items.push(Item { page: usize::try_from(cell.page).unwrap_or(0), name, cell: px, window, key: (id, rw, rh), job });
    }
    Ok(BookJob {
        pages: vec![(pw, ph); book.pages.len().max(1)],
        items,
        ppi: ppi as f32,
        background: rec2020_from_srgb8(settings.page.background),
        style,
        job: settings.job,
        title: book.name.clone(),
    })
}

/// The pages of a rendered book.
pub struct Rendered {
    pub pages: Vec<PageRaster>,
    job: PrintJob,
    title: String,
}

/// Draws a cropped photo into `cell`: the photo as large as the crop's window needs, then that window.
fn draw_cropped(page: &mut Image16, photo: &Image16, cell: PxRect, window: (f64, f64, f64, f64)) -> Result<(), String> {
    let (rw, rh) =
        (((cell.w as f64 / window.2.max(1e-6)).round() as usize).max(cell.w), ((cell.h as f64 / window.3.max(1e-6)).round() as usize).max(cell.h));
    let scaled = if (photo.w, photo.h) == (rw, rh) { None } else { Some(photo.resized(rw, rh)?) };
    let img = scaled.as_ref().unwrap_or(photo);
    let sx = ((window.0 * img.w as f64).round() as usize).min(img.w.saturating_sub(cell.w));
    let sy = ((window.1 * img.h as f64).round() as usize).min(img.h.saturating_sub(cell.h));
    let (w, h) = (cell.w.min(img.w), cell.h.min(img.h));
    page.blit(img, BlitRect { x: sx, y: sy, w, h }, BlitRect { x: cell.x, y: cell.y, w, h });
    Ok(())
}

impl BookJob {
    /// Renders (progress steps) the job will run.
    pub fn steps(&self) -> usize {
        self.items.len()
    }

    /// Renders every photo and composes the pages. `progress(done, name)` runs before each render;
    /// returning `false` cancels.
    pub fn run(self, progress: &mut dyn FnMut(usize, &str) -> bool) -> Result<Rendered, String> {
        let mut cache: Vec<((u64, usize, usize), Image16)> = Vec::new();
        let mut done = 0usize;
        let mut items = self.items.into_iter().peekable();
        let mut out = Vec::with_capacity(self.pages.len());
        for (index, &(w, h)) in self.pages.iter().enumerate() {
            let mut page = Image16::filled(w, h, self.background)?;
            while let Some(item) = items.next_if(|i| i.page == index) {
                if !progress(done, &item.name) {
                    return Err(CANCELLED.to_string());
                }
                done += 1;
                let at = match cache.iter().position(|(k, _)| *k == item.key) {
                    Some(at) => at,
                    None => {
                        let img = compose::render(item.job, &self.job.color).map_err(|e| format!("{}: {e}", item.name))?;
                        if cache.len() >= RENDER_CACHE {
                            cache.remove(0);
                        }
                        cache.push((item.key, img));
                        cache.len() - 1
                    }
                };
                let photo = &cache.get(at).ok_or("render cache lost an entry")?.1;
                match item.window {
                    Some(window) => draw_cropped(&mut page, photo, item.cell, window),
                    None => compose::draw_cell(&mut page, photo, item.cell, &self.style).map(|_| ()),
                }
                .map_err(|e| format!("{}: {e}", item.name))?;
            }
            out.push(PageRaster { width: w as u32, height: h as u32, ppi: self.ppi, rgb: page.rgb });
        }
        if !progress(done, "") {
            return Err(CANCELLED.to_string());
        }
        Ok(Rendered { pages: out, job: self.job, title: self.title })
    }
}

impl Rendered {
    /// The book as one PDF: a page per book page, its size the page size.
    pub fn pdf(&self) -> Result<Vec<u8>, String> {
        output::write(&self.pages, &self.job, &self.title)
    }

    /// One JPEG (sRGB) per page.
    pub fn jpegs(&self, quality: u8) -> Result<Vec<Vec<u8>>, String> {
        let options = ExportOptions { format: ExportFormat::Jpeg, quality, ..Default::default() };
        self.pages.iter().map(|p| encode_image(&output::page_to_srgb8(p)?, &options)).collect()
    }
}

/// A file name stem from a book name: letters, digits, spaces, `-` and `_` only.
pub fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "Book".into() } else { s }
}

/// The JPEG files of a book of `pages` pages in `dir`: `<name>-01.jpg`, `<name>-02.jpg`…
pub fn jpeg_targets(dir: &str, name: &str, pages: usize) -> Vec<String> {
    let name = file_stem(name);
    let digits = pages.to_string().len().max(2);
    (1..=pages).map(|n| std::path::Path::new(dir).join(format!("{name}-{n:0digits$}.jpg")).to_string_lossy().to_string()).collect()
}
