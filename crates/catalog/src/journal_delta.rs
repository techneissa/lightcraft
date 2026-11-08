//! Log records that hold only what changed (issue #77, fork catalog format 2).
//!
//! An edit is `SetDevelop` + `PushHistory` with the photo's whole develop settings, and an undo
//! or redo is `SetHistory` with every History step. With brush masks the settings hold every
//! point of every stroke, so the log grew with the square of the strokes (300 strokes: 240 MB of
//! log; each undo about 100 MB). Since fork format 2 these ops are written as patches against the
//! photo's state **before the record** ([`DeltaOp`]); every other op, and an op on a photo the
//! log doesn't know yet, is written as before ([`LogOp::Plain`]).
//!
//! The writer keeps a [`Shadow`]: the photos as replaying the log would leave them (the snapshot
//! or the loaded catalog, then every appended record, applied with the catalog's own rules). A
//! patch is made against the shadow, so replay finds exactly the state it was made from. The
//! shadow moves on only once a batch is appended; a failed append is retried against the same
//! state. Every patch checks the shape it was made from ([`crate::delta`]). On replay a patch that
//! doesn't fit is skipped with the rest of its photo's part of the record, and that photo's later
//! patches too, so a mismatch is reported (`LoadReport::failed`) and never mixes two states; the
//! other photos in the record still apply. `Journal::open` then keeps the log as
//! `catalog.log.damaged-<seq>` and writes a snapshot, so the next session patches that state.
//!
//! Patches are relative to the writer's JSON of the settings: any change to how develop settings
//! serialize is a catalog format change (see `journal` → *Format versions* → Rules).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::delta::{self, Patch};
use crate::{Catalog, HistoryStep, Op, Photo, PhotoId};

/// One op as a log record holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum LogOp {
    Delta(DeltaOp),
    /// Written exactly as before fork format 2.
    Plain(Op),
}

/// Ops written as patches against the photo before the record. Every tag starts with `patch`,
/// which no [`Op`] tag does ([`decode`] reads the tag to choose).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub(crate) enum DeltaOp {
    /// `SetDevelop`: the photo's settings before the record, patched.
    #[serde(rename = "patchDevelop")]
    Develop { id: PhotoId, patch: Patch, label: String, edited: Option<String> },
    /// `PushHistory` of a step whose settings are `base`.
    #[serde(rename = "patchPush")]
    Push { id: PhotoId, label: String, base: StepBase },
    /// `SetHistory`: each step kept from the photo's History before the record, or new.
    #[serde(rename = "patchHistory")]
    History { id: PhotoId, steps: Vec<StepRef> },
    /// `Batch`, its ops in order.
    #[serde(rename = "patchBatch")]
    Batch { ops: Vec<LogOp> },
}

/// Where a pushed History step's settings come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum StepBase {
    /// The settings this record last set for the photo (with `setDevelop` or `patchDevelop`).
    Develop,
    /// The photo's settings before the record, patched.
    Patch(Patch),
}

/// One step of a `patchHistory`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum StepRef {
    /// Steps `from .. from + len` of the photo's History before the record.
    Keep { from: usize, len: usize },
    /// A new step: the settings of the step before it in the list (the photo's settings before
    /// the record, for the first) patched.
    New { label: String, patch: Patch },
}

/// Parse a record's op. `Err` is a body this version can't read (written by a newer build, as
/// the record's CRC matched).
pub(crate) fn decode(body: &str) -> serde_json::Result<LogOp> {
    if body.starts_with("{\"op\":\"patch") { serde_json::from_str(body).map(LogOp::Delta) } else { serde_json::from_str(body).map(LogOp::Plain) }
}

/// The JSON of settings, and the settings it was made from, so the next patch of the same photo
/// doesn't convert them again. Holding the `Arc` keeps its address from being reused.
type Cached = Option<(Arc<DevelopSettings>, Value)>;

fn value_of(s: &Arc<DevelopSettings>, cache: &mut Cached) -> Option<Value> {
    match cache.take() {
        Some((a, v)) if Arc::ptr_eq(&a, s) => Some(v),
        _ => serde_json::to_value(&**s).ok(),
    }
}

/// The photos as the log leaves them; see the module docs.
#[derive(Default)]
pub(crate) struct Shadow {
    photos: BTreeMap<PhotoId, Arc<Photo>>,
    /// Keyed by the settings' identity, so it stays right whatever the shadow does.
    cache: Cached,
}

impl Shadow {
    pub(crate) fn of(catalog: &Catalog) -> Shadow {
        Shadow { photos: catalog.photos.clone(), cache: None }
    }

