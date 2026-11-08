//! techneissa's fork only (fork/in-app-update.md): the update check asks GitHub for the fork's own build, the
//! release `custom-build` of techneissa/lightcraft, and never for the official LightCraft release, whose
//! app would not have the fork's features. Update hands over to the "Update LightCraft" app, the one that
//! installs the fork's build (`~/scripts/craft/update-craft.sh`) and opens LightCraft again.
//!
//! The release's notes say which `custom` commit the app in it was built from (`commit: <sha>`, or
//! `commit: updating` while a build is uploaded). The running app knows its own commit from the
//! `LightCraftBuildCommit` key of its Info.plist, which `packaging/macos/package.sh` writes for every
//! build, GitHub's and the Mac's. The release is newer when GitHub's compare API says it is `ahead`.

use std::sync::Arc;

use serde_json::Value;

use crate::LightcraftApp;

/// The fork on GitHub.
pub const REPO: &str = "techneissa/lightcraft";
/// The tag of the release holding the newest build.
pub const RELEASE_TAG: &str = "custom-build";
/// The app that installs a build (its name as `open -a` takes it).
pub const UPDATER_APP: &str = "Update LightCraft";
/// The row in the activity stack while GitHub is asked.
pub const LABEL: &str = "Checking for updates";

/// A longest answer read from GitHub (the compare answer lists commits and files; `per_page=1` keeps it small).
#[cfg(not(test))]
const MAX_BODY: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------------------------------------
// Pure parts

/// What the release's notes say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Release {
    /// A build made from this commit.
    Ready(String),
    /// A build is being uploaded (`commit: updating`).
    Building,
    /// No build (no `commit:` line, `commit: none`, or something that is not a commit).
    None,
}

/// A full commit id: 40 hex digits (lower-cased), nothing else.
pub fn clean_sha(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

/// The `commit:` line of the release's notes.
pub fn parse_release_body(body: &str) -> Release {
    let Some(value) = body.lines().find_map(|l| l.trim().strip_prefix("commit:")) else {
        return Release::None;
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("updating") {
        return Release::Building;
    }
    clean_sha(value).map_or(Release::None, Release::Ready)
}

/// The commit an app was built from, from its Info.plist text (`LightCraftBuildCommit`); `None` when the key is
/// missing or holds `unknown` or anything but a commit id.
pub fn parse_plist_commit(plist: &str) -> Option<String> {
    let after = plist.split_once("<key>LightCraftBuildCommit</key>")?.1;
    let value = after.split_once("<string>")?.1.split_once("</string>")?.0;
    clean_sha(value)
}

/// What GitHub's compare answer says about the app's commit (base) and the release's (head).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    /// The release's commit has commits the app's lacks, and the app's has none the release's lacks.
    Ahead,
    /// Same commit, the app is newer, or both have commits of their own.
    NotAhead,
}

/// The `status` of a compare answer: `ahead` is newer, the other known statuses are not.
pub fn parse_compare(json: &str) -> Option<Compare> {
    let v: Value = serde_json::from_str(json).ok()?;
    match v.get("status")?.as_str()? {
        "ahead" => Some(Compare::Ahead),
        "identical" | "behind" | "diverged" => Some(Compare::NotAhead),
        _ => None,
    }
}

/// Where the update check stands.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Status {
    /// Not asked yet (or the check at start found nothing to say).
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A build is ready. `unknown`: the app's commit is not known to GitHub, so nothing could be compared.
    Ready {
        unknown: bool,
    },
    /// GitHub is still making the build.
    Building,
    /// The updater app was opened.
    Updating,
    /// Why the check or the update failed, in a few words.
    Failed(String),
}

impl Status {
    /// The one-line status, in Title Case.
    pub fn text(&self) -> String {
        let t = |s: &str| crate::i18n::tr(s).to_string();
        match self {
            Status::Idle => String::new(),
            Status::Checking => t("Checking…"),
            Status::UpToDate => t("Up to Date"),
            Status::Ready { unknown: false } => t("A New Build Is Ready"),
            Status::Ready { unknown: true } => t("Unknown Build"),
            Status::Building => t("A New Build Is Being Made"),
            Status::Updating => t("Update LightCraft Is Opening"),
            Status::Failed(why) => t(why),
        }
    }

    /// Whether the Update button shows.
    pub fn can_update(&self) -> bool {
        matches!(self, Status::Ready { .. })
    }
}

/// One answer of GitHub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// Asks GitHub for a URL (a fake in tests). `Err` is a few words for the status line.
pub type Getter = Arc<dyn Fn(&str) -> Result<Reply, String> + Send + Sync>;

