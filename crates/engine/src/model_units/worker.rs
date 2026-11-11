//! The thread that measures (issue #267), one model at a time, and [`plan`], which every load asks.
//!
//! A measurement waits while the photo on screen is being made or a job waits on a model, before each option and each
//! run. Its Neural Engine options wait for later while a load left behind past the cap is still going, and for a large
//! model until the app has been idle a while on mains power; the worker then takes other models and comes back to it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use lightcraft_coreml::Runtime;

use super::measure::{self, End, Probe, Rules, Wall};
use super::probes::{ModelProbe, SharedProbe, Spec};
use super::{IDLE_FOR, LOAD_CAP, Plan, Timing, Unit, lock, shared};
use crate::activity::{Activity, Cancel};
use crate::parse_ahead::power::PowerWatch;
use crate::prep::Foreground;

/// How often the worker looks again at models whose Neural Engine options wait for later.
const RETRY: Duration = Duration::from_secs(10);
/// How often a waiting measurement looks again.
const POLL: Duration = Duration::from_millis(50);

/// Loads left behind past the cap, still going (every measurement of the process shares them).
static LEFT_BEHIND: std::sync::OnceLock<Arc<AtomicUsize>> = std::sync::OnceLock::new();

fn left_behind() -> Arc<AtomicUsize> {
    LEFT_BEHIND.get_or_init(Arc::default).clone()
}

/// A model to measure for `key`.
struct Job {
    probe: Arc<ModelProbe>,
    key: String,
    large: bool,
}

/// The thread that measures.
pub(crate) struct Worker {
    tx: mpsc::Sender<Job>,
}

/// What the worker thread holds.
struct Host {
    activity: Activity,
    foreground: Foreground,
    power: PowerWatch,
}

/// Whether a model's Neural Engine options may load now: no load left behind, and for a `large` model the app idle a
/// while on mains power, not in Low Power Mode.
fn neural_now(power: &PowerWatch, large: bool) -> bool {
    left_behind().load(Ordering::Acquire) == 0
        && (!large || {
            let p = power.now();
            shared().idle_for() >= IDLE_FOR && !p.on_battery && !p.low_power
        })
}

impl Host {
    /// Measure `job`; `true` when its Neural Engine options wait for later.
    fn run(&self, job: &Job) -> bool {
        let model = job.probe.spec.model;
        let feature = crate::model_runtime::MODELS.iter().find(|(id, _)| *id == model).map_or(model, |(_, f)| f);
        let cancel = Arc::new(AtomicBool::new(false));
        let _row = self.activity.start_background("modelUnits", &format!("Finding the fastest unit for {feature}"), Cancel::Flag(cancel.clone()));
        let (foreground, stop) = (self.foreground.clone(), cancel.clone());
        // (the photo on screen, or a job waiting on a model: wait; a cancel or the app quitting: stop)
        let wait = move || loop {
            if stop.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down() {
                return true;
            }
            if !foreground.busy() && shared().user_jobs() == 0 {
                return false;
            }
            std::thread::sleep(POLL);
        };
        let (power, large) = (self.power.clone(), job.large);
        let rules =
            Rules { cap: LOAD_CAP, wait: Arc::new(wait), neural_now: Arc::new(move || neural_now(&power, large)), left_behind: left_behind() };
        let done = shared().done(model, &job.key);
        let save = |options: &[Timing], complete: bool| shared().store(model, &job.key, options.to_vec(), complete);
        let probe: Arc<dyn Probe> = Arc::new(SharedProbe(job.probe.clone()));
        let end = measure::measure(probe, Arc::new(Wall::default()), &rules, done, &save);
        log::info!("model units: {model}: {end:?}");
        end == End::Later
    }
}