    /// Start encoding a batch of records; [`Shadow::commit`] keeps what it did once they are written.
    pub(crate) fn pass(&mut self) -> Pass<'_> {
        Pass { photos: &self.photos, changed: BTreeMap::new(), cache: &mut self.cache }
    }

    /// Move the shadow past the records of a pass ([`Pass::changes`]) once they are written.
    pub(crate) fn commit(&mut self, changes: Changes) {
        for (id, p) in changes.0 {
            match p {
                Some(p) => self.photos.insert(id, p),
                None => self.photos.remove(&id),
            };
        }
    }
}

/// The photos a pass changed (`None`: removed).
pub(crate) struct Changes(BTreeMap<PhotoId, Option<Arc<Photo>>>);

/// Encoding a batch of records against the shadow, without changing it.
pub(crate) struct Pass<'a> {
    photos: &'a BTreeMap<PhotoId, Arc<Photo>>,
    changed: BTreeMap<PhotoId, Option<Arc<Photo>>>,
    cache: &'a mut Cached,
}

impl Pass<'_> {
    fn photo(&self, id: PhotoId) -> Option<&Arc<Photo>> {
        match self.changed.get(&id) {
            Some(p) => p.as_ref(),
            None => self.photos.get(&id),
        }
    }

    pub(crate) fn changes(self) -> Changes {
        Changes(self.changed)
    }

    /// The record for `op`, then the shadow moves past it.
    pub(crate) fn encode(&mut self, op: &Op) -> LogOp {
        let rec = self.encode_in(op, &mut HashMap::new());
        self.step(op);
        rec
    }

    /// The patch from `old` to `new` (`None` if they can't be converted to JSON).
    fn patch(&mut self, old: &Arc<DevelopSettings>, new: &Arc<DevelopSettings>) -> Option<Patch> {
        if Arc::ptr_eq(old, new) {
            return Some(Patch::Same);
        }
        let a = value_of(old, self.cache)?;
        let b = serde_json::to_value(&**new).ok()?;
        let p = delta::diff(&a, &b);
        *self.cache = Some((new.clone(), b));
        Some(p)
    }

    /// `set`: the settings this record set so far, by photo.
    fn encode_in(&mut self, op: &Op, set: &mut HashMap<PhotoId, Arc<DevelopSettings>>) -> LogOp {
        let plain = || LogOp::Plain(op.clone());
        match op {
            Op::SetDevelop { id, settings, label, edited } => {
                let before = self.photo(*id).map(|p| p.develop.clone());
                set.insert(*id, settings.clone());
                match before.and_then(|b| self.patch(&b, settings)) {
                    Some(patch) => LogOp::Delta(DeltaOp::Develop { id: *id, patch, label: label.clone(), edited: edited.clone() }),
                    None => plain(),
                }
            }
            Op::PushHistory { id, step } => {
                if set.get(id).is_some_and(|s| Arc::ptr_eq(s, &step.settings)) {
                    return LogOp::Delta(DeltaOp::Push { id: *id, label: step.label.clone(), base: StepBase::Develop });
                }
                let before = self.photo(*id).map(|p| p.develop.clone());
                match before.and_then(|b| self.patch(&b, &step.settings)) {
                    Some(p) => LogOp::Delta(DeltaOp::Push { id: *id, label: step.label.clone(), base: StepBase::Patch(p) }),
                    None => plain(),
                }
            }
            Op::SetHistory { id, history } => match self.history_steps(*id, history) {
                Some(steps) => LogOp::Delta(DeltaOp::History { id: *id, steps }),
                None => plain(),
            },
            Op::Batch { ops } => {
                let recs: Vec<LogOp> = ops.iter().map(|o| self.encode_in(o, set)).collect();
                if recs.iter().all(|r| matches!(r, LogOp::Plain(_))) { plain() } else { LogOp::Delta(DeltaOp::Batch { ops: recs }) }
            }
            _ => plain(),
        }
    }

    fn history_steps(&mut self, id: PhotoId, history: &[HistoryStep]) -> Option<Vec<StepRef>> {
        let p = self.photo(id)?.clone();
        let mut at: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, s) in p.history.iter().enumerate() {
            at.entry(Arc::as_ptr(&s.settings) as usize).or_default().push(i);
        }
        let same = |i: usize, s: &HistoryStep| p.history.get(i).is_some_and(|c| Arc::ptr_eq(&c.settings, &s.settings) && c.label == s.label);
        let mut out: Vec<StepRef> = Vec::new();
        let mut prev = p.develop.clone();
        for s in history {
            if let Some(StepRef::Keep { from, len }) = out.last_mut()
                && same(from.saturating_add(*len), s)
            {
                *len += 1;
            } else if let Some(i) = at.get(&(Arc::as_ptr(&s.settings) as usize)).and_then(|v| v.iter().copied().find(|i| same(*i, s))) {
                out.push(StepRef::Keep { from: i, len: 1 });
            } else {
                out.push(StepRef::New { label: s.label.clone(), patch: self.patch(&prev, &s.settings)? });
            }
            prev = s.settings.clone();
        }
        Some(out)
    }

    /// Move the shadow past `op`, as [`Catalog::apply`] moves a photo's develop settings and History.
    fn step(&mut self, op: &Op) {
        match op {
            Op::AddPhoto { photo } => {
                if self.photo(photo.id).is_none() {
                    let mut p = (**photo).clone();
                    p.trim_history();
                    self.changed.insert(p.id, Some(Arc::new(p)));
                }
            }
            Op::RemovePhoto { id } => {
                self.changed.insert(*id, None);
            }
            Op::SetDevelop { id, settings, .. } => self.edit(*id, |p| p.develop = settings.clone()),
            Op::PushHistory { id, step } => self.edit(*id, |p| p.push_history(step.clone())),
            Op::SetHistory { id, history } => self.edit(*id, |p| {
                p.history = history.clone();
                p.trim_history();
            }),
            Op::SetHistoryCursor { id, cursor } => self.edit(*id, |p| p.history_cursor = *cursor),
            Op::Batch { ops } => ops.iter().for_each(|o| self.step(o)),
            _ => {}
        }
    }

    fn edit(&mut self, id: PhotoId, f: impl FnOnce(&mut Photo)) {
        if !self.changed.contains_key(&id) {
            let Some(p) = self.photos.get(&id).cloned() else { return };
            self.changed.insert(id, Some(p));
        }
        // copied from the shadow once per pass, then changed in place
        if let Some(Some(p)) = self.changed.get_mut(&id) {
            f(Arc::make_mut(p));
        }
    }
}