/// A short message for an answer that is not 200.
fn http_failure(status: u16) -> String {
    match status {
        403 | 429 => "GitHub Is Busy, Try Later".into(),
        404 => "No Build Is Published Yet".into(),
        s => format!("GitHub Said {s}"),
    }
}

/// Asks GitHub whether a newer build than `app` (the running app's commit) is ready. Two requests at most;
/// no request when the release is the app's own commit.
pub fn run_check(app: Option<&str>, get: &dyn Fn(&str) -> Result<Reply, String>) -> Status {
    let release_url = format!("https://api.github.com/repos/{REPO}/releases/tags/{RELEASE_TAG}");
    let reply = match get(&release_url) {
        Ok(r) => r,
        Err(e) => return Status::Failed(e),
    };
    if reply.status != 200 {
        return Status::Failed(http_failure(reply.status));
    }
    let Some(body) = serde_json::from_str::<Value>(&reply.body).ok().and_then(|v| v.get("body").and_then(Value::as_str).map(str::to_string)) else {
        return Status::Failed("GitHub Sent Something Unexpected".into());
    };
    let release = match parse_release_body(&body) {
        Release::Ready(sha) => sha,
        Release::Building => return Status::Building,
        Release::None => return Status::Failed("No Build Is Published Yet".into()),
    };
    let Some(app) = app.and_then(clean_sha) else {
        return Status::Ready { unknown: true };
    };
    if app == release {
        return Status::UpToDate;
    }
    let compare_url = format!("https://api.github.com/repos/{REPO}/compare/{app}...{release}?per_page=1");
    let reply = match get(&compare_url) {
        Ok(r) => r,
        Err(e) => return Status::Failed(e),
    };
    match reply.status {
        200 => match parse_compare(&reply.body) {
            Some(Compare::Ahead) => Status::Ready { unknown: false },
            Some(Compare::NotAhead) => Status::UpToDate,
            None => Status::Failed("GitHub Sent Something Unexpected".into()),
        },
        // GitHub does not know the app's commit (a build made on this Mac from work not pushed)
        404 | 422 => Status::Ready { unknown: true },
        s => Status::Failed(http_failure(s)),
    }
}

// ---------------------------------------------------------------------------------------------------------
// The network, the Info.plist and the updater app

#[cfg(not(test))]
fn real_getter() -> Getter {
    Arc::new(http_get)
}

/// A test never reaches the network: it installs its own [`Getter`].
#[cfg(test)]
fn real_getter() -> Getter {
    Arc::new(|_| Err("No network in tests".into()))
}

/// `GET url` over HTTPS with the model downloader's client: timeouts, no redirects, a capped body.
#[cfg(not(test))]
fn http_get(url: &str) -> Result<Reply, String> {
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use lightcraft_fetch::http::{Limits, Url, get};

    const UNREACHABLE: &str = "Can't Reach GitHub";
    let url = Url::parse(url).map_err(|_| UNREACHABLE.to_string())?;
    let cancel = AtomicBool::new(false);
    let limits = Limits { connect: Duration::from_secs(10), stall: Duration::from_secs(15), cancel: &cancel };
    let headers = [
        ("Accept", "application/vnd.github+json".to_string()),
        ("User-Agent", "LightCraft-fork-update".to_string()),
        ("X-GitHub-Api-Version", "2022-11-28".to_string()),
    ];
    let mut response = get(&url, &headers, &limits).map_err(|e| {
        log::warn!("update check: {e}");
        UNREACHABLE.to_string()
    })?;
    let mut body = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = response.read(&mut buf, &limits).map_err(|e| {
            log::warn!("update check: {e}");
            UNREACHABLE.to_string()
        })?;
        if n == 0 {
            break;
        }
        if let Some(chunk) = buf.get(..n) {
            body.extend_from_slice(chunk);
        }
        if body.len() > MAX_BODY {
            return Err("GitHub Sent Something Unexpected".into());
        }
    }
    Ok(Reply { status: response.status, body: String::from_utf8_lossy(&body).into_owned() })
}

/// The commit this app was built from: the `LightCraftBuildCommit` of the Info.plist two folders above the
/// executable (`LightCraft.app/Contents/MacOS/LightCraft`). `None` for a build that is not a packaged app.
pub fn app_commit() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let plist = exe.parent()?.parent()?.join("Info.plist");
    // a small text file; anything larger is not ours
    if std::fs::metadata(&plist).ok()?.len() > 256 * 1024 {
        return None;
    }
    parse_plist_commit(&std::fs::read_to_string(plist).ok()?)
}

/// Starts the "Update LightCraft" app. It outlives this app, which it quits to install and opens again.
pub type Launcher = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

