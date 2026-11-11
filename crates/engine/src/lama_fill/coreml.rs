//! LaMa on Core ML (issue #268, `fork/coreml.md`): the runtime choice (Automatic, Core ML, Candle), the Neural
//! Engine's load in the background, and which engine fills a crop.
//!
//! The Neural Engine's first load on a Mac takes about a minute (it plans each of the package's functions for it), later
//! ones a few seconds, so the model is loaded on a thread of its own, with a row in the activity stack, as soon as the
//! model's folder and runtime are known ([`LamaFill::set_dir`], [`LamaFill::set_runtime`]); on Automatic every fill
//! until it is ready runs on candle, and so do crops under [`MIN_SIDE`]. A Core ML fill that fails hands that fill and
//! every later one to candle (Automatic) or is the fill's error (Core ML). The loaded model is kept: on the Neural
//! Engine it holds little of the process's memory.

use std::sync::{Arc, PoisonError};

use lightcraft_lama::coreml::{ComputeUnits, CoreMlLama, DEFAULT_UNITS, MIN_SIDE, Runtime, side_for};

use super::LamaFill;
use crate::activity::Cancel;

/// Where the Core ML model stands.
#[derive(Clone, Default)]
pub(super) enum State {
    /// Not loaded, nor loading.
    #[default]
    Off,
    Loading,
    Ready(Arc<CoreMlLama>),
    /// It did not load or a fill failed: candle from now on (until the runtime, units or folder change).
    Failed(String),
}

/// The Core ML side of a [`LamaFill`].
pub(super) struct CoreMl {
    pub(super) runtime: Runtime,
    pub(super) units: ComputeUnits,
    pub(super) state: State,
    /// Bumped by every change of runtime, units or folder: a load that finishes after one is not kept.
    pub(super) generation: u64,
    /// The engine of the last fill (`coreml` or `candle`).
    pub(super) backend: Option<&'static str>,
}

impl Default for CoreMl {
    fn default() -> Self {
        Self { runtime: Runtime::Automatic, units: DEFAULT_UNITS, state: State::Off, generation: 0, backend: None }
    }
}

impl LamaFill {
    fn coreml(&self) -> std::sync::MutexGuard<'_, CoreMl> {
        self.inner.coreml.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run LaMa on `runtime` from the next fill on, loading Core ML in the background when it may run.
    pub fn set_runtime(&self, runtime: Runtime) {
        {
            let mut c = self.coreml();
            if c.runtime == runtime {
                return;
            }
            c.runtime = runtime;
            reset(&mut c);
        }
        self.warm();
    }

    /// The engine chosen.
    pub fn runtime(&self) -> Runtime {
        self.coreml().runtime
    }

    /// Run Core ML on `units` (the Neural Engine by default; issue #267 picks them per Mac): the model is loaded again
    /// on them.
    pub fn set_coreml_units(&self, units: ComputeUnits) {
        {
            let mut c = self.coreml();
            if c.units == units {
                return;
            }
            c.units = units;
            reset(&mut c);
        }
        self.warm();
    }