/// Replaying the log: turns records back into ops against the catalog as replay has built it.
#[derive(Default)]
pub(crate) struct Replay {
    cache: Cached,
    /// Photos one of whose patches failed: their later patches are skipped, never applied to a
    /// state they weren't made from.
    broken: HashSet<PhotoId>,
}

impl Replay {
    /// Apply record `rec` to `catalog` (the state before it). A patch that doesn't fit (or is of a
    /// photo whose earlier patch didn't) is skipped with the rest of that photo's part of the
    /// record, and the photo's later patches are skipped too; the other photos' parts apply. An
    /// error says what was skipped.
    pub(crate) fn apply(&mut self, rec: LogOp, catalog: &mut Catalog) -> Result<(), String> {
        let mut skipped: Vec<(PhotoId, String)> = Vec::new();
        let op = self.resolve_in(rec, catalog, &mut HashMap::new(), &mut skipped);
        let bad: HashSet<PhotoId> = skipped.iter().map(|(id, _)| *id).collect();
        let op = op.and_then(|op| without(op, &bad));
        self.broken.extend(bad.iter().copied());
        if let Some(op) = op {
            let mut ids = Vec::new();
            op_ids(&op, &mut ids);
            if let Err(e) = catalog.apply(op) {
                // nothing of it applied: every photo it changes missed a change
                self.broken.extend(ids);
                skipped.push((PhotoId(0), e.to_string()));
            }
        }
        match skipped.first() {
            None => Ok(()),
            Some((_, e)) if skipped.len() == 1 => Err(e.clone()),
            Some((_, e)) => Err(format!("{e} (and {} more)", skipped.len() - 1)),
        }
    }

    fn develop(catalog: &Catalog, id: PhotoId) -> Result<Arc<DevelopSettings>, String> {
        catalog.photo(id).map(|p| p.develop.clone()).ok_or_else(|| format!("no photo {}", id.0))
    }

    fn patched(&mut self, base: &Arc<DevelopSettings>, patch: &Patch) -> Result<Arc<DevelopSettings>, String> {
        if *patch == Patch::Same {
            return Ok(base.clone());
        }
        let mut v = value_of(base, &mut self.cache).ok_or("develop settings don't convert to JSON")?;
        delta::apply(&mut v, patch)?;
        let s = Arc::new(DevelopSettings::deserialize(&v).map_err(|e| e.to_string())?);
        self.cache = Some((s.clone(), v));
        Ok(s)
    }

    /// The op `rec` stands for; a patch that can't be resolved is left out and listed in `skipped`.
    fn resolve_in(
        &mut self,
        rec: LogOp,
        catalog: &Catalog,
        set: &mut HashMap<PhotoId, Arc<DevelopSettings>>,
        skipped: &mut Vec<(PhotoId, String)>,
    ) -> Option<Op> {
        let d = match rec {
            LogOp::Plain(op) => {
                note_set(&op, set);
                return Some(op);
            }
            LogOp::Delta(DeltaOp::Batch { ops }) => {
                let out: Vec<Op> = ops.into_iter().filter_map(|o| self.resolve_in(o, catalog, set, skipped)).collect();
                return (!out.is_empty()).then_some(Op::Batch { ops: out });
            }
            LogOp::Delta(d) => d,
        };
        let mut ids = Vec::new();
        delta_ids(&d, &mut ids);
        let id = ids.first().copied().unwrap_or(PhotoId(0));
        if self.broken.contains(&id) {
            skipped.push((id, format!("photo {}: an earlier change to it failed", id.0)));
            return None;
        }
        match self.leaf(d, catalog, set) {
            Ok(op) => Some(op),
            Err(e) => {
                skipped.push((id, e));
                None
            }
        }
    }