#[cfg(not(test))]
fn real_launcher() -> Launcher {
    Arc::new(|| match std::process::Command::new("open").args(["-a", UPDATER_APP]).status() {
        Ok(s) if s.success() => Ok(()),
        _ => Err("Update LightCraft App Not Found".into()),
    })
}

/// A test never starts the updater.
#[cfg(test)]
fn real_launcher() -> Launcher {
    Arc::new(|| Err("No updater in tests".into()))
}

// ---------------------------------------------------------------------------------------------------------
// State, driven by `dialogs::updates_ui`

/// The update check's state (`updates_ui::Runtime::fork`).
pub struct State {
    pub status: Status,
    /// The window of the result (Help ▸ Check for Updates…, and a new build found at start).
    pub window_open: bool,
    /// A check was asked for and is not started yet.
    pending: bool,
    /// The app's commit is read once, at the first check (`Some(None)`: not known).
    commit: Option<Option<String>>,
    started: bool,
    in_flight: bool,
    pub getter: Getter,
    pub launcher: Launcher,
}

impl Default for State {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            window_open: false,
            pending: false,
            commit: None,
            started: false,
            in_flight: false,
            getter: real_getter(),
            launcher: real_launcher(),
        }
    }
}

impl State {
    /// Tests say which commit the app was built from.
    #[cfg(test)]
    pub fn set_app_commit(&mut self, commit: Option<&str>) {
        self.commit = Some(commit.map(str::to_string));
    }
}

/// Help ▸ Check for Updates…: opens the window and checks.
pub fn request_with_window(app: &mut LightcraftApp) {
    app.updates_rt.fork.window_open = true;
    request(app);
}

/// Settings' Check for Updates: checks and shows the result in Settings.
pub fn request(app: &mut LightcraftApp) {
    app.updates_rt.fork.pending = true;
}

/// Whether the desktop app checks for a new build when it starts: not when an agent drives it (a control port) or
/// when `LIGHTCRAFT_NO_UPDATE_CHECK` is set, so test copies of the app never pop up the update window.
pub fn check_at_start(control_port: bool, no_update_env: Option<&std::ffi::OsStr>) -> bool {
    !control_port && no_update_env.is_none_or(|v| v.is_empty())
}

/// Every frame: starts a check that was asked for, or the one at start (`at_start`, quiet).
pub fn tick(app: &mut LightcraftApp, at_start: bool) {
    let fork = &mut app.updates_rt.fork;
    if fork.in_flight {
        return;
    }
    if std::mem::take(&mut fork.pending) {
        start(app, false);
    } else if at_start && !fork.started {
        fork.started = true;
        start(app, true);
    }
}

/// Asks GitHub on a worker thread (a row in the activity stack meanwhile). `quiet`: the check at start, which
/// says something only when a build is ready.
fn start(app: &mut LightcraftApp, quiet: bool) {
    let fork = &mut app.updates_rt.fork;
    let commit = fork.commit.get_or_insert_with(app_commit).clone();
    let getter = fork.getter.clone();
    fork.status = Status::Checking;
    fork.in_flight = true;
    let work = move || run_check(commit.as_deref(), &*getter);
    let done = move |app: &mut LightcraftApp, _ctx: &egui::Context, status: Status| {
        let fork = &mut app.updates_rt.fork;
        fork.in_flight = false;
        fork.status = match status {
            // the check at start stays quiet when it finds nothing to offer
            Status::Failed(_) | Status::UpToDate | Status::Building if quiet => Status::Idle,
            s => s,
        };
        if quiet && fork.status.can_update() {
            fork.window_open = true;
        }
    };
    if let Err(e) = crate::tasks::spawn(app, LABEL, Some("update"), work, done) {
        log::warn!("update check: {e}");
        let fork = &mut app.updates_rt.fork;
        fork.in_flight = false;
        fork.status = if quiet { Status::Idle } else { Status::Failed("Can't Start the Check".into()) };
    }
}

/// The Update button: opens the "Update LightCraft" app, which installs the build and opens LightCraft again.
pub fn update(app: &mut LightcraftApp) {
    let fork = &mut app.updates_rt.fork;
    match (fork.launcher)() {
        Ok(()) => {
            fork.status = Status::Updating;
            fork.window_open = false;
        }
        Err(e) => fork.status = Status::Failed(e),
    }
}

// ---------------------------------------------------------------------------------------------------------
// Where it shows

