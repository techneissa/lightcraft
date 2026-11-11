//! Fork feature (issue #267, `fork/coreml.md`): each AI model with a Core ML version runs on its fastest unit on this
//! Mac (the GPU, the Neural Engine, Core ML's own mix, or candle on the GPU), chosen by measuring and remembered.
//!
//! - **Measured once** per model file, input shape, macOS version and chip ([`key`]), in the background as a row in
//!   the activity ring, never while the photo on screen is being made ([`measure::measure`]). Each option loads the
//!   model (capped at [`LOAD_CAP`]: a Neural Engine load can take minutes; an option over the cap is remembered as
//!   such), runs it once to warm it, then once timed, on the same made-up input.
//! - **Checked:** the first option is today's unit for the model (the reference, which the model's own tests hold equal
//!   to candle); another counts only when its output matches the reference's within the model's tolerance
//!   ([`probes`]).
//! - **Used:** a load tries the measured options fastest first, then the model's unit of old, then candle on the GPU,
//!   then candle on the CPU ([`order`]); one that does not load hands over to the next. Until a model is measured it
//!   loads as before, so nothing is slower on first use.
//! - **Kept** in `<models>/model-units.json`, beside the model folders. Settings ▸ Performance ▸ AI Models shows each
//!   model's unit and time, and measures again on request (`models.measure`).
//!
//! Not done: running a model on its second-best unit while another model holds its best one. A model is loaded once
//! on one unit and kept, so that needs a second loaded copy per model, not a busy count.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use lightcraft_coreml::{ComputeUnits, UnitOrder};
use serde::{Deserialize, Serialize};

pub mod measure;
#[cfg(not(target_arch = "wasm32"))]
pub mod probes;

/// The choices file's name in the models folder.
pub const FILE: &str = "model-units.json";
/// Largest choices file read.
const MAX_FILE: u64 = 256 << 10;
/// Longest a measured load may take before its option is dropped (BiRefNet took 257 s on the Neural Engine).
pub const LOAD_CAP: Duration = Duration::from_secs(90);

/// Where a model can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Unit {
    CoreMlGpu,
    CoreMlAll,
    CoreMlNeuralEngine,
    CandleGpu,
    CandleCpu,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::CoreMlGpu, Unit::CoreMlAll, Unit::CoreMlNeuralEngine, Unit::CandleGpu, Unit::CandleCpu];

    pub fn coreml(units: ComputeUnits) -> Self {
        match units {
            ComputeUnits::All => Unit::CoreMlAll,
            ComputeUnits::CpuAndNeuralEngine => Unit::CoreMlNeuralEngine,
            ComputeUnits::CpuAndGpu | ComputeUnits::CpuOnly => Unit::CoreMlGpu,
        }
    }

    /// Whether Core ML compiles the model for the Neural Engine on this option (its slow first load).
    pub fn neural_engine(self) -> bool {
        matches!(self, Unit::CoreMlAll | Unit::CoreMlNeuralEngine)
    }

    /// Core ML's units for this option; `None` for candle.
    pub fn compute_units(self) -> Option<ComputeUnits> {
        match self {
            Unit::CoreMlGpu => Some(ComputeUnits::CpuAndGpu),
            Unit::CoreMlAll => Some(ComputeUnits::All),
            Unit::CoreMlNeuralEngine => Some(ComputeUnits::CpuAndNeuralEngine),
            Unit::CandleGpu | Unit::CandleCpu => None,
        }
    }

    /// The id the choices file and `models.runtime` use.
    pub fn id(self) -> &'static str {
        match self {
            Unit::CoreMlGpu => "coreml-gpu",
            Unit::CoreMlAll => "coreml-all",
            Unit::CoreMlNeuralEngine => "coreml-neural-engine",
            Unit::CandleGpu => "candle-gpu",
            Unit::CandleCpu => "candle-cpu",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|u| u.id() == id)
    }

    /// The name Settings shows.
    pub fn label(self) -> &'static str {
        match self {
            Unit::CoreMlGpu => "GPU (Core ML)",
            Unit::CoreMlAll => "All Units (Core ML)",
            Unit::CoreMlNeuralEngine => "Neural Engine (Core ML)",
            Unit::CandleGpu => "GPU (Candle)",
            Unit::CandleCpu => "CPU (Candle)",
        }
    }
}