    /// One patch op (not a batch) as the op it stands for.
    fn leaf(&mut self, d: DeltaOp, catalog: &Catalog, set: &mut HashMap<PhotoId, Arc<DevelopSettings>>) -> Result<Op, String> {
        Ok(match d {
            DeltaOp::Develop { id, patch, label, edited } => {
                let settings = self.patched(&Self::develop(catalog, id)?, &patch)?;
                set.insert(id, settings.clone());
                Op::SetDevelop { id, settings, label, edited }
            }
            DeltaOp::Push { id, label, base } => {
                let settings = match base {
                    StepBase::Develop => {
                        set.get(&id).cloned().ok_or_else(|| format!("photo {}: a History step names settings the record doesn't set", id.0))?
                    }
                    StepBase::Patch(p) => self.patched(&Self::develop(catalog, id)?, &p)?,
                };
                Op::PushHistory { id, step: HistoryStep { label, settings } }
            }
            DeltaOp::History { id, steps } => {
                let p = catalog.photo(id).ok_or_else(|| format!("no photo {}", id.0))?;
                let mut history: Vec<HistoryStep> = Vec::new();
                let mut prev = p.develop.clone();
                for s in steps {
                    match s {
                        StepRef::Keep { from, len } => {
                            let kept = from
                                .checked_add(len)
                                .and_then(|end| p.history.get(from..end))
                                .ok_or_else(|| format!("photo {}: History steps {from}+{len} of {} kept", id.0, p.history.len()))?;
                            history.extend_from_slice(kept);
                        }
                        StepRef::New { label, patch } => {
                            let settings = self.patched(&prev, &patch)?;
                            history.push(HistoryStep { label, settings });
                        }
                    }
                    if let Some(last) = history.last() {
                        prev = last.settings.clone();
                    }
                    if history.len() > crate::HISTORY_LIMIT.saturating_mul(4) {
                        return Err(format!("photo {}: History of {} steps", id.0, history.len()));
                    }
                }
                Op::SetHistory { id, history }
            }
            DeltaOp::Batch { .. } => return Err("a batch where one op was expected".into()),
        })
    }
}

/// `op` without the ops that change a photo in `bad` (`None` when nothing is left).
fn without(op: Op, bad: &HashSet<PhotoId>) -> Option<Op> {
    if bad.is_empty() {
        return Some(op);
    }
    match op {
        Op::Batch { ops } => {
            let ops: Vec<Op> = ops.into_iter().filter_map(|o| without(o, bad)).collect();
            (!ops.is_empty()).then_some(Op::Batch { ops })
        }
        op => {
            let mut ids = Vec::new();
            op_ids(&op, &mut ids);
            (!ids.iter().any(|id| bad.contains(id))).then_some(op)
        }
    }
}

/// Record the settings each `SetDevelop` in `op` sets.
fn note_set(op: &Op, set: &mut HashMap<PhotoId, Arc<DevelopSettings>>) {
    match op {
        Op::SetDevelop { id, settings, .. } => {
            set.insert(*id, settings.clone());
        }
        Op::Batch { ops } => ops.iter().for_each(|o| note_set(o, set)),
        _ => {}
    }
}

fn delta_ids(d: &DeltaOp, out: &mut Vec<PhotoId>) {
    match d {
        DeltaOp::Develop { id, .. } | DeltaOp::Push { id, .. } | DeltaOp::History { id, .. } => out.push(*id),
        DeltaOp::Batch { ops } => {
            for o in ops {
                match o {
                    LogOp::Delta(d) => delta_ids(d, out),
                    LogOp::Plain(op) => op_ids(op, out),
                }
            }
        }
    }
}

/// The photos whose develop settings or History `op` changes.
fn op_ids(op: &Op, out: &mut Vec<PhotoId>) {
    match op {
        Op::SetDevelop { id, .. }
        | Op::PushHistory { id, .. }
        | Op::SetHistory { id, .. }
        | Op::SetHistoryCursor { id, .. }
        | Op::RemovePhoto { id } => out.push(*id),
        Op::AddPhoto { photo } => out.push(photo.id),
        Op::Batch { ops } => ops.iter().for_each(|o| op_ids(o, out)),
        _ => {}
    }
}
