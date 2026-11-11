//! Issue #267: the chooser with fake options and a fake clock (no real timing).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use lightcraft_coreml::{ComputeUnits, Error, first_that_loads};

use super::measure::{Clock, End, Probe, Rules, Runner, measure};
use super::{LOAD_CAP, ModelUnits, Outcome, Plan, Timing, Unit, chosen, order};

/// A clock that moves only when a fake model says it took time.
#[derive(Default)]
struct FakeClock(AtomicU64);

impl FakeClock {
    fn spend(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::Relaxed);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::Relaxed))
    }
}

/// One fake option: how long its load and run take, what it gives, and whether it loads.
#[derive(Clone)]
struct Fake {
    unit: Unit,
    load_ms: u64,
    run_ms: u64,
    output: Vec<f32>,
    loads: bool,
}

fn fake(unit: Unit, load_ms: u64, run_ms: u64, output: &[f32]) -> Fake {
    Fake { unit, load_ms, run_ms, output: output.to_vec(), loads: true }
}

struct FakeProbe {
    clock: Arc<FakeClock>,
    options: Vec<Fake>,
    /// The options loaded, in turn.
    loaded: Mutex<Vec<Unit>>,
}

impl FakeProbe {
    fn new(clock: &Arc<FakeClock>, options: Vec<Fake>) -> Arc<Self> {
        Arc::new(Self { clock: clock.clone(), options, loaded: Mutex::default() })
    }

    fn loaded(&self) -> Vec<Unit> {
        self.loaded.lock().unwrap().clone()
    }
}

struct FakeRunner {
    clock: Arc<FakeClock>,
    option: Fake,
}

impl Runner for FakeRunner {
    fn run(&mut self) -> Result<Vec<f32>, String> {
        self.clock.spend(self.option.run_ms);
        Ok(self.option.output.clone())
    }
}

impl Probe for FakeProbe {
    fn options(&self) -> Vec<Unit> {
        self.options.iter().map(|o| o.unit).collect()
    }

    fn load(&self, unit: Unit) -> Result<Box<dyn Runner>, String> {
        let option = self.options.iter().find(|o| o.unit == unit).cloned().ok_or("no such option")?;
        self.loaded.lock().unwrap().push(unit);
        self.clock.spend(option.load_ms);
        if !option.loads {
            return Err("does not load".into());
        }
        Ok(Box::new(FakeRunner { clock: self.clock.clone(), option }))
    }

    fn matches(&self, reference: &[f32], output: &[f32]) -> bool {
        super::measure::max_abs(reference, output) <= 0.01
    }
}

fn rules() -> Rules {
    Rules { cap: LOAD_CAP, wait: Arc::new(|| false), neural_now: Arc::new(|| true), left_behind: Arc::default() }
}

/// Measure `probe` after `done` under `rules`: how it ended, the options last saved and whether they were complete.
fn measure_with(probe: &Arc<FakeProbe>, clock: &Arc<FakeClock>, rules: &Rules, done: Vec<Timing>) -> (End, Vec<Timing>, bool) {
    let saved = Mutex::new((Vec::new(), false));
    let end = measure(probe.clone(), clock.clone(), rules, done, &|o, complete| *saved.lock().unwrap() = (o.to_vec(), complete));
    let (options, complete) = saved.into_inner().unwrap();
    (end, options, complete)
}

fn run(options: Vec<Fake>) -> Vec<Timing> {
    let clock = Arc::new(FakeClock::default());
    let (end, options, complete) = measure_with(&FakeProbe::new(&clock, options), &clock, &rules(), Vec::new());
    assert_eq!((end, complete), (End::Complete, true));
    options
}

const REF: [f32; 3] = [0.1, 0.5, 0.9];

