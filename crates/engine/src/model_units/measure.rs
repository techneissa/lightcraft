//! Measuring a model on each of its options (issue #267): load (capped), one run to warm it, then timed runs, the
//! output checked against the first option's. The model, the clock and the rules come in from outside, so the tests
//! use fakes.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
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

/// What a measurement waits for and leaves for later.
#[derive(Clone)]
pub struct Rules {
    /// Longest a load may take: one past it is left behind (Core ML can't stop a load) and its option dropped.
    pub cap: Duration,
    /// Asked before each option and before each run: waits while the app needs the Mac (the photo on screen, a job
    /// waiting on a model); `true` stops the measurement.
    pub wait: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Whether a Neural Engine option may load now (a large model's waits until the Mac is idle on mains power);
    /// `false` leaves it for later.
    pub neural_now: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Loads left behind past the cap and still going: no Neural Engine option loads while there is one, so two
    /// Neural Engine compiles never run at once.
    pub left_behind: Arc<AtomicUsize>,
}

/// How a measurement ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Every option was measured.
    Complete,
    /// Some Neural Engine options were left for later ([`Rules::neural_now`] or a load still left behind).
    Later,
    /// `wait` said stop.
    Stopped,
}

enum Step {
    Loaded(Duration),
    Ran(Result<(Vec<f32>, Duration), String>),
    Stopped,
}

