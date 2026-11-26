//! The run's tables: `scores.csv` (one row a mask), `summary.json` and `summary.md` (one row a job, method, click
//! count and set), and the photo-by-photo comparison of two runs.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::{Row, Used, write};

const COLUMNS: &str = "job,method,clicks,id,set,found,iou,bg_iou,boundary_f,band_iou,gaps_kept,sad,mse,seconds";
/// A photo this much worse than before breaks the ship rule of the training plan (#422).
const MUCH_WORSE: f64 = 0.10;

/// The means over some rows.
pub struct Group {
    pub job: String,
    pub method: String,
    pub clicks: u32,
    /// `all`, or one public set of a job that mixes several.
    pub set: String,
    pub photos: usize,
    pub not_found: usize,
    pub iou: f64,
    pub iou_median: f64,
    pub bg_iou: f64,
    pub boundary_f: f64,
    pub band_iou: f64,
    pub gaps_kept: Option<f64>,
    pub sad: f64,
    pub mse: f64,
    pub seconds_median: Option<f64>,
    pub worst: Vec<String>,
}

fn mean(v: impl Iterator<Item = f64>) -> Option<f64> {
    let (mut sum, mut n) = (0.0, 0u32);
    for x in v {
        sum += x;
        n += 1;
    }
    (n > 0).then(|| sum / f64::from(n))
}

pub fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let hi = *v.get(n / 2)?;
    let lo = *v.get(n.checked_sub(1)? / 2)?;
    Some((lo + hi) / 2.0)
}

/// How bad a mask is, for ranking (larger is worse): hair by its matte error, the rest by overlap.
pub fn badness(r: &Row) -> f64 {
    if r.job == "hair" { r.scores.mse } else { 1.0 - r.scores.iou }
}

fn group(set: &str, rows: &[&Row]) -> Option<Group> {
    let first = rows.first()?;
    let over = |f: fn(&Row) -> f64| mean(rows.iter().map(|r| f(r))).unwrap_or(0.0);
    let mut ranked = rows.to_vec();
    ranked.sort_by(|a, b| badness(b).total_cmp(&badness(a)));
    Some(Group {
        job: first.job.clone(),
        method: first.method.clone(),
        clicks: first.clicks,
        set: set.to_string(),
        photos: rows.len(),
        not_found: rows.iter().filter(|r| !r.found).count(),
        iou: over(|r| r.scores.iou),
        iou_median: median(rows.iter().map(|r| r.scores.iou).collect()).unwrap_or(0.0),
        bg_iou: over(|r| r.scores.bg_iou),
        boundary_f: over(|r| r.scores.boundary_f),
        band_iou: over(|r| r.scores.band_iou),
        gaps_kept: mean(rows.iter().filter_map(|r| r.scores.gaps_kept)),
        sad: over(|r| r.scores.sad),
        mse: over(|r| r.scores.mse),
        seconds_median: median(rows.iter().filter_map(|r| r.seconds).collect()),
        worst: ranked.iter().take(5).map(|r| r.item.id.clone()).collect(),
    })
}

/// The groups in the order the rows came, each job's methods overall, then per set when a job mixes sets.
pub fn groups(rows: &[Row]) -> Vec<Group> {
    let mut order: Vec<(&str, &str, u32)> = Vec::new();
    for r in rows {
        let key = (r.job.as_str(), r.method.as_str(), r.clicks);
        if !order.contains(&key) {
            order.push(key);
        }
    }
    let mut out = Vec::new();
    for (job, method, clicks) in order {
        let of: Vec<&Row> = rows.iter().filter(|r| r.job == job && r.method == method && r.clicks == clicks).collect();
        out.extend(group("all", &of));
        let mut sets: Vec<&str> = of.iter().map(|r| r.item.set.as_str()).collect();
        sets.sort_unstable();
        sets.dedup();
        if sets.len() > 1 {
            for set in sets {
                out.extend(group(set, &of.iter().copied().filter(|r| r.item.set == set).collect::<Vec<_>>()));
            }
        }
    }
    out
}

fn number(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "–".into(), |v| format!("{v:.digits$}"))
}

