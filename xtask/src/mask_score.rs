//! `cargo xtask mask-score` (fork, issue #256, `fork/mask-score.md`): score the app's AI masks against human-drawn
//! truth. The engine example `mask_score` makes each photo's mask through the app's commands; `lightcraft-maskscore`
//! measures it; the numbers, a chart and 1:1 sheets of the worst and median photos go into the run's folder.

mod chart;
mod report;
mod sheets;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lightcraft_maskscore::{Mask, Scores, score};
use rayon::prelude::*;
use serde_json::{Value, json};

/// The jobs, each with the methods that make its masks (today's models).
pub const JOBS: &[(&str, &[&str])] =
    &[("subject", &["birefnet", "matting", "vision"]), ("hair", &["birefnet", "matting", "vision"]), ("click", &["sam3"]), ("sky", &["sam3"])];
const CLICKS_SCORED: [u32; 3] = [1, 3, 5];

/// One photo of a test set (`<data>/<job>/<id>/`).
#[derive(Clone)]
pub struct Item {
    pub id: String,
    /// The public set it came from (`meta.json`'s `source.set`), else the job.
    pub set: String,
    pub photo: PathBuf,
    pub truth: PathBuf,
    pub ignore: Option<PathBuf>,
}

/// One scored mask.
#[derive(Clone)]
pub struct Row {
    pub job: String,
    pub method: String,
    /// 0 for a mask without clicks.
    pub clicks: u32,
    pub item: Item,
    pub pred: PathBuf,
    pub found: bool,
    pub seconds: Option<f64>,
    pub scores: Scores,
}

/// What a method's run used (`run.json`).
#[derive(Clone, Default)]
pub struct Used {
    pub job: String,
    pub method: String,
    pub backends: Value,
    pub failed: usize,
    /// Peak memory footprint of the process that made the masks, bytes (macOS).
    pub peak_bytes: Option<u64>,
    /// Size of the Core ML packages the method runs, bytes.
    pub disk_bytes: u64,
}

struct Options {
    data: PathBuf,
    out: PathBuf,
    jobs: Vec<String>,
    methods: Option<Vec<String>>,
    limit: Option<usize>,
    score_only: bool,
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| "HOME is not set".into())
}

fn artcraft() -> Result<PathBuf, String> {
    Ok(home()?.join("Library/Application Support/ArtCraft"))
}

fn options(args: &[&str]) -> Result<Options, String> {
    let datasets = artcraft()?.join("datasets");
    let mut o = Options {
        data: datasets.join("mask-score"),
        out: PathBuf::new(),
        jobs: JOBS.iter().map(|(j, _)| (*j).to_string()).collect(),
        methods: None,
        limit: None,
        score_only: false,
    };
    let (mut name, mut out) = ("today".to_string(), None);
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        let mut value = || it.next().map(|v| (*v).to_string()).ok_or_else(|| format!("{a} needs a value"));
        let list = |v: String| v.split(',').map(str::to_string).collect::<Vec<_>>();
        match a {
            "--data" => o.data = value()?.into(),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--name" => name = value()?,
            "--job" => o.jobs = list(value()?),
            "--method" => o.methods = Some(list(value()?)),
            "--limit" => o.limit = Some(value()?.parse().map_err(|e| format!("--limit: {e}"))?),
            "--score-only" => o.score_only = true,
            _ => return Err(format!("unknown option `{a}`")),
        }
    }
    o.out = out.unwrap_or_else(|| datasets.join("mask-score-runs").join(name));
    if let Some(j) = o.jobs.iter().find(|j| !JOBS.iter().any(|(k, _)| k == j)) {
        return Err(format!("unknown job `{j}`"));
    }
    Ok(o)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The job's photos, in id order.
fn items(data: &Path, job: &str) -> Result<Vec<Item>, String> {
    let dir = data.join(job);
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e} (fetch the sets first: fork/mask-score.md)", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(id) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        let meta = path.join("meta.json");
        if !meta.is_file() {
            continue;
        }
        let set = read_json(&meta)?["source"]["set"].as_str().unwrap_or(job).to_string();
        let photo = ["photo.png", "photo.jpg"].iter().map(|n| path.join(n)).find(|p| p.is_file()).ok_or(format!("{id}: no photo"))?;
        let ignore = Some(path.join("ignore.png")).filter(|p| p.is_file());
        out.push(Item { id, set, photo, truth: path.join("mask.png"), ignore });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    if out.is_empty() {
        return Err(format!("no photos in {}", dir.display()));
    }
    Ok(out)
}

fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path).map(|d| d.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0)
}

