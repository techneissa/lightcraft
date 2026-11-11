//! Fork feature (`fork/parse-ahead.md`): `parsing.ahead.*`, people parsing ahead of time.
//!
//! - `parsing.ahead.pump {pace, backward?, culling?, enabled?, wait?, timeoutMs?}`: the frontend's per-frame call (cheap when
//!   there is nothing to do); `wait` keeps going until the queue is done or a few seconds have passed (for agents and
//!   scripts, which call again while the state is `working`, `paused` or `loading`: the call runs on the UI thread).
//! - `parsing.ahead.status`: the state, the queue and the counts.

use std::time::{Duration, Instant};

use serde_json::Value;

use super::{CommandSpec, always, cmd};
use crate::parse_ahead::{Pace, pump, status};
use crate::{Result, Session};

/// Longest a waiting pump keeps going (`timeoutMs` may ask for less, never more): it holds the thread that runs
/// commands, the desktop app's UI thread.
const WAIT_MAX_MS: u64 = 3_000;
const WAIT_POLL: Duration = Duration::from_millis(20);

/// How long a waiting pump with these params keeps going.
fn wait_limit(p: &Value) -> Duration {
    Duration::from_millis(p.get("timeoutMs").and_then(Value::as_u64).map_or(WAIT_MAX_MS, |ms| ms.min(WAIT_MAX_MS)))
}

fn run_pump(s: &mut Session, p: &Value) -> Result<Value> {
    let pace = Pace::parse(p.get("pace").and_then(Value::as_str));
    let backward = p.get("backward").and_then(Value::as_bool).unwrap_or(false);
    s.set_parse_ahead_culling(p.get("culling").and_then(Value::as_bool).unwrap_or(false));
    if let Some(on) = p.get("enabled").and_then(Value::as_bool)
        && on != s.parse_ahead.enabled()
    {
        s.set_parse_ahead(on);
    }
    pump(s, pace, backward);
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false) && pace != Pace::Pause;
    if wait {
        let limit = wait_limit(p);
        let started = Instant::now();
        // until nothing is going and nothing the pace allows is left (or the work stands still)
        while matches!(status(s)["state"].as_str(), Some("working" | "paused" | "loading")) && started.elapsed() < limit {
            std::thread::sleep(WAIT_POLL);
            pump(s, pace, backward);
        }
    }
    Ok(status(s))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "parsing.ahead.pump",
            "Find People Parts Ahead",
            [],
            None,
            "{pace: \"pause\"|\"light\"|\"normal\" (what the user is doing: pause while they drag, type or scroll; light while they are around; normal when idle), backward?: bool (the user steps towards the start), culling?: bool (no Edit panel open: nothing near the open photo is parsed), enabled?: bool, wait?: bool, timeoutMs?} — people parsing ahead of time, one photo at a time on a worker thread, into the people parsing cache, for photos likely to be retouched: the open photo (and, once the user has edited it, the 4 photos ahead of it), photos with portrait settings, and (at the normal pace) photos with faces that are picked or starred, newest first; a photo only stepped past is never parsed. Nothing in Low Power Mode, on battery below 40 %, or when off. Cheap to call every frame; `wait` keeps pumping until the queue is done, for 3 s at most (`timeoutMs` may ask for less): call again while the state is working, paused or loading → {enabled, culling, editing (the user has edited since they came to the Edit panel), state: off|battery|lowPower|checkingPower|noModels|noCache|stopped|loading|paused|working|done|idle, active, queued, left (queued photos the pace allows now), queue, working, parsed, cached, skipped, failed, power, indexFile}",
            always,
            run_pump
        ),
        cmd!(
            query "parsing.ahead.status",
            "People Parts Ahead Status",
            [],
            None,
            "{} → as parsing.ahead.pump, without pumping",
            always,
            |s, _| Ok(status(s))
        ),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_waiting_pump_never_holds_the_ui_thread_for_more_than_a_few_seconds() {
        assert_eq!(wait_limit(&json!({})), Duration::from_secs(3));
        assert_eq!(wait_limit(&json!({"timeoutMs": 600_000})), Duration::from_secs(3));
        assert_eq!(wait_limit(&json!({"timeoutMs": 50})), Duration::from_millis(50));
    }
}
