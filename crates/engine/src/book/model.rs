//! The data of a book and the checks every book passes before it is stored or used.
use super::{MAX_CELLS_PER_PAGE, MAX_NAME, MAX_PAGES, MAX_PHOTOS, geometry};
use crate::print::layout::Orientation;
use serde::{Deserialize, Serialize};

/// Page sizes by name: portrait width and height, inches.
const SIZES: &[(&str, f64, f64)] = &[
    ("5x7", 5.0, 7.0),
    ("6x9", 6.0, 9.0),
    ("7x7", 7.0, 7.0),
    ("8x8", 8.0, 8.0),
    ("8x10", 8.0, 10.0),
    ("10x10", 10.0, 10.0),
    ("11x14", 11.0, 14.0),
    ("12x12", 12.0, 12.0),
    ("a5", 148.0 / 25.4, 210.0 / 25.4),
    ("a4", 210.0 / 25.4, 297.0 / 25.4),
    ("letter", 8.5, 11.0),
];

/// Largest margin and cell padding accepted, inches.
pub const MAX_MARGIN_IN: f64 = 2.0;
pub const MAX_PADDING_IN: f64 = 1.0;
/// Lowest and highest resolution a book is made at, pixels per inch.
pub use crate::print::{MAX_PPI, MIN_PPI};

/// The names [`BookSettings::size`] takes.
pub fn size_names() -> impl Iterator<Item = &'static str> {
    SIZES.iter().map(|s| s.0)
}

/// Portrait size in inches of a named page size.
pub fn size_of(name: &str) -> Option<(f64, f64)> {
    let name = name.trim().to_lowercase();
    SIZES.iter().find(|s| s.0 == name).map(|s| (s.1, s.2))
}

/// How a page is divided into photo cells. All original; the cells follow the page's long side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Template {
    /// One photo inside the margins.
    #[default]
    One,
    /// Two photos side by side (landscape pages) or stacked (portrait and square pages).
    Two,
    /// One large photo on the first half, two small on the other.
    Three,
    /// Four photos in a 2 x 2 grid.
    Grid4,
    /// One photo over the whole page, ignoring margin and padding.
    FullBleed,
}

impl Template {
    pub const ALL: [Template; 5] = [Template::One, Template::Two, Template::Three, Template::Grid4, Template::FullBleed];

    /// Photo cells on a page of this template.
    pub fn cells(self) -> usize {
        match self {
            Template::One | Template::FullBleed => 1,
            Template::Two => 2,
            Template::Three => 3,
            Template::Grid4 => MAX_CELLS_PER_PAGE,
        }
    }

    /// The plain template holding `n` photos (1 to 4).
    pub fn for_count(n: usize) -> Template {
        match n {
            0 | 1 => Template::One,
            2 => Template::Two,
            3 => Template::Three,
            _ => Template::Grid4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Template::One => "one",
            Template::Two => "two",
            Template::Three => "three",
            Template::Grid4 => "grid4",
            Template::FullBleed => "fullBleed",
        }
    }

    /// Parses a name from [`Template::name`], ignoring case.
    pub fn parse(s: &str) -> Result<Template, String> {
        Template::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("unknown template `{s}` (one of {})", Template::ALL.map(Template::name).join(", ")))
    }
}

/// Page size and the look shared by every page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BookSettings {
    /// A name from [`size_names`].
    pub size: String,
    /// Square sizes ignore it.
    pub orientation: Orientation,
    /// Space between the page edge and the cells, inches (full-bleed pages ignore it).
    pub margin: f64,
    /// Space inside each cell, on every side, inches (full-bleed pages ignore it).
    pub padding: f64,
    /// Page background, sRGB.
    pub background: [u8; 3],
    /// Fill each cell with its photo and crop what overflows; off: the whole photo shows.
    /// Full-bleed pages always fill.
    pub fill: bool,
    /// Resolution of the exported pages, pixels per inch.
    pub ppi: f32,
}

impl Default for BookSettings {
    fn default() -> Self {
        Self { size: "8x10".into(), orientation: Orientation::Landscape, margin: 0.5, padding: 0.1, background: [255; 3], fill: false, ppi: 150.0 }
    }
}