    /// The engine the last fill used (`coreml` or `candle`); `None` before one.
    pub fn backend(&self) -> Option<&'static str> {
        self.coreml().backend
    }

    pub(super) fn set_backend(&self, backend: &'static str) {
        self.coreml().backend = Some(backend);
    }

    /// Whether Core ML is loaded and fills.
    pub fn coreml_ready(&self) -> bool {
        matches!(self.coreml().state, State::Ready(_))
    }

    /// Whether the model's Core ML version is in its folder (`tools/convert-coreml-lama.py`).
    pub fn coreml_installed(&self) -> bool {
        self.dir().is_some_and(|d| lightcraft_lama::coreml::installed(&d))
    }

    /// Start loading Core ML on a thread of its own, with a row in the activity stack, when it may run here, is
    /// installed, and is neither loaded nor loading.
    pub(super) fn warm(&self) {
        let (units, generation) = {
            let mut c = self.coreml();
            if c.runtime == Runtime::Candle || !matches!(c.state, State::Off) || !lightcraft_coreml::available() || !self.coreml_installed() {
                return;
            }
            c.state = State::Loading;
            (c.units, c.generation)
        };
        let Some(dir) = self.dir() else { return };
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("lama-coreml".into()).spawn(move || {
            let _row = me.inner.activity.start("lamaCoreml", "Preparing LaMa for the Neural Engine", Cancel::No);
            let t = std::time::Instant::now();
            let loaded = std::panic::catch_unwind(|| CoreMlLama::load(&dir, units))
                .unwrap_or_else(|_| Err(lightcraft_coreml::Error::Load("the load stopped unexpectedly".into())));
            let mut c = me.coreml();
            if c.generation != generation {
                return;
            }
            c.state = match loaded {
                Ok(m) => {
                    log::info!("LaMa on Core ML ({units:?}) loaded in {:.1} s", t.elapsed().as_secs_f64());
                    State::Ready(Arc::new(m))
                }
                Err(e) => {
                    log::warn!("LaMa on Core ML did not load ({e}); fills use candle");
                    State::Failed(e.to_string())
                }
            };
        });
        if spawned.is_err() {
            let mut c = self.coreml();
            if c.generation == generation {
                c.state = State::Failed("could not start the Core ML load".into());
            }
        }
    }

    /// What fills this run's crops on Core ML: on Automatic the loaded model or none (it is loading, or not there);
    /// on Core ML the model, waited for while it loads, or an error.
    fn coreml_for_run(&self, runtime: Runtime, cancel: &dyn Fn() -> bool) -> Result<Option<Arc<CoreMlLama>>, String> {
        if runtime == Runtime::Candle {
            return Ok(None);
        }
        self.warm();
        loop {
            let state = self.coreml().state.clone();
            match state {
                State::Ready(m) => return Ok(Some(m)),
                _ if runtime == Runtime::Automatic => return Ok(None),
                State::Failed(e) => return Err(e),
                State::Off => {
                    return Err(match self.coreml_installed() {
                        true => "Core ML runs only on a Mac".to_string(),
                        false => "its Core ML model is not installed (tools/convert-coreml-lama.py)".to_string(),
                    });
                }
                State::Loading if cancel() => return Err("cancelled".into()),
                State::Loading => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
    }

    /// Fill `hole` of the gamma-encoded crop `rgb` (`w × h`): on Core ML when it may take the crop, else on candle.
    pub(super) fn fill_crop(&self, rgb: &[f32], hole: &[f32], w: usize, h: usize, cancel: &dyn Fn() -> bool) -> lightcraft_lama::Result<Vec<f32>> {
        let runtime = self.runtime();
        let coreml = self
            .coreml_for_run(runtime, cancel)
            .map_err(|e| if e == "cancelled" { lightcraft_lama::Error::Cancelled } else { lightcraft_lama::Error::CoreMl(e) })?;
        if let Some(model) = coreml {
            let fits = side_for(w, h).is_some();
            if runtime == Runtime::CoreMl && !fits {
                return Err(lightcraft_lama::Error::CoreMl(format!("a {w}×{h} crop is larger than the Core ML model takes")));
            }
            if fits && (runtime == Runtime::CoreMl || w.max(h) >= MIN_SIDE) {
                if cancel() {
                    return Err(lightcraft_lama::Error::Cancelled);
                }
                match model.inpaint(rgb, hole, w, h) {
                    Ok(out) => {
                        self.set_backend("coreml");
                        return Ok(out);
                    }
                    Err(e) if runtime == Runtime::CoreMl => return Err(e),
                    Err(e) => {
                        log::warn!("LaMa on Core ML failed ({e}); using candle from now on");
                        let mut c = self.coreml();
                        if matches!(&c.state, State::Ready(m) if Arc::ptr_eq(m, &model)) {
                            c.state = State::Failed(e.to_string());
                        }
                    }
                }
            }
        }
        let model = self.model().map_err(lightcraft_lama::Error::Model)?;
        self.set_backend("candle");
        model.inpaint(rgb, hole, w, h, cancel)
    }
}

/// Forget the loaded model and any failure: the next [`LamaFill::warm`] loads again.
pub(super) fn reset(c: &mut CoreMl) {
    c.generation = c.generation.wrapping_add(1);
    c.state = State::Off;
}
