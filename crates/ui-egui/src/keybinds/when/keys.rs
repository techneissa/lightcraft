//! The context keys a condition can use, and the [`Context`] they are read from each frame.
//!
//! A key is either an enumeration (`view`, `tool`) or on / off (`textFocus`). [`KEYS`] is the one
//! table the parser, the autocomplete, the chips and the builder menus read, so a key added here
//! shows up everywhere. A [`Context`] is plain data (a value number per key), built from the app by
//! [`Context::of_app`] and by hand in tests.

use crate::LightcraftApp;
use crate::state::{RightPanel, ViewMode};

/// One value of an enumeration key.
#[derive(Debug)]
pub struct ValueDef {
    /// How it is typed: `detail`.
    pub name: &'static str,
    /// How a chip names it: `Loupe View`.
    pub chip: &'static str,
    pub desc: &'static str,
}

/// What a key holds.
#[derive(Debug)]
pub enum Kind {
    /// One of a list of values (compared with `==` / `!=`).
    Enum(&'static [ValueDef]),
    /// On or off (used alone: `textFocus`, `!textFocus`). The chip names the on state; "Not" in front names the off one.
    Bool { chip: &'static str },
}

/// A context key.
#[derive(Debug)]
pub struct KeyDef {
    pub name: &'static str,
    /// The name in menus (Title Case).
    pub title: &'static str,
    pub desc: &'static str,
    pub kind: Kind,
}

impl KeyDef {
    /// How many values the key can hold (two for an on / off key).
    pub fn size(&self) -> usize {
        match &self.kind {
            Kind::Enum(values) => values.len(),
            Kind::Bool { .. } => 2,
        }
    }

    pub fn is_bool(&self) -> bool {
        matches!(self.kind, Kind::Bool { .. })
    }

    /// The values of an enumeration key (none for an on / off key).
    pub fn values(&self) -> &'static [ValueDef] {
        match &self.kind {
            Kind::Enum(values) => values,
            Kind::Bool { .. } => &[],
        }
    }
}

macro_rules! values {
    ($(($name:expr, $chip:expr, $desc:expr)),* $(,)?) => {
        &[$(ValueDef { name: $name, chip: $chip, desc: $desc }),*]
    };
}

/// The index of `view` in [`KEYS`].
pub const VIEW: usize = 0;
pub const GRID_VIEW: usize = 1;
pub const TOOL: usize = 2;
pub const BRUSH: usize = 3;
pub const TEXT_FOCUS: usize = 4;
pub const DIALOG_OPEN: usize = 5;
pub const PALETTE_OPEN: usize = 6;
pub const HAS_PHOTO: usize = 7;
pub const HAS_SELECTION: usize = 8;
pub const MULTI_SELECTION: usize = 9;
pub const FULLSCREEN: usize = 10;
pub const SLIDESHOW: usize = 11;

/// Every context key, in the order menus and the autocomplete list them.
pub const KEYS: &[KeyDef] = &[
    KeyDef {
        name: "view",
        title: "View",
        desc: "The view in the main area",
        kind: Kind::Enum(values![
            ("grid", "Grid View", "Photo Grid: justified rows of thumbnails"),
            ("square", "Square Grid View", "Square Grid: square thumbnails"),
            ("detail", "Loupe View", "Detail: one photo large (the loupe)"),
            ("compare", "Compare View", "Two photos side by side"),
            ("survey", "Survey View", "The selected photos tiled"),
            ("reference", "Reference View", "A reference photo beside the one being edited"),
            ("people", "People View", "A card for each person named on faces"),
        ]),
    },
    KeyDef {
        name: "gridView",
        title: "Library Grid",
        desc: "The view is Photo Grid or Square Grid",
        kind: Kind::Bool { chip: "Library Grid" },
    },
    KeyDef {
        name: "tool",
        title: "Tool",
        desc: "The tool or panel open on the right",
        kind: Kind::Enum(values![
            ("none", "No Tool", "Nothing open on the right"),
            ("edit", "Edit Tool", "The Edit panel (sliders, profiles)"),
            ("crop", "Crop Tool", "Crop, straighten and Upright"),
            ("remove", "Remove Tool", "Healing and clone spots"),
            ("masking", "Masking Tool", "Masks: brush, gradients, subject and more"),
            ("redEye", "Red Eye Tool", "Red eye and pet eye"),
            ("versions", "Versions Panel", "Saved versions of the photo"),
            ("activity", "Activity Panel", "Background tasks"),
            ("keywords", "Keywords Panel", "Keywords of the selected photos"),
            ("info", "Info Panel", "Metadata of the photo"),
        ]),
    },
    KeyDef {
        name: "brush",
        title: "Brush",
        desc: "The brush is the tool in use on the photo",
        kind: Kind::Bool { chip: "Brush Active" },
    },
    KeyDef {
        name: "textFocus",
        title: "Typing",
        desc: "A text box has the keyboard (shortcuts normally wait for it)",
        kind: Kind::Bool { chip: "Typing" },
    },
    KeyDef {
        name: "dialogOpen",
        title: "Dialog",
        desc: "A dialog is open (shortcuts normally wait for it)",
        kind: Kind::Bool { chip: "Dialog Open" },
    },
    KeyDef {
        name: "paletteOpen",
        title: "Command Palette",
        desc: "The Command Palette is open (shortcuts normally wait for it)",
        kind: Kind::Bool { chip: "Palette Open" },
    },
    KeyDef {
        name: "hasPhoto",
        title: "Active Photo",
        desc: "There is an active photo",
        kind: Kind::Bool { chip: "Active Photo" },
    },
    KeyDef {
        name: "hasSelection",
        title: "Selection",
        desc: "At least one photo is selected",
        kind: Kind::Bool { chip: "Photo Selected" },
    },
    KeyDef {
        name: "multiSelection",
        title: "Several Selected",
        desc: "Two or more photos are selected",
        kind: Kind::Bool { chip: "Several Selected" },
    },
    KeyDef {
        name: "fullscreen",
        title: "Full Screen",
        desc: "The full-screen preview is showing",
        kind: Kind::Bool { chip: "Full Screen" },
    },
    KeyDef {
        name: "slideshow",
        title: "Slideshow",
        desc: "A slideshow is playing",
        kind: Kind::Bool { chip: "Slideshow" },
    },
];

/// The index of the key called `name`.
pub fn key_index(name: &str) -> Option<usize> {
    KEYS.iter().position(|k| k.name == name)
}

/// The index of `value` among the values of the enumeration key `key`.
pub fn value_index(key: usize, value: &str) -> Option<usize> {
    KEYS.get(key)?.values().iter().position(|v| v.name == value)
}

/// Keys that follow from others and are enumerated together (so an impossible mix, such as the
/// Photo Grid with `gridView` off, is never offered as a way two conditions can both hold).
const RELATED: &[(usize, usize)] = &[(GRID_VIEW, VIEW), (VIEW, GRID_VIEW), (MULTI_SELECTION, HAS_SELECTION), (HAS_SELECTION, MULTI_SELECTION)];

/// The keys that have to be enumerated with `keys`.
pub fn closure(keys: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = keys.to_vec();
    for k in keys {
        for (a, b) in RELATED {
            if a == k && !out.contains(b) {
                out.push(*b);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// What the keys hold at one moment: a value number per key (`0`/`1` for an on / off key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    values: [u8; KEYS.len()],
}

impl Default for Context {
    /// The Photo Grid with nothing else going on (so `gridView` is on).
    fn default() -> Self {
        let mut c = Context { values: [0; KEYS.len()] };
        c.set_flag(GRID_VIEW, true);
        c
    }
}

impl Context {
    pub fn get(&self, key: usize) -> usize {
        self.values.get(key).map_or(0, |v| usize::from(*v))
    }

    pub fn set(&mut self, key: usize, value: usize) {
        if let Some(v) = self.values.get_mut(key) {
            *v = u8::try_from(value).unwrap_or(0);
        }
    }

    /// An on / off key.
    pub fn flag(&self, key: usize) -> bool {
        self.get(key) == 1
    }

    pub fn set_flag(&mut self, key: usize, on: bool) {
        self.set(key, usize::from(on));
    }

    /// Whether the values agree with each other: the keys in [`RELATED`] describe one state.
    pub fn consistent(&self) -> bool {
        let grid = matches!(self.get(VIEW), 0 | 1);
        self.flag(GRID_VIEW) == grid && (!self.flag(MULTI_SELECTION) || self.flag(HAS_SELECTION))
    }

    /// A context with the enumeration `key` set to its value called `value` (a test helper that
    /// also keeps `gridView` in step).
    pub fn with(mut self, key: &str, value: &str) -> Self {
        if let Some(k) = key_index(key) {
            match KEYS[k].is_bool() {
                true => self.set_flag(k, value == "true"),
                false => self.set(k, value_index(k, value).unwrap_or(0)),
            }
            self.set_flag(GRID_VIEW, matches!(self.get(VIEW), 0 | 1));
        }
        self
    }

    /// The context now: read from the app and the frame (`ctx` says whether a text box has the
    /// keyboard).
    pub fn of_app(app: &LightcraftApp, ctx: &egui::Context) -> Self {
        let mut c = Context::default();
        c.set(
            VIEW,
            match app.ui.view {
                ViewMode::PhotoGrid => 0,
                ViewMode::SquareGrid => 1,
                ViewMode::Detail => 2,
                ViewMode::Compare => 3,
                ViewMode::Survey => 4,
                ViewMode::Reference => 5,
                ViewMode::People => 6,
            },
        );
        c.set_flag(GRID_VIEW, crate::shortcuts::library_grid(app));
        c.set(
            TOOL,
            match app.ui.right {
                RightPanel::None => 0,
                RightPanel::Edit | RightPanel::Profiles => 1,
                RightPanel::Crop => 2,
                RightPanel::Remove => 3,
                RightPanel::Masking => 4,
                RightPanel::RedEye => 5,
                RightPanel::Versions => 6,
                RightPanel::Activity => 7,
                RightPanel::Keywords => 8,
                RightPanel::Info => 9,
            },
        );
        c.set_flag(BRUSH, app.ui.tool == "brush");
        c.set_flag(TEXT_FOCUS, app.text_focus || ctx.egui_wants_keyboard_input());
        c.set_flag(DIALOG_OPEN, app.ui.dialog.is_some());
        c.set_flag(PALETTE_OPEN, app.ui.palette.is_some());
        c.set_flag(HAS_PHOTO, app.session.active().is_some());
        let selected = app.session.selected().len();
        c.set_flag(HAS_SELECTION, selected >= 1);
        c.set_flag(MULTI_SELECTION, selected >= 2);
        c.set_flag(FULLSCREEN, app.ui.fullscreen);
        c.set_flag(SLIDESHOW, app.ui.slideshow.is_some());
        c
    }

    /// The chip of `key` as this context holds it (`Loupe View`, `not Typing`).
    pub fn chip(&self, key: usize) -> String {
        match KEYS.get(key).map(|k| &k.kind) {
            Some(Kind::Enum(values)) => values.get(self.get(key)).map_or("", |v| v.chip).to_string(),
            Some(Kind::Bool { chip }) => if self.flag(key) { chip.to_string() } else { format!("Not {chip}") },
            None => String::new(),
        }
    }

    /// The value of `key` as a condition spells it (`detail`, `true`).
    pub fn value_name(&self, key: usize) -> String {
        match KEYS.get(key).map(|k| &k.kind) {
            Some(Kind::Enum(values)) => values.get(self.get(key)).map_or("", |v| v.name).to_string(),
            Some(Kind::Bool { .. }) => self.flag(key).to_string(),
            None => String::new(),
        }
    }
}
