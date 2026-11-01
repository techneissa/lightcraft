//! A book as a print job: the book's cells become the cells of a Custom Package layout, so the
//! Print module's composer renders the photos and its PDF writer produces the file. Photos that
//! show whole (not filled) get a cell of their own aspect inside their slot, so one composer
//! setting (fill the cell) serves both modes and each page can differ.
use super::geometry::{cells_for, fitted};
use super::{Book, Template};
use crate::Session;
use crate::export::{ExportFormat, ExportOptions, encode_image, output_size};
use crate::print::compose::{self, ImageSettings, PageStyle, Prepared};
use crate::print::layout::{Cell, Layout, Margins, Orientation, PageSetup, Style};
use crate::print::template::PrintSettings;
use crate::print::{PageRaster, PrintJob, output};
use lightcraft_catalog::PhotoId;

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

/// A book set up from the session: one render per photo cell, ready to run on another thread.
pub struct BookJob {
    prepared: Prepared,
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
    let (settings, ids) = print_settings(book, &aspect)?;
    let prepared = compose::prepare(session, &settings, &ids)?;
    Ok(BookJob { prepared, job: settings.job, title: book.name.clone() })
}

/// The pages of a rendered book.
pub struct Rendered {
    pub pages: Vec<PageRaster>,
    job: PrintJob,
    title: String,
}

impl BookJob {
    /// Renders (progress steps) the job will run.
    pub fn steps(&self) -> usize {
        self.prepared.len()
    }

    /// Renders every photo and composes the pages. `progress(done, name)` runs before each render;
    /// returning `false` cancels.
    pub fn run(self, progress: &mut dyn FnMut(usize, &str) -> bool) -> Result<Rendered, String> {
        let pages = self.prepared.run(progress).map_err(|e| if e == "Print cancelled" { CANCELLED.to_string() } else { e })?;
        Ok(Rendered { pages, job: self.job, title: self.title })
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
