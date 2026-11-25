//! The Flow build-up and the edge of a live stroke whose edge is feathered as one dab's is
//! (process V10, `lightcraft_pipeline::masks::feather_dab`), kept only where the stroke painted: in
//! tiles of [`TILE`] × [`TILE`] buffer pixels, made when a dab first reaches them.

/// Tile side (buffer pixels).
const TILE: usize = 64;

/// Per pixel `[build, edge]`, in tiles made on first touch.
#[derive(Default)]
pub struct FeatherTiles {
    /// Tiles across.
    cols: usize,
    tiles: Vec<Option<Box<[[f32; 2]]>>>,
}

impl FeatherTiles {
    /// No tile yet, for a `w × h` buffer.
    pub fn new(w: usize, h: usize) -> FeatherTiles {
        let (cols, rows) = (w.div_ceil(TILE), h.div_ceil(TILE));
        FeatherTiles { cols, tiles: (0..cols * rows).map(|_| None).collect() }
    }

    fn index(&self, x: usize, y: usize) -> (usize, usize) {
        ((y / TILE) * self.cols + x / TILE, (y % TILE) * TILE + x % TILE)
    }

    /// `[build, edge]` at buffer pixel `(x, y)`; `None` where nothing painted.
    pub fn get(&self, x: usize, y: usize) -> Option<[f32; 2]> {
        if x >= self.cols * TILE {
            return None;
        }
        let (t, i) = self.index(x, y);
        self.tiles.get(t)?.as_ref()?.get(i).copied()
    }

    /// `[build, edge]` at `(x, y)`, its tile made if needed; `None` outside the buffer.
    pub fn get_mut(&mut self, x: usize, y: usize) -> Option<&mut [f32; 2]> {
        if x >= self.cols * TILE {
            return None;
        }
        let (t, i) = self.index(x, y);
        self.tiles.get_mut(t)?.get_or_insert_with(|| vec![[0.0; 2]; TILE * TILE].into_boxed_slice()).get_mut(i)
    }

    /// Bytes held.
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.tiles.iter().flatten().map(|t| std::mem::size_of_val(&**t)).sum()
    }
}