fn csv(rows: &[Row]) -> String {
    let mut out = format!("{COLUMNS}\n");
    for r in rows {
        let s = &r.scores;
        out += &format!(
            "{},{},{},{},{},{},{:.5},{:.5},{:.5},{:.5},{},{:.4},{:.6},{}\n",
            r.job,
            r.method,
            r.clicks,
            r.item.id,
            r.item.set,
            r.found,
            s.iou,
            s.bg_iou,
            s.boundary_f,
            s.band_iou,
            s.gaps_kept.map_or_else(String::new, |v| format!("{v:.5}")),
            s.sad,
            s.mse,
            r.seconds.map_or_else(String::new, |v| format!("{v:.3}")),
        );
    }
    out
}

type Column = (&'static str, fn(&Group) -> String);

/// The columns that say something about a job's masks.
fn columns(job: &str) -> Vec<Column> {
    let iou: [Column; 2] = [("IoU mean", |g| format!("{:.3}", g.iou)), ("IoU median", |g| format!("{:.3}", g.iou_median))];
    let edges: [Column; 2] = [("boundary F", |g| format!("{:.3}", g.boundary_f)), ("edge-band IoU", |g| format!("{:.3}", g.band_iou))];
    let matte: [Column; 2] = [("SAD", |g| format!("{:.2}", g.sad)), ("MSE ×1000", |g| format!("{:.1}", g.mse * 1000.0))];
    let gaps: Column = ("gaps kept", |g| number(g.gaps_kept, 3));
    let tail: [Column; 2] = [("seconds, median", |g| number(g.seconds_median, 2)), ("nothing found", |g| g.not_found.to_string())];
    let mut out: Vec<Column> = Vec::new();
    match job {
        "hair" => {
            out.extend(matte);
            out.extend(edges);
            out.extend(iou);
            out.push(gaps);
        }
        "subject" => {
            out.extend(iou);
            out.extend(edges);
            out.push(gaps);
            out.push(("Background IoU", |g| format!("{:.3}", g.bg_iou)));
        }
        _ => {
            out.extend(iou);
            out.extend(edges);
        }
    }
    out.extend(tail);
    out
}

fn markdown(groups: &[Group], used: &[Used]) -> String {
    let mut out = String::new();
    let mut job = "";
    for g in groups {
        let columns = columns(&g.job);
        if g.job != job {
            job = &g.job;
            let names: Vec<&str> = columns.iter().map(|c| c.0).collect();
            out += &format!("\n### {job}\n\n| method | set | photos | {} |\n", names.join(" | "));
            out += &format!("|---|---|---|{}\n", "---|".repeat(columns.len()));
        }
        let method = match g.clicks {
            0 => g.method.clone(),
            n => format!("{} @{n}", g.method),
        };
        let values: Vec<String> = columns.iter().map(|c| (c.1)(g)).collect();
        out += &format!("| {method} | {} | {} | {} |\n", g.set, g.photos, values.join(" | "));
    }
    out += "\n### What ran\n\n| job | method | Core ML packages on disk, MB | peak memory of the run, GB | engines | failed |\n|---|---|---|---|---|---|\n";
    for u in used {
        let engines: Vec<String> =
            u.backends.as_object().into_iter().flatten().filter_map(|(k, v)| v.as_str().map(|v| format!("{k}: {v}"))).collect();
        out += &format!(
            "| {} | {} | {:.0} | {} | {} | {} |\n",
            u.job,
            u.method,
            u.disk_bytes as f64 / 1e6,
            number(u.peak_bytes.map(|b| b as f64 / 1e9), 1),
            if engines.is_empty() { "–".into() } else { engines.join(", ") },
            u.failed,
        );
    }
    out += "\n### Worst photos\n\n";
    for g in groups.iter().filter(|g| g.set == "all") {
        let clicks = if g.clicks == 0 { String::new() } else { format!(" @{}", g.clicks) };
        out += &format!("- {} {}{clicks}: {}\n", g.job, g.method, g.worst.join(", "));
    }
    out
}

fn summary_json(groups: &[Group], used: &[Used]) -> Value {
    let list: Vec<Value> = groups
        .iter()
        .map(|g| {
            json!({"job": g.job, "method": g.method, "clicks": g.clicks, "set": g.set, "photos": g.photos, "notFound": g.not_found,
                "iou": g.iou, "iouMedian": g.iou_median, "backgroundIou": g.bg_iou, "boundaryF": g.boundary_f, "bandIou": g.band_iou,
                "gapsKept": g.gaps_kept, "sad": g.sad, "mse": g.mse, "secondsMedian": g.seconds_median, "worst": g.worst})
        })
        .collect();
    let runs: Vec<Value> = used
        .iter()
        .map(|u| {
            json!({"job": u.job, "method": u.method, "backends": u.backends, "failed": u.failed, "peakBytes": u.peak_bytes,
                "diskBytes": u.disk_bytes})
        })
        .collect();
    json!({"groups": list, "runs": runs})
}

pub fn save(out: &Path, rows: &[Row], used: &[Used]) -> Result<(), String> {
    let groups = groups(rows);
    write(&out.join("scores.csv"), &csv(rows))?;
    write(&out.join("summary.json"), &format!("{:#}\n", summary_json(&groups, used)))?;
    write(&out.join("summary.md"), &markdown(&groups, used))
}

/// (job, method, clicks, id) → (IoU, boundary F, SAD) of a run's `scores.csv`.
type Scores = BTreeMap<(String, String, String, String), (f64, f64, f64)>;

fn parse(csv: &str) -> Result<Scores, String> {
    let mut lines = csv.lines();
    if lines.next() != Some(COLUMNS) {
        return Err("not a scores.csv of this tool".into());
    }
    lines
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let n = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).ok_or_else(|| format!("bad row: {l}"));
            let t = |i: usize| f.get(i).map(|v| (*v).to_string()).ok_or_else(|| format!("bad row: {l}"));
            Ok(((t(0)?, t(1)?, t(2)?, t(3)?), (n(6)?, n(8)?, n(11)?)))
        })
        .collect()
}

