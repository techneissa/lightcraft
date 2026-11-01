//! Changing a book: auto layout, a page's template, moving a photo.
use super::model::{Page, Slot, Template};
use super::{Book, MAX_PAGES, MAX_PHOTOS};

/// How auto layout chooses a page's template.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preset {
    /// Cycles one, two, three and four photos per page.
    #[default]
    Mixed,
    One,
    Two,
    Three,
    Grid4,
    FullBleed,
}

impl Preset {
    pub fn parse(s: &str) -> Result<Preset, String> {
        Ok(match s.trim().to_lowercase().as_str() {
            "mixed" => Preset::Mixed,
            "one" => Preset::One,
            "two" => Preset::Two,
            "three" => Preset::Three,
            "grid4" => Preset::Grid4,
            "fullbleed" => Preset::FullBleed,
            _ => return Err(format!("unknown preset `{s}` (one of mixed, one, two, three, grid4, fullBleed)")),
        })
    }

    fn template(self, page: usize) -> Template {
        match self {
            Preset::Mixed => [Template::One, Template::Two, Template::Three, Template::Grid4][page % 4],
            Preset::One => Template::One,
            Preset::Two => Template::Two,
            Preset::Three => Template::Three,
            Preset::Grid4 => Template::Grid4,
            Preset::FullBleed => Template::FullBleed,
        }
    }
}

/// Pages for `photos` in order: with a cover, the first photo alone on a full-bleed first page;
/// then pages of the preset's templates. The last page takes the template that fits the photos
/// left, so no page has an empty cell.
pub fn auto_layout(photos: &[u64], cover: bool, preset: Preset) -> Result<Vec<Page>, String> {
    if photos.is_empty() {
        return Err("a book needs at least one photo".into());
    }
    if photos.len() > MAX_PHOTOS {
        return Err(format!("a book holds at most {MAX_PHOTOS} photos"));
    }
    let mut pages = Vec::new();
    let mut rest = photos;
    if cover && let Some((first, tail)) = rest.split_first() {
        pages.push(Page::with(Template::FullBleed, &[*first]));
        rest = tail;
    }
    let mut inner = 0;
    while !rest.is_empty() {
        let wanted = preset.template(inner);
        let n = wanted.cells().min(rest.len());
        let template = if n == wanted.cells() { wanted } else { Template::for_count(n) };
        let (now, later) = rest.split_at(n);
        pages.push(Page::with(template, now));
        rest = later;
        inner += 1;
        if pages.len() > MAX_PAGES {
            return Err(format!(
                "{} photos make more than {MAX_PAGES} pages; use fewer photos or a template with more photos per page",
                photos.len()
            ));
        }
    }
    Ok(pages)
}

impl Book {
    /// Lays `photos` out again (see [`auto_layout`]), replacing every page.
    pub fn relayout(&mut self, photos: &[u64], cover: bool, preset: Preset) -> Result<(), String> {
        self.pages = auto_layout(photos, cover, preset)?;
        self.cover = cover;
        self.unplaced.clear();
        Ok(())
    }

    /// Gives page `index` (zero-based) another template. Photos keep their order: the first cells
    /// keep theirs, photos that no longer fit go to [`Book::unplaced`], and new cells take photos
    /// from there first.
    pub fn set_template(&mut self, index: usize, template: Template) -> Result<(), String> {
        let count = self.pages.len();
        let page = self.pages.get_mut(index).ok_or_else(|| format!("no page {} (the book has {count})", index.saturating_add(1)))?;
        let mut slots = std::mem::take(&mut page.slots);
        let want = template.cells();
        for dropped in slots.drain(want.min(slots.len())..) {
            self.unplaced.extend(dropped);
        }
        while slots.len() < want {
            slots.push((!self.unplaced.is_empty()).then(|| self.unplaced.remove(0)));
        }
        page.template = template;
        page.slots = slots;
        Ok(())
    }

    fn slot_mut(&mut self, slot: Slot) -> Result<&mut Option<u64>, String> {
        match slot {
            Slot::Cell { page, cell } => {
                let count = self.pages.len();
                let p = self.pages.get_mut(page).ok_or_else(|| format!("no page {} (the book has {count})", page.saturating_add(1)))?;
                p.slots.get_mut(cell).ok_or_else(|| format!("page {} has no cell {}", page + 1, cell.saturating_add(1)))
            }
            Slot::Unplaced { .. } => Err("an unplaced photo has no cell".into()),
        }
    }

    /// Moves the photo at `from` to `to`. A cell that holds a photo swaps with it (an unplaced
    /// photo's place is taken by the displaced one); moving a page photo to the tray unplaces it.
    pub fn move_photo(&mut self, from: Slot, to: Slot) -> Result<(), String> {
        if from == to {
            self.check_slot(from)?;
            return Ok(());
        }
        match (from, to) {
            (Slot::Unplaced { unplaced: i }, Slot::Cell { .. }) => {
                let photo = *self.unplaced.get(i).ok_or_else(|| format!("no unplaced photo {}", i.saturating_add(1)))?;
                let target = self.slot_mut(to)?;
                let displaced = target.replace(photo);
                self.unplaced.remove(i);
                self.unplaced.extend(displaced);
            }
            (Slot::Cell { .. }, Slot::Cell { .. }) => {
                let photo = (*self.slot_mut(from)?).ok_or("that cell holds no photo")?;
                let displaced = self.slot_mut(to)?.replace(photo);
                *self.slot_mut(from)? = displaced;
            }
            (Slot::Cell { .. }, Slot::Unplaced { .. }) => {
                let photo = self.slot_mut(from)?.take().ok_or("that cell holds no photo")?;
                self.unplaced.push(photo);
            }
            (Slot::Unplaced { .. }, Slot::Unplaced { .. }) => {
                self.check_slot(from)?;
            }
        }
        Ok(())
    }

    fn check_slot(&self, slot: Slot) -> Result<(), String> {
        match slot {
            Slot::Unplaced { unplaced } => {
                self.unplaced.get(unplaced).map(|_| ()).ok_or_else(|| format!("no unplaced photo {}", unplaced.saturating_add(1)))
            }
            Slot::Cell { page, cell } => {
                let p = self.pages.get(page).ok_or_else(|| format!("no page {} (the book has {})", page.saturating_add(1), self.pages.len()))?;
                p.slots.get(cell).map(|_| ()).ok_or_else(|| format!("page {} has no cell {}", page + 1, cell.saturating_add(1)))
            }
        }
    }
}