fn check(v: f64, what: &str, max: f64) -> Result<(), String> {
    if v.is_finite() && (0.0..=max).contains(&v) { Ok(()) } else { Err(format!("{what} must be a number from 0 to {max} inches")) }
}

impl BookSettings {
    /// Page width and height in inches after orientation.
    pub fn page_size(&self) -> Result<(f64, f64), String> {
        let (w, h) = size_of(&self.size).ok_or_else(|| format!("unknown page size `{}` (one of {})", self.size, size_names().collect::<Vec<_>>().join(", ")))?;
        Ok(match self.orientation {
            Orientation::Landscape if w != h => (h, w),
            _ => (w, h),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        self.page_size()?;
        check(self.margin, "margin", MAX_MARGIN_IN)?;
        check(self.padding, "padding", MAX_PADDING_IN)?;
        if !(self.ppi.is_finite() && (MIN_PPI..=MAX_PPI).contains(&self.ppi)) {
            return Err(format!("ppi must be a number from {MIN_PPI} to {MAX_PPI}"));
        }
        geometry::check_cells(self)
    }
}

/// One page: a template and a photo (or none) for each of its cells, in reading order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub template: Template,
    pub slots: Vec<Option<u64>>,
}

impl Page {
    /// A page of `template` with `photos` in its first cells (the rest stay empty).
    pub fn with(template: Template, photos: &[u64]) -> Page {
        let mut slots = vec![None; template.cells()];
        for (slot, p) in slots.iter_mut().zip(photos) {
            *slot = Some(*p);
        }
        Page { template, slots }
    }
}

/// Where a photo sits in a book: a cell of a page, or the tray of photos on no page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", untagged)]
pub enum Slot {
    Cell { page: usize, cell: usize },
    Unplaced { unplaced: usize },
}

/// A photo book.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Book {
    pub id: u32,
    pub name: String,
    pub settings: BookSettings,
    /// The first page is the cover.
    pub cover: bool,
    pub pages: Vec<Page>,
    /// Photos of the book that are on no page.
    pub unplaced: Vec<u64>,
}

/// A book name: trimmed, 1 to [`MAX_NAME`] characters, no control characters.
pub fn clean_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(format!("a book name is 1 to {MAX_NAME} characters without control characters"));
    }
    Ok(name.to_string())
}

impl Book {
    /// Every photo in the book, pages first (a photo used twice is listed twice).
    pub fn photos(&self) -> Vec<u64> {
        self.pages.iter().flat_map(|p| p.slots.iter().flatten().copied()).chain(self.unplaced.iter().copied()).collect()
    }

    /// Checks the settings and the shape of every page; run on anything that did not come from the
    /// setters (a stored file, command parameters).
    pub fn validate(&mut self) -> Result<(), String> {
        self.name = clean_name(&self.name)?;
        self.settings.validate()?;
        if self.pages.len() > MAX_PAGES {
            return Err(format!("a book has at most {MAX_PAGES} pages"));
        }
        if self.cover && self.pages.is_empty() {
            return Err("a book with a cover has a first page".into());
        }
        for (i, p) in self.pages.iter().enumerate() {
            if p.slots.len() != p.template.cells() {
                return Err(format!("page {} has {} slots but its template has {} cells", i + 1, p.slots.len(), p.template.cells()));
            }
        }
        if self.photos().len() > MAX_PHOTOS {
            return Err(format!("a book holds at most {MAX_PHOTOS} photos"));
        }
        Ok(())
    }
}

impl Book {
    /// Page indices as they lie open: the first page (the cover) alone, then two by two; the last
    /// page stands alone when the count is even.
    pub fn spreads(&self) -> Vec<Vec<usize>> {
        let n = self.pages.len();
        if n == 0 {
            return Vec::new();
        }
        let mut out = vec![vec![0]];
        out.extend((1..n).step_by(2).map(|i| (i..(i + 2).min(n)).collect::<Vec<_>>()));
        out
    }
}
