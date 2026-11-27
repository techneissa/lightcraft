//! The loupe's pixels for an eyedropper's magnifier, including a render drawn straight from the GPU (which keeps no CPU
//! copy): read back once per render while the magnifier asks for it.

use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::media::QuickSource;

use super::{Renderer, Slot};

/// What the loupe shows of a photo, for the magnifier.
pub struct MainPixels {
    pub pixels: Arc<egui::ColorImage>,
    pub size: [usize; 2],
    /// The colours are the photo's current look (not a stand-in from another look: the embedded camera JPEG, a
    /// thumbnail-level render), so a value read from them is the edit's.
    pub current: bool,
}

impl Renderer {
    /// The pixels the loupe shows of `photo` (`None` when it shows another photo or nothing readable yet).
    pub fn main_pixels(&mut self, photo: PhotoId) -> Option<MainPixels> {
        let t = self.textures.get(&Slot::Main).filter(|t| t.photo == photo)?;
        let (size, current) = (t.size, t.quick.is_none_or(|q| q == QuickSource::Cached));
        if let Some(pixels) = t.pixels.clone() {
            return Some(MainPixels { pixels, size, current });
        }
        t.native?;
        let shown = self.main_native.0;
        if let Some((at, pixels)) = &self.main_native.1
            && *at == shown
        {
            return Some(MainPixels { pixels: pixels.clone(), size, current });
        }
        let image = self.screen.as_ref()?.read(self.native_frames.get(&Slot::Main)?)?;
        let pixels = Arc::new(image);
        self.main_native.1 = Some((shown, pixels.clone()));
        Some(MainPixels { pixels, size, current })
    }
}