/// The Core ML packages a job's method loads, below the models folder.
fn packages(job: &str, method: &str) -> &'static [&'static str] {
    const BIREFNET: &str = "birefnet/coreml/birefnet.mlpackage";
    const MATTING: &str = "birefnet-matting/coreml/birefnet-matting.mlpackage";
    match (job, method) {
        (_, "birefnet" | "vision") => &[BIREFNET],
        (_, "matting") => &[BIREFNET, MATTING],
        ("click", "sam3") => &["sam3-coreml/sam3-encoder.mlpackage", "sam3-coreml/sam3-clickfeat.mlpackage", "sam3-coreml/sam3-clickdec.mlpackage"],
        ("sky", "sam3") => &[
            "sam3-coreml/sam3-encoder.mlpackage",
            "sam3-coreml/sam3-textfeat.mlpackage",
            "sam3-coreml/sam3-textscore.mlpackage",
            "sam3-coreml/sam3-textmask.mlpackage",
        ],
        _ => &[],
    }
}

fn disk_bytes(job: &str, method: &str) -> Result<u64, String> {
    let models = match std::env::var_os("LIGHTCRAFT_BIREFNET_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => artcraft()?.join("models"),
    };
    Ok(packages(job, method).iter().map(|p| size_of(&models.join(p))).sum())
}

/// Build the engine example and give its path.
fn build() -> Result<PathBuf, String> {
    let mut c = crate::cargo();
    c.args(["build", "--release", "-p", "lightcraft-engine", "--features", "sam", "--example", "mask_score"]);
    crate::run(c, "cargo build --release -p lightcraft-engine --features sam --example mask_score")?;
    let target = crate::metadata()?["target_directory"].as_str().map(PathBuf::from).ok_or("cargo metadata: no target directory")?;
    Ok(target.join("release/examples/mask_score"))
}

/// `/usr/bin/time -l`'s "peak memory footprint" line.
fn peak_footprint(stderr: &str) -> Option<u64> {
    stderr.lines().find(|l| l.contains("peak memory footprint")).and_then(|l| l.split_whitespace().next()?.parse().ok())
}

/// Make `items`' masks into `dir`.
fn predict(binary: &Path, o: &Options, job: &str, method: &str, items: &[Item], dir: &Path) -> Result<(), String> {
    // (masks of an earlier run must not be scored as this one's; only a folder this tool made is cleared)
    if dir.join("items.json").is_file() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let list: Vec<Value> = items.iter().map(|i| json!({"id": i.id, "photo": i.photo, "truth": i.truth, "ignore": i.ignore})).collect();
    let list_path = dir.join("items.json");
    write(&list_path, &Value::Array(list).to_string())?;
    let log = dir.join("log.txt");
    let stderr = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mac = cfg!(target_os = "macos");
    let mut c = if mac { Command::new("/usr/bin/time") } else { Command::new(binary) };
    if mac {
        c.arg("-l").arg(binary);
    }
    c.args([job, method]).arg(&list_path).arg(dir);
    // (settings of its own: the app's are neither read nor written)
    c.env("LIGHTCRAFT_CONFIG_DIR", o.out.join("config").join(format!("{job}-{method}"))).stderr(Stdio::from(stderr));
    eprintln!("$ mask_score {job} {method} ({} photos; log: {})", items.len(), log.display());
    let status = c.status().map_err(|e| format!("mask_score: {e}"))?;
    // (2: some photos failed; they are listed and left out of the numbers)
    if !matches!(status.code(), Some(0 | 2)) || !dir.join("predict.json").is_file() {
        return Err(format!("mask_score {job} {method} failed ({status}); see {}", log.display()));
    }
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    if let Some(peak) = peak_footprint(&text) {
        write(&dir.join("peak.txt"), &peak.to_string())?;
    }
    Ok(())
}

fn grey(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.into_luma8();
    Ok((img.width() as usize, img.height() as usize, img.into_raw()))
}

fn mask(path: &Path) -> Result<Mask, String> {
    let (w, h, v) = grey(path)?;
    Mask::from_grey(w, h, &v).ok_or_else(|| format!("{}: empty", path.display()))
}

pub fn ignore_of(item: &Item) -> Result<Option<Vec<bool>>, String> {
    item.ignore.as_deref().map(|p| grey(p).map(|(_, _, v)| v.iter().map(|&v| v >= 128).collect())).transpose()
}

fn score_one(pred: &Path, item: &Item) -> Result<Scores, String> {
    let (p, t) = (mask(pred)?, mask(&item.truth)?);
    let ignore = ignore_of(item)?;
    score(&p, &t, ignore.as_deref())
        .ok_or_else(|| format!("{}: the mask is {}×{}, the truth {}×{}", pred.display(), p.width(), p.height(), t.width(), t.height()))
}

