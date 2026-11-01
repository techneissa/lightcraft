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
    /// Lays `photos` out again (see [`auto_layout`]), replacing every page except the locked ones:
    /// those stay where they are, with their photos, and the other pages are made from the other
    /// photos (a photo on a locked page is not laid out again). A locked first page stays the cover.
    pub fn relayout(&mut self, photos: &[u64], cover: bool, preset: Preset) -> Result<(), String> {
        let locked: Vec<(usize, Page)> = self.pages.iter().enumerate().filter(|(_, p)| p.locked).map(|(i, p)| (i, p.clone())).collect();
        if locked.is_empty() {
            self.pages = auto_layout(photos, cover, preset)?;
            self.cover = cover;
            self.unplaced.clear();
            return Ok(());
        }
        let mut free = photos.to_vec();
        for photo in locked.iter().flat_map(|(_, p)| p.slots.iter().flatten()) {
            if let Some(at) = free.iter().position(|f| f == photo) {
                free.remove(at);
            }
        }
        // a locked first page stays first (and stays the cover when it was); nothing else can be
        let keeps_first = locked.first().is_some_and(|(i, _)| *i == 0);
        let want_cover = cover && !keeps_first;
        let mut pages = if free.is_empty() { Vec::new() } else { auto_layout(&free, want_cover, preset)? };
        let cover_now = if keeps_first { self.cover } else { want_cover && !pages.is_empty() };
        let mut at_end = Vec::new();
        for (i, page) in locked {
            if i <= pages.len() {
                pages.insert(i, page);
            } else {
                at_end.push(page);
            }
        }
        pages.extend(at_end);
        if pages.len() > MAX_PAGES {
            return Err(format!("the layout needs more than {MAX_PAGES} pages; use fewer photos or a template with more photos per page"));
        }
        self.cover = cover_now;
        self.pages = pages;
        self.unplaced.clear();
        Ok(())
    }

    /// Adds or removes the cover and nothing else: every other page keeps its template and photos.
    /// A new cover is a full-bleed page that takes the first photo in the tray (empty without one);
    /// a removed cover's photo goes to the end of the tray, so none is lost.
    pub fn set_cover(&mut self, on: bool) -> Result<(), String> {
        if on == self.cover {
            return Ok(());
        }
        if on {
            if self.pages.len() >= MAX_PAGES {
                return Err(format!("a book has at most {MAX_PAGES} pages; there is no room for a cover"));
            }
            let photo = (!self.unplaced.is_empty()).then(|| self.unplaced.remove(0));
            self.pages.insert(0, Page { template: Template::FullBleed, crops: vec![None], slots: vec![photo], locked: false });
        } else if !self.pages.is_empty() {
            let cover = self.pages.remove(0);
            self.unplaced.extend(cover.slots.into_iter().flatten());
        }
        self.cover = on;
        Ok(())
    }

    /// Gives page `index` (zero-based) another template. Photos keep their order: the first cells
    /// keep theirs (and their crops), photos that no longer fit go to [`Book::unplaced`], and new
    /// cells take photos from there first.
    pub fn set_template(&mut self, index: usize, template: Template) -> Result<(), String> {
        let count = self.pages.len();
        let page = self.pages.get_mut(index).ok_or_else(|| format!("no page {} (the book has {count})", index.saturating_add(1)))?;
        let mut slots = std::mem::take(&mut page.slots);
        let mut crops = std::mem::take(&mut page.crops);
        let want = template.cells();
        for dropped in slots.drain(want.min(slots.len())..) {
            self.unplaced.extend(dropped);
        }
        crops.resize(want, None);
        while slots.len() < want {
            slots.push((!self.unplaced.is_empty()).then(|| self.unplaced.remove(0)));
        }
        page.template = template;
        page.slots = slots;
        page.crops = crops;
        Ok(())
    }

    fn page_mut(&mut self, page: usize) -> Result<&mut Page, String> {
        let count = self.pages.len();
        self.pages.get_mut(page).ok_or_else(|| format!("no page {} (the book has {count})", page.saturating_add(1)))
    }

    /// The photo and crop in a cell.
    fn cell_content(&self, slot: Slot) -> Result<(Option<u64>, Option<super::Crop>), String> {
        match slot {
            Slot::Cell { page, cell } => {
                let p = self.pages.get(page).ok_or_else(|| format!("no page {} (the book has {})", page.saturating_add(1), self.pages.len()))?;
                let photo = p.slots.get(cell).copied().ok_or_else(|| format!("page {} has no cell {}", page + 1, cell.saturating_add(1)))?;
                Ok((photo, p.crop(cell)))
            }
            Slot::Unplaced { .. } => Err("an unplaced photo has no cell".into()),
        }
    }

    fn put_cell(&mut self, slot: Slot, photo: Option<u64>, crop: Option<super::Crop>) -> Result<(), String> {
        let Slot::Cell { page, cell } = slot else { return Err("an unplaced photo has no cell".into()) };
        self.page_mut(page)?.put(cell, photo, crop).map(|_| ()).ok_or_else(|| format!("page {} has no cell {}", page + 1, cell.saturating_add(1)))
    }

    /// Moves the photo at `from` to `to`. A cell that holds a photo swaps with it (an unplaced
    /// photo's place is taken by the displaced one); a photo takes its crop along to another cell,
    /// and loses it in the tray. Moving a page photo to the tray unplaces it, at the
    /// tray position given (0 to its length), and a tray photo moved within the tray takes that position.
    pub fn move_photo(&mut self, from: Slot, to: Slot) -> Result<(), String> {
        if from == to {
            self.check_slot(from)?;
            return Ok(());
        }
        match (from, to) {
            (Slot::Unplaced { unplaced: i }, Slot::Cell { .. }) => {
                let photo = *self.unplaced.get(i).ok_or_else(|| format!("no unplaced photo {}", i.saturating_add(1)))?;
                let (displaced, _) = self.cell_content(to)?;
                self.put_cell(to, Some(photo), None)?;
                self.unplaced.remove(i);
                self.unplaced.extend(displaced);
            }
            (Slot::Cell { .. }, Slot::Cell { .. }) => {
                let (photo, crop) = self.cell_content(from)?;
                photo.ok_or("that cell holds no photo")?;
                let (other, other_crop) = self.cell_content(to)?;
                self.put_cell(to, photo, crop)?;
                self.put_cell(from, other, other_crop)?;
            }
            (Slot::Cell { .. }, Slot::Unplaced { unplaced: at }) => {
                if at > self.unplaced.len() {
                    return Err(format!("the tray holds {} photos; position {at} is past its end", self.unplaced.len()));
                }
                let (photo, _) = self.cell_content(from)?;
                let photo = photo.ok_or("that cell holds no photo")?;
                self.put_cell(from, None, None)?;
                self.unplaced.insert(at, photo);
            }
            (Slot::Unplaced { unplaced: i }, Slot::Unplaced { unplaced: at }) => {
                self.check_slot(from)?;
                if at >= self.unplaced.len() {
                    return Err(format!("the tray holds {} photos; position {at} is past its end", self.unplaced.len()));
                }
                let photo = self.unplaced.remove(i);
                self.unplaced.insert(at, photo);
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
