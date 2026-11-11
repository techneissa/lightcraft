//! Issue #267: the chooser with fake options and a fake clock (no real timing).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lightcraft_coreml::{ComputeUnits, Error, first_that_loads};

use super::measure::{Clock, Probe, Runner, measure};
use super::{LOAD_CAP, ModelUnits, Outcome, Plan, Unit, chosen, order};

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

fn run(options: Vec<Fake>) -> Vec<super::Timing> {
    let clock = Arc::new(FakeClock::default());
    let probe = Arc::new(FakeProbe { clock: clock.clone(), options });
    measure(probe, clock, LOAD_CAP, &|| false).unwrap()
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
    let record = super::Record { key: "k".into(), options: t };
    assert_eq!(
        order(Some(&record), "k", Unit::CoreMlGpu),
        [Unit::CoreMlAll, Unit::CoreMlGpu, Unit::CandleGpu, Unit::CandleCpu],
        "matching options fastest first, the mismatched one never, candle last"
    );
}

#[test]
fn a_failing_chosen_option_hands_over_to_the_next() {
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), fake(Unit::CoreMlNeuralEngine, 10, 300, &REF)]);
    let record = super::Record { key: "k".into(), options: t };
    let plan = Plan::from_order(&order(Some(&record), "k", Unit::CoreMlGpu), ComputeUnits::CpuAndGpu);
    assert!(!plan.candle_first);
    assert_eq!(plan.coreml.first(), ComputeUnits::CpuAndNeuralEngine);
    // the chosen units fail to load: the next ones run the model
    let loaded = first_that_loads(plan.coreml, |u| if u == ComputeUnits::CpuAndNeuralEngine { Err(Error::Load("x".into())) } else { Ok(u) });
    assert_eq!(loaded.map(|(_, u)| u), Ok(ComputeUnits::CpuAndGpu));
    // an option that failed when measured is never tried; candle on the CPU stays last whatever was measured
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), Fake { loads: false, ..fake(Unit::CoreMlAll, 10, 1, &REF) }]);
    assert_eq!(t[1].outcome, Outcome::Failed);
    let record = super::Record { key: "k".into(), options: t };
    assert_eq!(order(Some(&record), "k", Unit::CoreMlGpu), [Unit::CoreMlGpu, Unit::CandleGpu, Unit::CandleCpu]);
    // candle on the GPU measured fastest: candle first
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), fake(Unit::CandleGpu, 10, 100, &REF)]);
    let record = super::Record { key: "k".into(), options: t };
    let plan = Plan::from_order(&order(Some(&record), "k", Unit::CoreMlGpu), ComputeUnits::CpuAndGpu);
    assert!(plan.candle_first);
    assert_eq!(plan.runtime(lightcraft_coreml::Runtime::Automatic), lightcraft_coreml::Runtime::Candle);
    assert_eq!(plan.runtime(lightcraft_coreml::Runtime::CoreMl), lightcraft_coreml::Runtime::CoreMl);
}

#[test]
fn a_new_key_is_measured_again_and_old_measurements_are_not_used() {
    let units = ModelUnits::default();
    let t = run(vec![fake(Unit::CoreMlGpu, 10, 650, &REF), fake(Unit::CoreMlAll, 10, 100, &REF)]);
    units.store("m", "files-1 shape mac", t);
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
    units.store("m", "k", t);
    // kept across launches
    let again = ModelUnits::default();
    again.open(Some(file.clone()));
    assert_eq!(again.record("m").map(|r| r.options[1].outcome), Some(Outcome::LoadCap));
    assert!(again.measured("m", "k"));
    assert_eq!(again.order("m", "k", Unit::CoreMlGpu), [Unit::CoreMlGpu, Unit::CandleGpu, Unit::CandleCpu]);
    // the reference itself over the cap: nothing to compare with, and the model loads as it did before
    let t = run(vec![fake(Unit::CoreMlAll, cap + 1, 10, &REF), fake(Unit::CoreMlGpu, 10, 650, &REF)]);
    assert_eq!(t[0].outcome, Outcome::LoadCap);
    assert_eq!(t[1].outcome, Outcome::Mismatch);
    let record = super::Record { key: "k".into(), options: t };
    assert_eq!(order(Some(&record), "k", Unit::CoreMlAll), [Unit::CoreMlAll, Unit::CandleGpu, Unit::CandleCpu]);
    // a damaged file is no measurement, not a crash
    std::fs::write(&file, b"{not json").unwrap();
    let damaged = ModelUnits::default();
    damaged.open(Some(file));
    assert!(damaged.record("m").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stopped_measurement_files_nothing() {
    let clock = Arc::new(FakeClock::default());
    let probe = Arc::new(FakeProbe { clock: clock.clone(), options: vec![fake(Unit::CoreMlGpu, 10, 10, &REF)] });
    assert!(measure(probe, clock, LOAD_CAP, &|| true).is_none());
}