/// How one option did when measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Loaded, ran, and matched the reference.
    Ok,
    /// Ran, but its output was not the reference's.
    Mismatch,
    /// Its load took longer than [`LOAD_CAP`].
    LoadCap,
    /// It did not load or run.
    Failed,
}

/// One option's measurement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    /// [`Unit::id`].
    pub unit: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// One run's time (the mean of up to ten when a run is short).
    pub run_ms: Option<f64>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Timing {
    pub fn unit(&self) -> Option<Unit> {
        Unit::from_id(&self.unit)
    }
}

/// A model's measurement and what it was measured for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// [`key`]: a measurement of another key is not used.
    pub key: String,
    pub options: Vec<Timing>,
}

/// The fastest option that matched the reference, and its time.
pub fn chosen(options: &[Timing]) -> Option<(Unit, f64)> {
    options.iter().filter(|t| t.outcome == Outcome::Ok).filter_map(|t| Some((t.unit()?, t.run_ms?))).min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The options a load tries, in turn: when `record` was measured for `key`, its options that matched the reference,
/// fastest first; then `default` (the model's unit of old, how it loaded before any measurement, so it is never
/// dropped); then candle on the GPU, then candle on the CPU, always last. An option that failed, missed the reference
/// or hit the load cap is not tried.
pub fn order(record: Option<&Record>, key: &str, default: Unit) -> Vec<Unit> {
    let mut ok: Vec<(Unit, f64)> = record
        .filter(|r| r.key == key)
        .map(|r| r.options.iter().filter(|t| t.outcome == Outcome::Ok).filter_map(|t| Some((t.unit()?, t.run_ms?))).collect())
        .unwrap_or_default();
    ok.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut out: Vec<Unit> = ok.into_iter().map(|(u, _)| u).filter(|&u| u != Unit::CandleCpu).collect();
    for u in [default, Unit::CandleGpu, Unit::CandleCpu] {
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// What a load of a model with a Core ML version does, from [`order`]: Core ML on `coreml`'s units in turn, unless
/// candle comes first (`candle_first`: candle was measured fastest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub coreml: UnitOrder,
    pub candle_first: bool,
}

impl Plan {
    pub fn from_order(order: &[Unit], default: ComputeUnits) -> Self {
        let units: Vec<ComputeUnits> = order.iter().filter_map(|u| u.compute_units()).collect();
        let coreml = if units.is_empty() { UnitOrder::one(default) } else { UnitOrder::new(&units) };
        Self { coreml, candle_first: order.first().is_some_and(|u| u.compute_units().is_none()) }
    }

    /// The model's unit of old, as loads were before any measurement.
    pub fn default_for(units: ComputeUnits) -> Self {
        Self { coreml: UnitOrder::one(units), candle_first: false }
    }

    /// The runtime a load uses for the one chosen in Settings: Automatic becomes Candle when candle comes first.
    pub fn runtime(&self, chosen: lightcraft_coreml::Runtime) -> lightcraft_coreml::Runtime {
        match chosen {
            lightcraft_coreml::Runtime::Automatic if self.candle_first => lightcraft_coreml::Runtime::Candle,
            other => other,
        }
    }
}

/// What a measurement is filed under: the model files' `fingerprint`, the input `shape`, and this Mac ([`machine`]).
pub fn key(fingerprint: &str, shape: &str) -> String {
    format!("{fingerprint} {shape} {}", machine())
}

/// This Mac's macOS version and chip, read once (`other` off the Mac).
pub fn machine() -> &'static str {
    static MACHINE: OnceLock<String> = OnceLock::new();
    MACHINE.get_or_init(read_machine)
}

#[cfg(target_os = "macos")]
fn read_machine() -> String {
    // (both read without `unsafe`: the system's version file, and `sysctl`)
    let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").unwrap_or_default();
    let os = plist
        .split("<key>ProductBuildVersion</key>")
        .nth(1)
        .and_then(|rest| rest.split("<string>").nth(1))
        .and_then(|rest| rest.split("</string>").next())
        .unwrap_or("unknown")
        .trim()
        .to_string();
    let chip = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().replace(' ', "-"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    format!("macos-{os} {chip}")
}

#[cfg(not(target_os = "macos"))]
fn read_machine() -> String {
    "other".into()
}

fn read(path: &Path) -> BTreeMap<String, Record> {
    let Ok(meta) = std::fs::metadata(path) else { return BTreeMap::new() };
    if meta.len() > MAX_FILE {
        log::warn!("{}: too large, ignored", path.display());
        return BTreeMap::new();
    }
    match std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(map) => map,
        None => {
            log::warn!("{}: not a model units file, ignored", path.display());
            BTreeMap::new()
        }
    }
}

fn write(path: &Path, map: &BTreeMap<String, Record>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.part");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The measurements, the file they are kept in (`None`: in memory, as in tests), and the models being measured.
#[derive(Default)]
pub struct ModelUnits {
    file: Mutex<Option<PathBuf>>,
    records: Mutex<BTreeMap<String, Record>>,
    /// Models waiting for or under measurement.
    pending: Mutex<BTreeSet<String>>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Mutex<Option<probes::Worker>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app's measurements: one set per Mac, shared by every session of the process.
pub fn shared() -> &'static ModelUnits {
    static SHARED: OnceLock<ModelUnits> = OnceLock::new();
    SHARED.get_or_init(ModelUnits::default)
}

impl ModelUnits {
    /// Keep the measurements in `file`, reading what it holds.
    pub fn open(&self, file: Option<PathBuf>) {
        let records = file.as_deref().map(read).unwrap_or_default();
        *lock(&self.records) = records;
        *lock(&self.file) = file;
    }

    /// `model`'s measurement, whatever its key.
    pub fn record(&self, model: &str) -> Option<Record> {
        lock(&self.records).get(model).cloned()
    }

    /// Whether `model` was measured for `key`.
    pub fn measured(&self, model: &str, key: &str) -> bool {
        lock(&self.records).get(model).is_some_and(|r| r.key == key)
    }

    /// The options a load of `model` tries ([`order`]).
    pub fn order(&self, model: &str, key: &str, default: Unit) -> Vec<Unit> {
        order(lock(&self.records).get(model), key, default)
    }

    /// Whether `model` waits for or is under measurement.
    pub fn measuring(&self, model: &str) -> bool {
        lock(&self.pending).contains(model)
    }

    /// Mark `model` as waiting for measurement: `false` when it already was.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn begin(&self, model: &str) -> bool {
        lock(&self.pending).insert(model.to_string())
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn end(&self, model: &str) {
        lock(&self.pending).remove(model);
    }

    /// File `model`'s measurement for `key` (saved when there is a file; kept in memory when the save fails).
    pub fn store(&self, model: &str, key: &str, options: Vec<Timing>) {
        let mut records = lock(&self.records);
        records.insert(model.to_string(), Record { key: key.to_string(), options });
        if let Some(path) = lock(&self.file).as_deref()
            && let Err(e) = write(path, &records)
        {
            log::warn!("model units: {e}");
        }
    }

    /// Drop `model`'s measurement, so it is measured again.
    pub fn forget(&self, model: &str) {
        let mut records = lock(&self.records);
        if records.remove(model).is_some()
            && let Some(path) = lock(&self.file).as_deref()
            && let Err(e) = write(path, &records)
        {
            log::warn!("model units: {e}");
        }
    }
}

#[cfg(test)]
mod tests;