/// Settings ▸ General: the Check for Updates button, the status in one line and, when a build is ready, Update.
pub fn settings_row(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &crate::theme::Tokens) {
    crate::dialogs::settings::row(ui, t, crate::i18n::tr("Updates"), |ui| {
        let busy = app.updates_rt.fork.status == Status::Checking;
        let r = ui.add_enabled(!busy, egui::Button::new(crate::i18n::tr("Check for Updates")));
        crate::widgets::register(ui, "button:settingsCheckForUpdates", r.rect);
        if r.clicked() {
            request(app);
        }
        let status = app.updates_rt.fork.status.clone();
        let text = status.text();
        if !text.is_empty() {
            let r = ui.label(egui::RichText::new(text).color(t.text_dim));
            crate::widgets::register(ui, "label:settingsUpdateStatus", r.rect);
        }
        if status.can_update() {
            let r = ui.button(crate::i18n::tr("Update"));
            crate::widgets::register(ui, "button:settingsUpdate", r.rect);
            if r.clicked() {
                update(app);
            }
        }
    });
}

/// The result window of Help ▸ Check for Updates… and of a build found at start.
pub fn window(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.updates_rt.fork.window_open {
        return;
    }
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut again = false;
    let mut install = false;
    let status = app.updates_rt.fork.status.clone();
    let shown = egui::Window::new(crate::i18n::tr("LightCraft Updates"))
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let r = ui.label(status.text());
            crate::widgets::register(ui, "label:updateStatus", r.rect);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if status.can_update() {
                    let r = ui.button(crate::i18n::tr("Update"));
                    crate::widgets::register(ui, "button:updateInstall", r.rect);
                    install |= r.clicked();
                }
                let r = ui.add_enabled(status != Status::Checking, egui::Button::new(crate::i18n::tr("Check Again")));
                crate::widgets::register(ui, "button:updateCheckAgain", r.rect);
                again |= r.clicked();
                let r = ui.button(crate::i18n::tr("Close"));
                crate::widgets::register(ui, "button:updateClose", r.rect);
                close |= r.clicked();
            });
        });
    if let Some(w) = &shown {
        crate::widgets::announce_dialog(ctx, &w.response, crate::widgets::Modal::Window);
    }
    if install {
        update(app);
    }
    if again {
        request(app);
    }
    if close {
        app.updates_rt.fork.window_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "d7ae6dfc978dd9d4ec963aae4a6c37cecd680806";
    const B: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn agent_driven_runs_skip_the_check_at_start() {
        use std::ffi::OsStr;
        assert!(check_at_start(false, None));
        assert!(check_at_start(false, Some(OsStr::new(""))));
        assert!(!check_at_start(true, None));
        assert!(!check_at_start(false, Some(OsStr::new("1"))));
    }

    #[test]
    fn release_body_names_a_commit() {
        assert_eq!(parse_release_body(&format!("commit: {A}")), Release::Ready(A.into()));
        assert_eq!(parse_release_body(&format!("Notes\r\ncommit: {}  \n", A.to_uppercase())), Release::Ready(A.into()));
    }

    #[test]
    fn release_body_while_a_build_uploads() {
        assert_eq!(parse_release_body("commit: updating"), Release::Building);
        assert_eq!(parse_release_body("commit: Updating\n"), Release::Building);
    }

    #[test]
    fn release_body_without_a_commit() {
        for body in ["", "commit: none", "commit: abc", "commit:", "commits: x", "no commit here", &format!("commit: {A}00")] {
            assert_eq!(parse_release_body(body), Release::None, "{body:?}");
        }
    }

    #[test]
    fn plist_commit_is_read() {
        let plist = format!(
            "<dict>\n  <key>LightCraftVersion</key>\n  <string>1.2</string>\n  <key>LightCraftBuildCommit</key>\n  <string>{A}</string>\n</dict>"
        );
        assert_eq!(parse_plist_commit(&plist), Some(A.into()));
    }

    #[test]
    fn plist_without_a_known_commit() {
        for plist in [
            "",
            "<key>LightCraftBuildCommit</key><string>unknown</string>",
            "<key>LightCraftBuildCommit</key><string></string>",
            "<key>LightCraftBuildCommit</key>",
            "<key>Other</key><string>d7ae6dfc978dd9d4ec963aae4a6c37cecd680806</string>",
        ] {
            assert_eq!(parse_plist_commit(plist), None, "{plist:?}");
        }
    }

    #[test]
    fn compare_status_ahead_is_newer() {
        assert_eq!(parse_compare(r#"{"status":"ahead","ahead_by":3}"#), Some(Compare::Ahead));
        for s in ["identical", "behind", "diverged"] {
            assert_eq!(parse_compare(&format!(r#"{{"status":"{s}"}}"#)), Some(Compare::NotAhead), "{s}");
        }
        for bad in ["", "{}", r#"{"status":7}"#, r#"{"status":"other"}"#, "not json"] {
            assert_eq!(parse_compare(bad), None, "{bad:?}");
        }
    }

    /// A fake GitHub: the release's notes and the compare answer.
    fn github(body: &str, compare: Option<(u16, &str)>) -> impl Fn(&str) -> Result<Reply, String> + use<> {
        let body = body.to_string();
        let compare = compare.map(|(s, b)| (s, b.to_string()));
        move |url: &str| {
            if url.contains("/releases/tags/custom-build") {
                Ok(Reply { status: 200, body: serde_json::json!({ "body": body }).to_string() })
            } else if url.contains("/compare/") {
                let (status, body) = compare.clone().ok_or_else(|| "compare was not expected".to_string())?;
                Ok(Reply { status, body })
            } else {
                Err(format!("unexpected url {url}"))
            }
        }
    }

    #[test]
    fn check_says_a_newer_build_is_ready_when_the_release_is_ahead() {
        let get = github(&format!("commit: {B}"), Some((200, r#"{"status":"ahead"}"#)));
        assert_eq!(run_check(Some(A), &get), Status::Ready { unknown: false });
    }

    #[test]
    fn check_says_up_to_date_for_the_same_commit_without_a_second_request() {
        // (no compare answer: asking for one would be an error)
        let get = github(&format!("commit: {A}"), None);
        assert_eq!(run_check(Some(A), &get), Status::UpToDate);
    }

    #[test]
    fn check_says_up_to_date_when_the_app_is_ahead_or_diverged() {
        for s in ["behind", "diverged", "identical"] {
            let json = format!(r#"{{"status":"{s}"}}"#);
            let get = github(&format!("commit: {B}"), Some((200, &json)));
            assert_eq!(run_check(Some(A), &get), Status::UpToDate, "{s}");
        }
    }

    #[test]
    fn check_says_a_build_is_being_made() {
        let get = github("commit: updating", None);
        assert_eq!(run_check(Some(A), &get), Status::Building);
        assert_eq!(run_check(None, &get), Status::Building);
    }

    #[test]
    fn an_unknown_build_can_still_update() {
        let get = github(&format!("commit: {B}"), None);
        assert_eq!(run_check(None, &get), Status::Ready { unknown: true });
        assert_eq!(run_check(Some("unknown"), &get), Status::Ready { unknown: true });
        // GitHub does not know the app's commit
        let get = github(&format!("commit: {B}"), Some((404, "{}")));
        assert_eq!(run_check(Some(A), &get), Status::Ready { unknown: true });
    }

    #[test]
    fn errors_are_short_messages() {
        let down = |_: &str| Err::<Reply, _>("Can't Reach GitHub".to_string());
        assert_eq!(run_check(Some(A), &down), Status::Failed("Can't Reach GitHub".into()));
        let busy = |_: &str| Ok(Reply { status: 403, body: "rate limited".into() });
        assert_eq!(run_check(Some(A), &busy), Status::Failed("GitHub Is Busy, Try Later".into()));
        let gone = |_: &str| Ok(Reply { status: 404, body: "{}".into() });
        assert_eq!(run_check(Some(A), &gone), Status::Failed("No Build Is Published Yet".into()));
        let junk = |_: &str| Ok(Reply { status: 200, body: "<html>".into() });
        assert_eq!(run_check(Some(A), &junk), Status::Failed("GitHub Sent Something Unexpected".into()));
        let none = github("commit: none", None);
        assert_eq!(run_check(Some(A), &none), Status::Failed("No Build Is Published Yet".into()));
        // the compare request failing after the release was read
        let flaky = |url: &str| {
            if url.contains("/compare/") {
                Ok(Reply { status: 500, body: String::new() })
            } else {
                Ok(Reply { status: 200, body: serde_json::json!({ "body": format!("commit: {B}") }).to_string() })
            }
        };
        assert_eq!(run_check(Some(A), &flaky), Status::Failed("GitHub Said 500".into()));
    }

    #[test]
    fn status_lines_are_title_case() {
        assert_eq!(Status::UpToDate.text(), "Up to Date");
        assert_eq!(Status::Ready { unknown: false }.text(), "A New Build Is Ready");
        assert_eq!(Status::Ready { unknown: true }.text(), "Unknown Build");
        assert_eq!(Status::Building.text(), "A New Build Is Being Made");
        assert_eq!(Status::Idle.text(), "");
        assert!(Status::Ready { unknown: true }.can_update());
        assert!(!Status::Building.can_update());
    }
}