#[test]
fn the_fastest_option_whose_output_matches_is_chosen() {
    let t = run(vec![
        fake(Unit::CoreMlGpu, 2_000, 650, &REF),
        fake(Unit::CoreMlAll, 3_000, 400, &[0.1, 0.505, 0.9]),
        fake(Unit::CoreMlNeuralEngine, 5_000, 300, &[0.3, 0.5, 0.9]),
        fake(Unit::CandleGpu, 1_000, 7_000, &REF),
    ]);
    let outcome = |u: Unit| t.iter().find(|x| x.unit() == Some(u)).map(|x| (x.outcome, x.load_ms, x.run_ms));
    assert_eq!(outcome(Unit::CoreMlGpu), Some((Outcome::Ok, Some(2_000), Some(650.0))));
    assert_eq!(outcome(Unit::CoreMlAll), Some((Outcome::Ok, Some(3_000), Some(400.0))));
    // faster, but not the reference's output: skipped
    assert_eq!(outcome(Unit::CoreMlNeuralEngine), Some((Outcome::Mismatch, Some(5_000), Some(300.0))));
    assert_eq!(chosen(&t), Some((Unit::CoreMlAll, 400.0)));
    // a fast run is timed over several runs: the mean of one run
    let fast = run(vec![fake(Unit::CoreMlGpu, 10, 3, &REF)]);
    assert_eq!(fast[0].run_ms, Some(3.0));
    let record = super::Record { key: "k".into(), options: t, complete: true };
    assert_eq!(
        order(Some(&record), "k", Unit::CoreMlGpu),
        [Unit::CoreMlAll, Unit::CoreMlGpu, Unit::CandleGpu, Unit::CandleCpu],
        "matching options fastest first, the mismatched one never, candle last"
    );
}

#[test]
fn a_failing_chosen_option_hands_over_to_the_next() {
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), fake(Unit::CoreMlNeuralEngine, 10, 300, &REF)]);
    let record = super::Record { key: "k".into(), options: t, complete: true };
    let plan = Plan::from_order(&order(Some(&record), "k", Unit::CoreMlGpu), ComputeUnits::CpuAndGpu);
    assert!(!plan.candle_first);
    assert_eq!(plan.coreml.first(), ComputeUnits::CpuAndNeuralEngine);
    // the chosen units fail to load: the next ones run the model
    let loaded = first_that_loads(plan.coreml, |u| if u == ComputeUnits::CpuAndNeuralEngine { Err(Error::Load("x".into())) } else { Ok(u) });
    assert_eq!(loaded.map(|(_, u)| u), Ok(ComputeUnits::CpuAndGpu));
    // an option that failed when measured is never tried; candle on the CPU stays last whatever was measured
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), Fake { loads: false, ..fake(Unit::CoreMlAll, 10, 1, &REF) }]);
    assert_eq!(t[1].outcome, Outcome::Failed);
    let record = super::Record { key: "k".into(), options: t, complete: true };
    assert_eq!(order(Some(&record), "k", Unit::CoreMlGpu), [Unit::CoreMlGpu, Unit::CandleGpu, Unit::CandleCpu]);
    // candle on the GPU measured fastest: candle first
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), fake(Unit::CandleGpu, 10, 100, &REF)]);
    let record = super::Record { key: "k".into(), options: t, complete: true };
    let plan = Plan::from_order(&order(Some(&record), "k", Unit::CoreMlGpu), ComputeUnits::CpuAndGpu);
    assert!(plan.candle_first);
    assert_eq!(plan.runtime(lightcraft_coreml::Runtime::Automatic), lightcraft_coreml::Runtime::Candle);
    assert_eq!(plan.runtime(lightcraft_coreml::Runtime::CoreMl), lightcraft_coreml::Runtime::CoreMl);
}

#[test]
fn a_new_key_is_measured_again_and_old_measurements_are_not_used() {
    let units = ModelUnits::default();
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), fake(Unit::CoreMlAll, 10, 100, &REF)]);
    units.store("m", "files-1 shape mac", t, true);
    assert!(units.measured("m", "files-1 shape mac"));
    assert_eq!(units.order("m", "files-1 shape mac", Unit::CoreMlGpu)[0], Unit::CoreMlAll);
    // new model files, another macOS or another chip: not measured, and loads as before until it is
    assert!(!units.measured("m", "files-2 shape mac"));
    assert_eq!(units.order("m", "files-2 shape mac", Unit::CoreMlGpu), [Unit::CoreMlGpu, Unit::CandleGpu, Unit::CandleCpu]);
    // and a model never measured loads on its unit of old
    assert_eq!(units.order("other", "k", Unit::CoreMlAll), [Unit::CoreMlAll, Unit::CandleGpu, Unit::CandleCpu]);
    units.forget("m");
    assert!(!units.measured("m", "files-1 shape mac"));
}

