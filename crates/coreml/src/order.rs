//! The Core ML units a model tries in turn (issue #267): the first that loads runs it. The app measures which units
//! are fastest for each model on this Mac and passes them in that order; until then each model passes its one unit of
//! old. [`Sample`] is the one call a measurement makes.

use std::path::PathBuf;

use crate::{ComputeUnits, Error, Model, Result};

impl ComputeUnits {
    /// The units a measurement tries (the CPU alone is never faster than the GPU).
    pub const MEASURED: [ComputeUnits; 3] = [ComputeUnits::CpuAndGpu, ComputeUnits::All, ComputeUnits::CpuAndNeuralEngine];

    /// The id the choices file uses.
    pub fn id(self) -> &'static str {
        match self {
            ComputeUnits::All => "all",
            ComputeUnits::CpuAndNeuralEngine => "neural-engine",
            ComputeUnits::CpuAndGpu => "gpu",
            ComputeUnits::CpuOnly => "cpu",
        }
    }
}

/// Up to four Core ML units to try in turn, each once (a `Copy` list, so a set of choices stays `Copy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitOrder {
    units: [ComputeUnits; 4],
    len: u8,
}

impl UnitOrder {
    /// Only `units`.
    pub const fn one(units: ComputeUnits) -> Self {
        Self { units: [units; 4], len: 1 }
    }

    /// `units` in this order, repeats dropped, at most four; an empty list is [`ComputeUnits::CpuAndGpu`] alone.
    pub fn new(units: &[ComputeUnits]) -> Self {
        let mut me = Self { units: [ComputeUnits::CpuAndGpu; 4], len: 0 };
        for &u in units {
            if me.len < 4 && !me.iter().any(|v| v == u) {
                if let Some(slot) = me.units.get_mut(usize::from(me.len)) {
                    *slot = u;
                }
                me.len += 1;
            }
        }
        if me.len == 0 { Self::one(ComputeUnits::CpuAndGpu) } else { me }
    }

    pub fn iter(&self) -> impl Iterator<Item = ComputeUnits> + '_ {
        self.units.iter().copied().take(usize::from(self.len))
    }

    /// The units tried first.
    pub fn first(&self) -> ComputeUnits {
        self.units[0]
    }
}

/// Load with each of `order`'s units in turn until one works: the model and its units. A model that is missing or
/// unavailable is that error at once (no other units would find it); otherwise the last unit's error.
pub fn first_that_loads<T>(order: UnitOrder, mut load: impl FnMut(ComputeUnits) -> Result<T>) -> Result<(T, ComputeUnits)> {
    let mut last = Error::Unavailable;
    for units in order.iter() {
        match load(units) {
            Ok(m) => return Ok((m, units)),
            Err(e @ (Error::Missing(_) | Error::Unavailable)) => return Err(e),
            Err(e) => {
                log::warn!("coreml: the model did not load on {} ({e}); trying the next units", units.id());
                last = e;
            }
        }
    }
    Err(last)
}

/// One call to a converted model, as a measurement makes it: its package, the function, and the input and output it
/// takes and gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub package: PathBuf,
    pub compiled: PathBuf,
    pub function: Option<String>,
    pub input: &'static str,
    pub shape: Vec<usize>,
    pub output: &'static str,
}

impl Sample {
    /// Load the model on `units` (compiled first when needed).
    pub fn load(&self, units: ComputeUnits) -> Result<Model> {
        crate::load_package_function(&self.package, &self.compiled, units, self.function.as_deref())
    }

    /// A smooth made-up input of the sample's shape, values between 0.1 and 0.9 (the same at every call).
    pub fn input_data(&self) -> Result<Vec<f32>> {
        let n = crate::layout::check_shape(&self.shape, self.shape.iter().try_fold(1usize, |a, &d| a.checked_mul(d)).unwrap_or(0))?;
        let width = self.shape.last().copied().unwrap_or(1).max(1);
        Ok((0..n)
            .map(|i| {
                let (x, y) = ((i % width) as f32, (i / width) as f32);
                0.5 + 0.4 * (x * 0.071 + y * 0.013).sin() * (y * 0.029 + x * 0.005).cos()
            })
            .collect())
    }

    /// Run `model` on `data`.
    pub fn run(&self, model: &Model, data: &[f32]) -> Result<Vec<f32>> {
        Ok(model.predict(self.input, &self.shape, data, self.output)?.data)
    }
}