/// Print run `b` against run `a`: the change of each mean, and the photos that got much worse.
pub fn compare(a: &Path, b: &Path) -> Result<(), String> {
    let load = |dir: &Path| {
        let path = dir.join("scores.csv");
        parse(&std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?)
    };
    let (old, new) = (load(a)?, load(b)?);
    // (only photos both runs scored)
    let mut by_group: BTreeMap<(&str, &str, &str), Vec<(&str, [f64; 3])>> = BTreeMap::new();
    for (key, &(iou, f, sad)) in &new {
        if let Some(&(iou0, f0, sad0)) = old.get(key) {
            by_group.entry((&key.0, &key.1, &key.2)).or_default().push((&key.3, [iou - iou0, f - f0, sad - sad0]));
        }
    }
    if by_group.is_empty() {
        return Err("the two runs share no photo of the same job and method".into());
    }
    let mut csv = String::from("job,method,clicks,id,iou_change,boundary_f_change,sad_change\n");
    println!("| job | method | clicks | photos | IoU | boundary F | SAD | photos {MUCH_WORSE} IoU worse |");
    println!("|---|---|---|---|---|---|---|---|");
    for ((job, method, clicks), photos) in &by_group {
        let over = |i: usize| mean(photos.iter().map(|(_, d)| d[i])).unwrap_or(0.0);
        let worse: Vec<&str> = photos.iter().filter(|(_, d)| d[0] < -MUCH_WORSE).map(|(id, _)| *id).collect();
        println!(
            "| {job} | {method} | {clicks} | {} | {:+.4} | {:+.4} | {:+.3} | {} |",
            photos.len(),
            over(0),
            over(1),
            over(2),
            if worse.is_empty() { "none".into() } else { worse.join(" ") }
        );
        for (id, d) in photos {
            csv += &format!("{job},{method},{clicks},{id},{:.5},{:.5},{:.4}\n", d[0], d[1], d[2]);
        }
    }
    let path = b.join("compare.csv");
    write(&path, &csv)?;
    println!("photo by photo: {}", path.display());
    Ok(())
}
