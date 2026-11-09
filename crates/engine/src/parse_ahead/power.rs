//! Whether the Mac's power lets background parsing run: not in Low Power Mode, and not on battery below
//! [`MIN_BATTERY`] %. Read from `pmset` (on the Mac) at most once a minute, on a thread of its own, so the pump never
//! waits for it. On the Mac nothing starts before the first reading is in; elsewhere power never holds the work back.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Below this battery charge (while on battery) nothing is parsed ahead.
pub const MIN_BATTERY: u8 = 40;
/// How long a reading is used.
const FRESH: Duration = Duration::from_secs(60);

/// The power state, as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Power {
    pub on_battery: bool,
    /// The battery's charge in percent, when there is a battery.
    pub percent: Option<u8>,
    pub low_power: bool,
}

impl Power {
    /// What holds background parsing back: `lowPower` or `battery`; `None` when nothing does.
    pub fn holds_back(&self) -> Option<&'static str> {
        if self.low_power {
            Some("lowPower")
        } else if self.on_battery && self.percent.is_some_and(|p| p < MIN_BATTERY) {
            Some("battery")
        } else {
            None
        }
    }
}

/// `pmset -g batt`: whether the Mac draws from its battery, and the charge.
pub fn parse_batt(text: &str) -> (bool, Option<u8>) {
    let on_battery = text.lines().next().is_some_and(|l| l.contains("'Battery Power'"));
    let percent = text.lines().find(|l| l.contains("InternalBattery")).and_then(|l| {
        let end = l.find('%')?;
        let start = l.get(..end)?.rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
        l.get(start..end)?.parse::<u8>().ok()
    });
    (on_battery, percent)
}

/// `pmset -g`: whether Low Power Mode is on (`lowpowermode 1`, or `powermode 1` on newer systems).
pub fn parse_low_power(text: &str) -> bool {
    text.lines().any(|l| {
        let mut words = l.split_whitespace();
        matches!((words.next(), words.next()), (Some("lowpowermode" | "powermode"), Some("1")))
    })
}

/// What holds the work back with `power`, `read` or not yet, on a Mac (`mac`) or not.
fn holds(power: Power, read: bool, mac: bool) -> Option<&'static str> {
    if mac && !read {
        return Some("checkingPower");
    }
    power.holds_back()
}

#[cfg(target_os = "macos")]
fn read_now() -> Power {
    let run = |args: &[&str]| {
        std::process::Command::new("/usr/bin/pmset").args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let (on_battery, percent) = run(&["-g", "batt"]).map_or((false, None), |t| parse_batt(&t));
    let low_power = run(&["-g"]).is_some_and(|t| parse_low_power(&t));
    Power { on_battery, percent, low_power }
}

#[cfg(not(target_os = "macos"))]
fn read_now() -> Power {
    Power::default()
}

#[derive(Default)]
struct State {
    power: Power,
    read_at: Option<Instant>,
    reading: bool,
    /// Tests: this state instead of the Mac's.
    fixed: Option<Power>,
}

/// The power state, refreshed in the background.
#[derive(Clone, Default)]
pub struct PowerWatch {
    state: Arc<Mutex<State>>,
}

impl PowerWatch {
    /// The last reading; a stale one starts a new read on a thread (its result counts from a later call).
    pub fn now(&self) -> Power {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(p) = st.fixed {
            return p;
        }
        if !st.reading && st.read_at.is_none_or(|t| t.elapsed() >= FRESH) {
            st.reading = true;
            let state = self.state.clone();
            let spawned = std::thread::Builder::new().name("parse-ahead-power".into()).spawn(move || {
                let p = read_now();
                let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
                st.power = p;
                st.read_at = Some(Instant::now());
                st.reading = false;
            });
            if spawned.is_err() {
                st.reading = false;
                st.read_at = Some(Instant::now());
            }
        }
        st.power
    }

    /// What holds background parsing back now ([`Power::holds_back`]); on the Mac also `checkingPower` until the
    /// first reading is in (a Mac on a low battery never starts before it is known).
    pub fn holds_back(&self) -> Option<&'static str> {
        let power = self.now();
        let read = {
            let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            st.fixed.is_some() || st.read_at.is_some()
        };
        holds(power, read, cfg!(target_os = "macos"))
    }

    /// Tests: use `power` from now on (`None`: the Mac's again).
    #[cfg(test)]
    pub fn fix(&self, power: Option<Power>) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).fixed = power;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pmset_output_is_read_and_odd_output_reads_as_mains_power() {
        let batt = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t35%; discharging; 2:11 remaining present: true\n";
        assert_eq!(parse_batt(batt), (true, Some(35)));
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t100%; charged; 0:00 remaining present: true\n";
        assert_eq!(parse_batt(ac), (false, Some(100)));
        assert_eq!(parse_batt("Now drawing from 'AC Power'\n"), (false, None));
        assert_eq!(parse_batt(""), (false, None));
        assert_eq!(parse_batt("Now drawing from 'Battery Power'\n -InternalBattery-0 999%;"), (true, None));
        assert!(parse_low_power("System-wide power settings:\n lowpowermode         1\n sleep 1\n"));
        assert!(parse_low_power(" powermode            1\n"));
        assert!(!parse_low_power(" powermode            2\n"));
        assert!(!parse_low_power(" lowpowermode         0\n"));
    }

    #[test]
    fn low_power_mode_and_a_low_battery_hold_the_work_back() {
        assert_eq!(Power::default().holds_back(), None);
        assert_eq!(Power { low_power: true, ..Power::default() }.holds_back(), Some("lowPower"));
        assert_eq!(Power { on_battery: true, percent: Some(MIN_BATTERY - 1), low_power: false }.holds_back(), Some("battery"));
        assert_eq!(Power { on_battery: true, percent: Some(MIN_BATTERY), low_power: false }.holds_back(), None);
        // plugged in with a low battery: charging, nothing to save
        assert_eq!(Power { on_battery: false, percent: Some(5), low_power: false }.holds_back(), None);
    }

    #[test]
    fn a_mac_waits_for_its_first_power_reading() {
        assert_eq!(holds(Power::default(), false, true), Some("checkingPower"));
        assert_eq!(holds(Power::default(), true, true), None);
        assert_eq!(holds(Power { low_power: true, ..Power::default() }, true, true), Some("lowPower"));
        // elsewhere nothing is read: power never holds the work back
        assert_eq!(holds(Power::default(), false, false), None);
    }
}