#[test]
fn an_option_over_the_load_cap_is_remembered_and_dropped() {
    let cap = LOAD_CAP.as_millis() as u64;
    let t = run(vec![
        fake(Unit::CoreMlGpu, 2_000, 650, &REF),
        fake(Unit::CoreMlNeuralEngine, cap + 167_000, 300, &REF),
        fake(Unit::CoreMlAll, 1, 1, &REF),
    ]);
    assert_eq!(t[1].outcome, Outcome::LoadCap);
    assert_eq!(t[1].run_ms, None, "never run");
    // the other Neural Engine option compiles the same way: not loaded
    assert_eq!((t[2].outcome, t[2].load_ms), (Outcome::LoadCap, None));
    let dir = std::env::temp_dir().join(format!("lightcraft-model-units-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join(super::FILE);
    let units = ModelUnits::default();
    units.open(Some(file.clone()));
    units.store("m", "k", t, true);
    // kept across launches
    let again = ModelUnits::default();
    again.open(Some(file.clone()));
    assert_eq!(again.record("m").map(|r| r.options[1].outcome), Some(Outcome::LoadCap));
    assert!(again.measured("m", "k"));
    assert_eq!(again.order("m", "k", Unit::CoreMlGpu), [Unit::CoreMlGpu, Unit::CandleGpu, Unit::CandleCpu]);
    // the reference itself over the cap: nothing to compare with, and its units are not tried either
    let t = run(vec![fake(Unit::CoreMlAll, cap + 1, 10, &REF), fake(Unit::CoreMlGpu, 10, 650, &REF)]);
    assert_eq!(t[0].outcome, Outcome::LoadCap);
    assert_eq!(t[1].outcome, Outcome::Mismatch);
    let record = super::Record { key: "k".into(), options: t, complete: true };
    assert_eq!(order(Some(&record), "k", Unit::CoreMlAll), [Unit::CandleGpu, Unit::CandleCpu]);
    // a damaged file is no measurement, not a crash
    std::fs::write(&file, b"{not json").unwrap();
    let damaged = ModelUnits::default();
    damaged.open(Some(file));
    assert!(damaged.record("m").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// F4: each option is filed as it finishes; a stop (quit, cancel) loses only the one under way, and the next
/// measurement goes on from there, running only the reference again.
#[test]
fn a_stopped_measurement_keeps_the_options_it_finished() {
    let options = vec![fake(Unit::CoreMlGpu, 10, 650, &REF), fake(Unit::CoreMlAll, 10, 400, &REF), fake(Unit::CandleGpu, 10, 900, &REF)];
    let clock = Arc::new(FakeClock::default());
    let probe = FakeProbe::new(&clock, options.clone());
    // (asked before each option and each run: the 1st option takes 3 asks, the 2nd's warm run is the 5th)
    let asked = Arc::new(AtomicUsize::new(0));
    let a = asked.clone();
    let stop_at_5th = Rules { wait: Arc::new(move || a.fetch_add(1, Ordering::Relaxed) + 1 >= 5), ..rules() };
    let (end, saved, complete) = measure_with(&probe, &clock, &stop_at_5th, Vec::new());
    assert_eq!((end, complete), (End::Stopped, false));
    assert_eq!(saved.iter().map(|t| t.unit()).collect::<Vec<_>>(), [Some(Unit::CoreMlGpu)]);
    // the next launch: the reference again (its output is needed), the rest from where it stopped
    let probe = FakeProbe::new(&clock, options);
    let (end, saved, complete) = measure_with(&probe, &clock, &rules(), saved);
    assert_eq!((end, complete), (End::Complete, true));
    assert_eq!(probe.loaded(), [Unit::CoreMlGpu, Unit::CoreMlAll, Unit::CandleGpu]);
    assert_eq!(chosen(&saved), Some((Unit::CoreMlAll, 400.0)));
    // a measurement that stops before anything finished files nothing
    let probe = FakeProbe::new(&clock, vec![fake(Unit::CoreMlGpu, 10, 10, &REF)]);
    let (end, saved, _) = measure_with(&probe, &clock, &Rules { wait: Arc::new(|| true), ..rules() }, Vec::new());
    assert_eq!((end, saved.len()), (End::Stopped, 0));
    assert!(probe.loaded().is_empty());
}

/// F1: no Neural Engine option loads while a load left behind is still going, nor while `neural_now` says no (a
/// large model, the Mac busy or on battery); the others are measured and used, and the Neural Engine ones come later.
#[test]
fn neural_engine_options_wait_while_a_load_is_left_behind() {
    let options = vec![
        fake(Unit::CoreMlGpu, 10, 650, &REF),
        fake(Unit::CandleGpu, 10, 900, &REF),
        fake(Unit::CoreMlAll, 10, 400, &REF),
        fake(Unit::CoreMlNeuralEngine, 10, 300, &REF),
    ];
    let clock = Arc::new(FakeClock::default());
    for held in [Rules { left_behind: Arc::new(AtomicUsize::new(1)), ..rules() }, Rules { neural_now: Arc::new(|| false), ..rules() }] {
        let probe = FakeProbe::new(&clock, options.clone());
        let (end, saved, complete) = measure_with(&probe, &clock, &held, Vec::new());
        assert_eq!((end, complete), (End::Later, false));
        assert_eq!(probe.loaded(), [Unit::CoreMlGpu, Unit::CandleGpu]);
        let record = super::Record { key: "k".into(), options: saved.clone(), complete };
        assert_eq!(order(Some(&record), "k", Unit::CoreMlGpu)[0], Unit::CoreMlGpu, "what was measured is used meanwhile");
        // later, nothing held: only the Neural Engine ones load (and the reference, for its output)
        let probe = FakeProbe::new(&clock, options.clone());
        let (end, saved, complete) = measure_with(&probe, &clock, &rules(), saved);
        assert_eq!((end, complete), (End::Complete, true));
        assert_eq!(probe.loaded(), [Unit::CoreMlGpu, Unit::CoreMlAll, Unit::CoreMlNeuralEngine]);
        assert_eq!(chosen(&saved), Some((Unit::CoreMlNeuralEngine, 300.0)));
    }
}

/// A probe whose load waits until the test lets it go.
struct HeldProbe {
    release: Mutex<mpsc::Receiver<()>>,
}

impl Probe for HeldProbe {
    fn options(&self) -> Vec<Unit> {
        vec![Unit::CoreMlGpu]
    }

    fn load(&self, _: Unit) -> Result<Box<dyn Runner>, String> {
        let _ = self.release.lock().unwrap().recv();
        Err("let go".into())
    }

    fn matches(&self, _: &[f32], _: &[f32]) -> bool {
        true
    }
}

/// F1: a load past the cap is counted as left behind until it returns.
#[test]
fn a_load_left_behind_is_counted_until_it_returns() {
    let (tx, rx) = mpsc::channel();
    let probe = Arc::new(HeldProbe { release: Mutex::new(rx) });
    // (the load never returns before the test lets it go, so any cap is passed)
    let rules = Rules { cap: Duration::from_millis(1), ..rules() };
    let (end, saved) = {
        let saved = Mutex::new(Vec::new());
        let end = measure(probe, Arc::new(FakeClock::default()), &rules, Vec::new(), &|o, _| *saved.lock().unwrap() = o.to_vec());
        (end, saved.into_inner().unwrap())
    };
    assert_eq!(end, End::Complete);
    assert_eq!(saved[0].outcome, Outcome::LoadCap);
    assert_eq!(rules.left_behind.load(Ordering::Acquire), 1);
    tx.send(()).unwrap();
    // (a hang guard only: the count drops as soon as the load returns)
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while rules.left_behind.load(Ordering::Acquire) != 0 {
        assert!(std::time::Instant::now() < until, "the load left behind was never counted out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