/// Measure from now on, with rows in `activity`, waiting while `foreground` says the photo on screen is being made, and
/// a large model's Neural Engine options while `power` says battery (the app and MCP hosts do this; a bare session, as
/// in tests, never measures). Once per process.
pub fn attach(activity: Activity, foreground: Foreground, power: PowerWatch) {
    let mut worker = lock(&shared().worker);
    if worker.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel::<Job>();
    let host = Host { activity, foreground, power };
    let spawned = std::thread::Builder::new().name("model-units".into()).spawn(move || work(&host, &rx));
    match spawned {
        Ok(_) => *worker = Some(Worker { tx }),
        Err(e) => log::warn!("model units: no measuring thread ({e})"),
    }
}

/// The worker's loop: new jobs first; when none comes, a model whose Neural Engine options may load now.
fn work(host: &Host, rx: &mpsc::Receiver<Job>) {
    let mut later: Vec<Job> = Vec::new();
    loop {
        let job = match rx.recv_timeout(RETRY) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => match later.iter().position(|j| neural_now(&host.power, j.large)) {
                Some(at) => later.remove(at),
                None => continue,
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let model = job.probe.spec.model;
        shared().set_later(model, false);
        if host.run(&job) {
            shared().set_later(model, true);
            later.push(job);
        } else {
            shared().end(model);
        }
    }
}

/// Measure `spec`'s model now, on this thread, with the real clock, nothing left for later and nothing filed (the
/// `model_units` example prints it).
pub fn measure_now(spec: Spec) -> Result<Vec<Timing>, String> {
    let probe: Arc<dyn Probe> = Arc::new(SharedProbe(ModelProbe::new(spec)?));
    let rules = Rules { cap: LOAD_CAP, wait: Arc::new(|| false), neural_now: Arc::new(|| true), left_behind: Arc::default() };
    let all = std::sync::Mutex::new(Vec::new());
    measure::measure(probe, Arc::new(Wall::default()), &rules, Vec::new(), &|options, _| *lock(&all) = options.to_vec());
    Ok(all.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Queue `spec` for measurement unless it is queued already or nothing measures.
fn request(spec: Spec, key: String) -> bool {
    let units = shared();
    let model = spec.model;
    if !units.begin(model) {
        return true;
    }
    let job = ModelProbe::new(spec).map(|probe| Job { large: probe.large(), probe, key });
    let sent = match job {
        Ok(job) => lock(&units.worker).as_ref().is_some_and(|w| w.tx.send(job).is_ok()),
        Err(e) => {
            log::warn!("model units: {model}: {e}");
            false
        }
    };
    if !sent {
        units.end(model);
    }
    sent
}

/// How a load of `spec`'s model on `runtime` (the Settings choice) goes: the measured order (in part when the
/// measurement is not finished), or the unit of old while the model has not been measured for its files on this Mac,
/// which queues its measurement.
pub fn plan(spec: Spec, runtime: Runtime) -> Plan {
    let old = Plan::default_for(spec.default);
    if runtime == Runtime::Candle || !spec.coreml_ready() {
        return old;
    }
    let key = spec.key();
    let units = shared();
    let plan = Plan::from_order(&units.order(spec.model, &key, Unit::coreml(spec.default)), spec.default);
    if !units.measured(spec.model, &key) {
        request(spec, key);
    }
    // (Core ML only, as Settings asked: no candle first)
    if runtime == Runtime::CoreMl { Plan { candle_first: false, ..plan } } else { plan }
}

/// Measure `spec`'s model again (Settings' Measure Again, `models.measure`).
pub fn measure_again(spec: Spec) -> Result<(), String> {
    if !spec.coreml_ready() {
        return Err("this model's Core ML version is not installed".into());
    }
    if lock(&shared().worker).is_none() {
        return Err("models are measured only in the app".into());
    }
    if shared().measuring(spec.model) {
        return Err("this model is being measured now".into());
    }
    let key = spec.key();
    shared().forget(spec.model);
    if request(spec, key) { Ok(()) } else { Err("the measuring thread stopped".into()) }
}