/// Score the masks in `dir` (made by [`predict`], or put there by hand: `<id>.png`, `<id>@N.png` for clicks).
fn score_method(job: &str, method: &str, items: &[Item], dir: &Path) -> Result<(Vec<Row>, Used), String> {
    // (a candidate's masks made elsewhere have no predict.json)
    let report = read_json(&dir.join("predict.json")).unwrap_or(Value::Null);
    let of = |id: &str| report["items"].as_array().and_then(|a| a.iter().find(|v| v["id"] == id));
    let counts: &[u32] = if job == "click" { &CLICKS_SCORED } else { &[0] };
    let mut wanted = Vec::new();
    let mut failed = 0;
    for item in items {
        let made = of(&item.id);
        if made.is_some_and(|m| m.get("error").is_some()) {
            failed += 1;
            continue;
        }
        for &clicks in counts {
            let name = if clicks == 0 { format!("{}.png", item.id) } else { format!("{}@{clicks}.png", item.id) };
            let pred = dir.join(name);
            if !pred.is_file() {
                return Err(format!("missing {} (run without --score-only)", pred.display()));
            }
            // (seconds of the whole mask, or of the clicks up to this one)
            let seconds = made.and_then(|m| m["seconds"].as_array()).map(|a| {
                let n = if clicks == 0 { a.len() } else { (clicks as usize).min(a.len()) };
                a.iter().take(n).filter_map(Value::as_f64).sum()
            });
            let found = made.is_none_or(|m| m["found"].as_bool().unwrap_or(true));
            wanted.push((item.clone(), clicks, pred, found, seconds));
        }
    }
    let rows: Result<Vec<Row>, String> = wanted
        .into_par_iter()
        .map(|(item, clicks, pred, found, seconds)| {
            let scores = score_one(&pred, &item)?;
            Ok(Row { job: job.into(), method: method.into(), clicks, item, pred, found, seconds, scores })
        })
        .collect();
    let peak_bytes = std::fs::read_to_string(dir.join("peak.txt")).ok().and_then(|t| t.trim().parse().ok());
    let used = Used {
        job: job.into(),
        method: method.into(),
        backends: report["backends"].clone(),
        failed,
        peak_bytes,
        disk_bytes: disk_bytes(job, method)?,
    };
    Ok((rows?, used))
}

fn run_all(o: &Options) -> Result<(), String> {
    let binary = if o.score_only { None } else { Some(build()?) };
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let (mut rows, mut used) = (Vec::new(), Vec::new());
    for &(job, methods) in JOBS.iter().filter(|(j, _)| o.jobs.iter().any(|x| x == j)) {
        let mut photos = items(&o.data, job)?;
        if let Some(n) = o.limit {
            // (every k-th, so a short run still sees every set)
            let k = (photos.len() / n.max(1)).max(1);
            photos = photos.into_iter().step_by(k).take(n).collect();
        }
        // (a candidate's masks may carry any method name when only scoring)
        let chosen: Vec<String> = match &o.methods {
            Some(m) => m.iter().filter(|m| o.score_only || methods.contains(&m.as_str())).cloned().collect(),
            None => methods.iter().map(|m| (*m).to_string()).collect(),
        };
        for method in &chosen {
            let dir = o.out.join(job).join(method);
            if o.score_only && !dir.is_dir() {
                continue;
            }
            if let Some(binary) = &binary {
                predict(binary, o, job, method, &photos, &dir)?;
            }
            let (r, u) = score_method(job, method, &photos, &dir)?;
            eprintln!("{job}/{method}: {} masks scored, {} failed", r.len(), u.failed);
            rows.extend(r);
            used.push(u);
        }
    }
    if rows.is_empty() {
        return Err("nothing to score (check --job, --method and the run folder)".into());
    }
    report::save(&o.out, &rows, &used)?;
    chart::save(&o.out.join("chart.png"), &rows)?;
    sheets::save(&o.out.join("sheets"), &rows)?;
    println!("{}", std::fs::read_to_string(o.out.join("summary.md")).unwrap_or_default());
    println!("results: {}", o.out.display());
    let failed: usize = used.iter().map(|u| u.failed).sum();
    if failed > 0 { Err(format!("{failed} photo(s) failed; see the logs in the run folder")) } else { Ok(()) }
}

pub fn run(args: &[&str]) -> Result<(), String> {
    match args {
        ["--compare", a, b] => report::compare(Path::new(a), Path::new(b)),
        _ => run_all(&options(args)?),
    }
}