/// A load's state, decided once between the measuring thread and the loading one.
const LOADING: u8 = 0;
const LOADED: u8 = 1;
const LEFT: u8 = 2;

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn ms_f(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn timing(unit: Unit, outcome: Outcome, load: Option<Duration>, run: Option<Duration>, error: Option<String>) -> Timing {
    Timing { unit: unit.id().to_string(), outcome, load_ms: load.map(ms), run_ms: run.map(ms_f), error }
}

/// What the loading thread is handed.
struct Loading {
    probe: Arc<dyn Probe>,
    clock: Arc<dyn Clock>,
    unit: Unit,
    rules: Rules,
    state: Arc<AtomicU8>,
    tx: mpsc::Sender<Step>,
    go: mpsc::Receiver<bool>,
}

impl Loading {
    /// Load, say so (unless it was left behind: then it only counts itself out), wait for the go, one warm run, then
    /// timed runs, asking `wait` before each.
    fn run(self) {
        let t0 = self.clock.now();
        let loaded = self.probe.load(self.unit);
        if self.state.compare_exchange(LOADING, LOADED, Ordering::AcqRel, Ordering::Acquire).is_err() {
            // left behind: the measurement counted this load; it ends here, the model dropped
            self.rules.left_behind.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        let mut runner = match loaded {
            Ok(r) => r,
            Err(e) => {
                let _ = self.tx.send(Step::Ran(Err(e)));
                return;
            }
        };
        let _ = self.tx.send(Step::Loaded(self.clock.now().saturating_sub(t0)));
        if self.go.recv() != Ok(true) {
            return;
        }
        let _ = self.tx.send(self.runs(runner.as_mut()));
    }

    /// One warm run, then timed runs until [`MIN_TIMED`] or [`MAX_RUNS`]: the last output and the mean time.
    fn runs(&self, runner: &mut dyn Runner) -> Step {
        let (mut spent, mut timed, mut out) = (Duration::ZERO, 0u32, Vec::new());
        let mut warm = true;
        while warm || (timed < MAX_RUNS && spent < MIN_TIMED) {
            if (self.rules.wait)() {
                return Step::Stopped;
            }
            let t = self.clock.now();
            match runner.run() {
                Ok(o) => out = o,
                Err(e) => return Step::Ran(Err(e)),
            }
            if !warm {
                spent += self.clock.now().saturating_sub(t);
                timed += 1;
            }
            warm = false;
        }
        Step::Ran(Ok((out, spent / timed.max(1))))
    }
}

/// The load's time, or the option's timing when it went no further.
fn wait_for_load(rx: &mpsc::Receiver<Step>, go: &mpsc::Sender<bool>, unit: Unit, rules: &Rules, state: &AtomicU8) -> Result<Duration, Timing> {
    let step = match rx.recv_timeout(rules.cap) {
        Ok(step) => step,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // counted before the state changes, so the thread never counts itself out first
            rules.left_behind.fetch_add(1, Ordering::AcqRel);
            if state.compare_exchange(LOADING, LEFT, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return Err(timing(unit, Outcome::LoadCap, Some(rules.cap), None, None));
            }
            // it loaded just now: not left behind after all
            rules.left_behind.fetch_sub(1, Ordering::AcqRel);
            rx.recv().unwrap_or(Step::Ran(Err("the load stopped".into())))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Step::Ran(Err("the load stopped".into())),
    };
    match step {
        Step::Loaded(d) if d > rules.cap => {
            let _ = go.send(false);
            Err(timing(unit, Outcome::LoadCap, Some(d), None, None))
        }
        Step::Loaded(d) => {
            let _ = go.send(true);
            Ok(d)
        }
        Step::Ran(Err(e)) => Err(timing(unit, Outcome::Failed, None, None, Some(e))),
        Step::Ran(Ok(_)) | Step::Stopped => Err(timing(unit, Outcome::Failed, None, None, Some("ran before it loaded".into()))),
    }
}

/// One option, on a thread of its own so a load past the cap can be left behind. The timing and the output; `None`
/// when `wait` said stop.
fn one(probe: &Arc<dyn Probe>, clock: &Arc<dyn Clock>, unit: Unit, rules: &Rules) -> Option<(Timing, Option<Vec<f32>>)> {
    let (tx, rx) = mpsc::channel();
    let (go_tx, go) = mpsc::channel();
    let state = Arc::new(AtomicU8::new(LOADING));
    let loading = Loading { probe: probe.clone(), clock: clock.clone(), unit, rules: rules.clone(), state: state.clone(), tx, go };
    if let Err(e) = std::thread::Builder::new().name(format!("measure-{}", unit.id())).spawn(move || loading.run()) {
        return Some((timing(unit, Outcome::Failed, None, None, Some(e.to_string())), None));
    }
    let load = match wait_for_load(&rx, &go_tx, unit, rules, &state) {
        Ok(d) => d,
        Err(t) => return Some((t, None)),
    };
    Some(match rx.recv_timeout(RUN_LIMIT) {
        Ok(Step::Ran(Ok((out, run)))) if out.iter().all(|v| v.is_finite()) => (timing(unit, Outcome::Ok, Some(load), Some(run), None), Some(out)),
        Ok(Step::Ran(Ok((_, run)))) => (timing(unit, Outcome::Mismatch, Some(load), Some(run), Some("not a number in the output".into())), None),
        Ok(Step::Ran(Err(e))) => (timing(unit, Outcome::Failed, Some(load), None, Some(e)), None),
        Ok(Step::Stopped) => return None,
        Ok(Step::Loaded(_)) | Err(_) => (timing(unit, Outcome::Failed, Some(load), None, Some("the run did not end".into())), None),
    })
}

/// What happens to option `unit` before it is measured: `Some` skips it (with the timing to add, if any).
fn before_option(out: &[Timing], unit: Unit, rules: &Rules, later: &mut bool) -> Option<Option<Timing>> {
    if !unit.neural_engine() {
        return None;
    }
    if let Some(c) = out.iter().find(|t| t.outcome == Outcome::LoadCap && t.unit().is_some_and(Unit::neural_engine)) {
        let error = format!("not loaded: the load on {} went over the cap", c.unit);
        return Some(Some(timing(unit, Outcome::LoadCap, None, None, Some(error))));
    }
    if rules.left_behind.load(Ordering::Acquire) > 0 || !(rules.neural_now)() {
        *later = true;
        return Some(None);
    }
    None
}

/// Measure each of `probe`'s options in turn, after those in `done` (an earlier measurement of the same key that did
/// not finish: its options are kept, and only the reference runs again, for its output). `save` gets the options so
/// far after each one, and whether all are in, so a quit loses at most the option under way.
///
/// Without a reference output the others are still timed but none can match. A Neural Engine option is left for later
/// while a load is left behind or [`Rules::neural_now`] says no; once one went over the cap, the other is not loaded
/// (it compiles the same way).
pub fn measure(probe: Arc<dyn Probe>, clock: Arc<dyn Clock>, rules: &Rules, done: Vec<Timing>, save: &dyn Fn(&[Timing], bool)) -> End {
    let mut out = done;
    let mut reference: Option<Vec<f32>> = None;
    let mut later = false;
    for (i, unit) in probe.options().into_iter().enumerate() {
        match out.iter().position(|t| t.unit() == Some(unit)) {
            // (the reference runs again for its output, unless it could not run)
            Some(at) if i == 0 && out.get(at).is_some_and(|t| t.outcome == Outcome::Ok) => {
                out.remove(at);
            }
            Some(_) => continue,
            None => {}
        }
        if (rules.wait)() {
            return End::Stopped;
        }
        // (the reference is how the model loads anyway: never left for later)
        if let Some(skip) = before_option(&out, unit, rules, &mut later).filter(|_| i > 0) {
            if let Some(t) = skip {
                out.push(t);
                save(&out, false);
            }
            continue;
        }
        let Some((mut t, output)) = one(&probe, &clock, unit, rules) else { return End::Stopped };
        match (i, output) {
            (0, Some(o)) => reference = Some(o),
            (_, Some(o)) if t.outcome == Outcome::Ok && !reference.as_deref().is_some_and(|r| r.len() == o.len() && probe.matches(r, &o)) => {
                t.outcome = Outcome::Mismatch;
            }
            _ => {}
        }
        out.push(t);
        save(&out, false);
    }
    save(&out, !later);
    if later { End::Later } else { End::Complete }
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
