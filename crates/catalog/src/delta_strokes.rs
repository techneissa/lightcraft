//! Log patches of develop settings without turning every brush stroke into JSON each time (issue #378).
//!
//! A patch is [`delta::diff`] of the settings' JSON before and after an edit. With brush masks that JSON holds every
//! point of every stroke, so each stroke converted, compared and dropped every earlier stroke: 43 ms a stroke at 800
//! strokes. Here the settings are converted with each stroke as a small *stand-in*: a stroke with no points whose
//! `size`, `flow` and `density` are [`MARK`] and whose `feather` is the stroke's id in a table. The table keeps each
//! stroke's JSON next to its points' `Arc`, so a stroke is converted once, the first time it is seen, and
//! [`delta::diff_in`] compares two strokes by id (the same points, pressures and numbers) or else by their JSON,
//! and writes a stroke's JSON where the patch holds it. The patch is the same, byte for byte, as `delta::diff` of
//! the whole JSON.
//!
//! A stand-in is told apart by its numbers. When the settings hold [`MARK`] anywhere else, a stand-in could be
//! mistaken for something else: the settings are then diffed whole, as before.

use std::collections::HashMap;
use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, MaskShape};
use serde_json::Value;

use crate::delta::{self, Patch, StandIns};

/// The number a stand-in's `size`, `flow` and `density` hold: not a whole number, and no setting's real value.
const MARK: f64 = f64::from_bits(0x3E80_9D2A_57C4_E1B3);

/// What makes two strokes written the same: the same points and pressures (by `Arc`) and the same numbers (by bits).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    points: usize,
    pressure: usize,
    numbers: [u64; 4],
    erase: bool,
    auto_mask: bool,
}

impl Key {
    fn of(s: &BrushStroke) -> Key {
        Key {
            points: Arc::as_ptr(&s.points).cast::<()>() as usize,
            pressure: Arc::as_ptr(&s.pressure).cast::<()>() as usize,
            numbers: [s.size.to_bits(), s.feather.to_bits(), s.flow.to_bits(), s.density.to_bits()],
            erase: s.erase,
            auto_mask: s.auto_mask,
        }
    }
}

struct Entry {
    /// Holds the `Arc`s, so their addresses in `key` aren't reused while the entry lives.
    _stroke: BrushStroke,
    json: Value,
    /// The last [`Strokes::pass`] that used it.
    used: u64,
}

/// Each stroke's JSON, by id.
#[derive(Default)]
pub(crate) struct Strokes {
    by_key: HashMap<Key, u64>,
    by_id: HashMap<u64, Entry>,
    next: u64,
    pass: u64,
    /// Strokes converted to JSON so far.
    pub(crate) converted: u64,
}

impl Strokes {
    fn id_of(&mut self, s: &BrushStroke) -> Option<u64> {
        let key = Key::of(s);
        if let Some(&id) = self.by_key.get(&key)
            && let Some(e) = self.by_id.get_mut(&id)
        {
            e.used = self.pass;
            return Some(id);
        }
        let json = serde_json::to_value(s).ok()?;
        self.converted += 1;
        let id = self.next;
        self.next += 1;
        self.by_key.insert(key, id);
        self.by_id.insert(id, Entry { _stroke: s.clone(), json, used: self.pass });
        Some(id)
    }

    /// The settings' JSON with each stroke as a stand-in (`None`: they don't convert, or hold [`MARK`] elsewhere).
    fn skeleton(&mut self, s: &DevelopSettings) -> Option<Value> {
        let mut c = s.clone();
        let mut n = 0usize;
        let brushes = c.masks.iter_mut().flat_map(|m| m.components.iter_mut()).filter_map(|p| match &mut p.shape {
            MaskShape::Brush { strokes } => Some(strokes),
            _ => None,
        });
        let strokes = brushes.flat_map(|s| s.iter_mut()).chain(c.lens_blur.focus_strokes.iter_mut()).chain(c.lens_blur.blur_strokes.iter_mut());
        for st in strokes {
            let id = self.id_of(st)?;
            *st = BrushStroke {
                points: Arc::default(),
                pressure: Arc::default(),
                size: MARK,
                feather: id as f64,
                flow: MARK,
                density: MARK,
                erase: false,
                auto_mask: false,
            };
            n += 1;
        }
        let v = serde_json::to_value(&c).ok()?;
        let (found, marks) = self.count(&v);
        (found == n && marks == n.saturating_mul(3)).then_some(v)
    }

    /// The stand-ins in `v`, and the numbers that are [`MARK`].
    fn count(&self, v: &Value) -> (usize, usize) {
        let here = usize::from(self.stand_in(v).is_some());
        match v {
            Value::Number(x) => (0, usize::from(x.as_f64().is_some_and(|f| f.to_bits() == MARK.to_bits()))),
            Value::Array(a) => a.iter().map(|x| self.count(x)).fold((here, 0), |(f, m), (g, k)| (f + g, m + k)),
            Value::Object(o) => o.values().map(|x| self.count(x)).fold((here, 0), |(f, m), (g, k)| (f + g, m + k)),
            _ => (0, 0),
        }
    }

    /// Drop the strokes this pass didn't use.
    fn end_pass(&mut self) {
        let pass = self.pass;
        self.by_id.retain(|_, e| e.used == pass);
        let by_id = &self.by_id;
        self.by_key.retain(|_, id| by_id.contains_key(id));
    }
}

impl StandIns for Strokes {
    fn stand_in<'a>(&'a self, v: &Value) -> Option<(u64, &'a Value)> {
        let o = v.as_object()?;
        let marked = |k: &str| o.get(k).and_then(Value::as_f64).is_some_and(|f| f.to_bits() == MARK.to_bits());
        if !(marked("size") && marked("flow") && marked("density")) {
            return None;
        }
        let f = o.get("feather")?.as_f64()?;
        if !(f >= 0.0 && f.fract() == 0.0 && f < 9.0e15) {
            return None;
        }
        let id = f as u64;
        self.by_id.get(&id).map(|e| (id, &e.json))
    }
}

/// The settings' JSON of the last patch, with stand-ins, and the table of strokes it names.
#[derive(Default)]
pub(crate) struct Skeletons {
    /// Holding the `Arc` keeps its address from being reused.
    last: Option<(Arc<DevelopSettings>, Value)>,
    pub(crate) strokes: Strokes,
}

impl Skeletons {
    /// `delta::diff` of the JSON of `old` and `new` (`None` if they can't be converted to JSON).
    pub(crate) fn patch(&mut self, old: &Arc<DevelopSettings>, new: &Arc<DevelopSettings>) -> Option<Patch> {
        if Arc::ptr_eq(old, new) {
            return Some(Patch::Same);
        }
        self.strokes.pass += 1;
        let a = match self.last.take() {
            Some((s, v)) if Arc::ptr_eq(&s, old) => Some(v),
            _ => self.strokes.skeleton(old),
        };
        let b = self.strokes.skeleton(new);
        let patch = match (a, b) {
            (Some(a), Some(b)) => {
                let p = delta::diff_in(&a, &b, &self.strokes);
                self.last = Some((new.clone(), b));
                Some(p)
            }
            _ => whole(old, new),
        };
        self.strokes.end_pass();
        patch
    }
}

/// `delta::diff` of the settings' whole JSON.
pub(crate) fn whole(old: &DevelopSettings, new: &DevelopSettings) -> Option<Patch> {
    Some(delta::diff(&serde_json::to_value(old).ok()?, &serde_json::to_value(new).ok()?))
}
