//! Measuring a model on each of its options (issue #267): load (capped), one run to warm it, one timed run, the
//! output checked against the first option's. The model and the clock come in from outside, so the tests use fakes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use super::{Outcome, Timing, Unit};

/// Longest a measured run may take before its option counts as failed (a guard against a hang, not a budget).
const RUN_LIMIT: Duration = Duration::from_secs(600);
/// Timed runs of a fast model repeat until they took this long together, or [`MAX_RUNS`] of them.
const MIN_TIMED: Duration = Duration::from_millis(250);
const MAX_RUNS: u32 = 10;

/// Where the time comes from.
pub trait Clock: Send + Sync {
    /// Time since some fixed start.
    fn now(&self) -> Duration;
}

/// The real clock.
pub struct Wall(std::time::Instant);

impl Default for Wall {
    fn default() -> Self {
        Self(std::time::Instant::now())
    }
}

impl Clock for Wall {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// A model loaded on one option, ready to run its made-up input.
pub trait Runner {
    fn run(&mut self) -> Result<Vec<f32>, String>;
}

/// A model as a measurement sees it.
pub trait Probe: Send + Sync {
    /// The options to measure, the reference first.
    fn options(&self) -> Vec<Unit>;
    /// Load on `unit`.
    fn load(&self, unit: Unit) -> Result<Box<dyn Runner>, String>;
    /// Whether `output` is close enough to the reference's.
    fn matches(&self, reference: &[f32], output: &[f32]) -> bool;
}

enum Step {
    Loaded(Duration),
    Ran(Result<(Vec<f32>, Duration), String>),
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn ms_f(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// One option, on a thread of its own so a load past `cap` can be left behind (Core ML can't stop a load midway; the
/// thread drops the model when it ends). The output and the timing, or how it went wrong.
fn one(probe: &Arc<dyn Probe>, clock: &Arc<dyn Clock>, unit: Unit, cap: Duration) -> (Timing, Option<Vec<f32>>) {
    let timing = |outcome, load: Option<Duration>, run: Option<Duration>, error: Option<String>| Timing {
        unit: unit.id().to_string(),
        outcome,
        load_ms: load.map(ms),
        run_ms: run.map(ms_f),
        error,
    };
    let (tx, rx) = mpsc::channel();
    let left = Arc::new(AtomicBool::new(false));
    let (p, c, l) = (probe.clone(), clock.clone(), left.clone());
    let spawned = std::thread::Builder::new().name(format!("measure-{}", unit.id())).spawn(move || {
        let t0 = c.now();
        let mut runner = match p.load(unit) {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Step::Ran(Err(e)));
                return;
            }
        };
        let _ = tx.send(Step::Loaded(c.now().saturating_sub(t0)));
        if l.load(Ordering::Relaxed) {
            return;
        }
        let ran = runner.run().and_then(|_warm| {
            let t1 = c.now();
            let mut runs = 0u32;
            loop {
                let out = runner.run()?;
                runs += 1;
                let spent = c.now().saturating_sub(t1);
                if runs >= MAX_RUNS || spent >= MIN_TIMED {
                    return Ok((out, spent / runs));
                }
            }
        });
        let _ = tx.send(Step::Ran(ran));
    });
    if let Err(e) = spawned {
        return (timing(Outcome::Failed, None, None, Some(e.to_string())), None);
    }
    let load = match rx.recv_timeout(cap) {
        Ok(Step::Loaded(d)) if d > cap => {
            left.store(true, Ordering::Relaxed);
            return (timing(Outcome::LoadCap, Some(d), None, None), None);
        }
        Ok(Step::Loaded(d)) => d,
        Ok(Step::Ran(Err(e))) => return (timing(Outcome::Failed, None, None, Some(e)), None),
        Ok(Step::Ran(Ok(_))) => return (timing(Outcome::Failed, None, None, Some("ran before it loaded".into())), None),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            left.store(true, Ordering::Relaxed);
            return (timing(Outcome::LoadCap, Some(cap), None, None), None);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => return (timing(Outcome::Failed, None, None, Some("the load stopped".into())), None),
    };
    match rx.recv_timeout(RUN_LIMIT) {
        Ok(Step::Ran(Ok((out, run)))) if out.iter().all(|v| v.is_finite()) => (timing(Outcome::Ok, Some(load), Some(run), None), Some(out)),
        Ok(Step::Ran(Ok((_, run)))) => (timing(Outcome::Mismatch, Some(load), Some(run), Some("not a number in the output".into())), None),
        Ok(Step::Ran(Err(e))) => (timing(Outcome::Failed, Some(load), None, Some(e)), None),
        Ok(Step::Loaded(_)) | Err(_) => (timing(Outcome::Failed, Some(load), None, Some("the run did not end".into())), None),
    }
}

/// Measure each of `probe`'s options in turn. `wait` is asked before each one and waits while the photo on screen is
/// being made; `true` stops the measurement (cancelled), which then gives `None`. Without a reference output (the
/// first option failed) the others are still timed but none can match. Once a Neural Engine load went over the cap,
/// the other Neural Engine option is not loaded (it compiles the same way; a load left behind keeps compiling).
pub fn measure(probe: Arc<dyn Probe>, clock: Arc<dyn Clock>, cap: Duration, wait: &dyn Fn() -> bool) -> Option<Vec<Timing>> {
    let mut reference: Option<Vec<f32>> = None;
    let mut out: Vec<Timing> = Vec::new();
    for (i, unit) in probe.options().into_iter().enumerate() {
        if wait() {
            return None;
        }
        let capped = out.iter().find(|t| t.outcome == Outcome::LoadCap && t.unit().is_some_and(Unit::neural_engine));
        if let Some(c) = capped.filter(|_| unit.neural_engine()) {
            let error = format!("not loaded: the load on {} went over the cap", c.unit);
            out.push(Timing { unit: unit.id().to_string(), outcome: Outcome::LoadCap, load_ms: None, run_ms: None, error: Some(error) });
            continue;
        }
        let (mut t, output) = one(&probe, &clock, unit, cap);
        match (i, output) {
            (0, Some(o)) => reference = Some(o),
            (_, Some(o)) if t.outcome == Outcome::Ok && !reference.as_deref().is_some_and(|r| r.len() == o.len() && probe.matches(r, &o)) => {
                t.outcome = Outcome::Mismatch;
            }
            _ => {}
        }
        out.push(t);
    }
    Some(out)
}

/// Mean |a − b| over two outputs of one length (∞ for different lengths).
pub fn mean_abs(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::INFINITY;
    }
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32
}

/// Largest |a − b| (∞ for different lengths).
pub fn max_abs(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::INFINITY;
    }
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}
