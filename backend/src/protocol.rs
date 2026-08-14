//! The wire contract between the helper and `Service.qml`.
//!
//! One JSON object per line in each direction. Line-delimited rather than
//! length-prefixed because Quickshell's `Process` hands us `stdout` split on
//! newlines already, and a framing the shell cannot get wrong is worth more
//! than the bytes a tighter encoding would save.
//!
//! `PROTOCOL` is bumped whenever a field the shell reads changes shape. The
//! shell refuses a helper whose protocol it does not know, because a plugin
//! updated on disk while the shell is running is the normal case, not the
//! exception: `omarchy plugin update` rewrites the folder under a live shell.

use serde::{Deserialize, Serialize};

/// Incremented on any breaking change to the shapes below.
pub const PROTOCOL: u32 = 1;

/// Where a token came from. Shown in the panel so the user can tell an
/// imported `gh` credential from one they pasted, and so revoking the right
/// one is obvious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenSource {
    /// Imported from the `gh` CLI's own credential store.
    GhCli,
    /// Pasted by the user into the panel.
    Pat,
    /// Read from `OMARCHY_PIPELINES_TOKEN`; never persisted by us.
    Environment,
}

impl TokenSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::GhCli => "GitHub CLI",
            Self::Pat => "Personal access token",
            Self::Environment => "Environment",
        }
    }
}

/// Where the token is actually being kept, so the panel can warn when it fell
/// back to a plaintext file instead of the keyring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenStorage {
    /// libsecret via `secret-tool`; encrypted at rest with the login keyring.
    Keyring,
    /// A `0600` file under `$XDG_STATE_HOME`. Plaintext — surfaced in the UI.
    StateFile,
    /// Process environment. Nothing was written to disk.
    Environment,
    /// No token held.
    None,
}

/// A repository the user asked us to watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSpec {
    /// `owner/name`, as GitHub spells it.
    pub slug: String,
    /// Optional display override. Empty means "use the repo name".
    #[serde(default)]
    pub label: String,
    /// Restrict to one branch. Empty means every branch.
    #[serde(default)]
    pub branch: String,
    /// Restrict to one workflow file (`deploy.yml`). Empty means all of them.
    #[serde(default)]
    pub workflow: String,
    /// A muted repo is still polled but never drives the bar glyph, so a
    /// known-broken project cannot hold the indicator red forever.
    #[serde(default)]
    pub muted: bool,
}

impl RepoSpec {
    /// Split `owner/name`. Returns `None` for anything that is not exactly one
    /// owner and one name, both non-empty.
    pub fn parts(&self) -> Option<(&str, &str)> {
        let (owner, name) = self.slug.split_once('/')?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some((owner, name))
    }

    pub fn display(&self) -> &str {
        if !self.label.is_empty() {
            return &self.label;
        }
        self.parts().map_or(self.slug.as_str(), |(_, name)| name)
    }
}

/// Tuning the user can reach from the panel. Defaults are the values the
/// governor uses when the shell has not sent a `configure` yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Cadence while the panel is open, in seconds.
    #[serde(default = "Settings::default_focused")]
    pub focused_interval: u64,
    /// Cadence while a run is in progress and the panel is closed.
    #[serde(default = "Settings::default_active")]
    pub active_interval: u64,
    /// Cadence when everything is green and nobody is looking.
    #[serde(default = "Settings::default_idle")]
    pub idle_interval: u64,
    /// Fraction of the hourly quota, in percent, we refuse to spend. The token
    /// is usually shared with `gh` and editor tooling, so the helper is not
    /// entitled to all of it.
    #[serde(default = "Settings::default_reserve")]
    pub reserve_percent: u32,
    /// Desktop notification when a run changes to a failing conclusion.
    #[serde(default = "Settings::default_true")]
    pub notify_failures: bool,
    /// Desktop notification when a previously failing repo goes green again.
    #[serde(default)]
    pub notify_recoveries: bool,
}

impl Settings {
    const fn default_focused() -> u64 {
        15
    }
    const fn default_active() -> u64 {
        30
    }
    const fn default_idle() -> u64 {
        180
    }
    const fn default_reserve() -> u32 {
        25
    }
    const fn default_true() -> bool {
        true
    }

    /// Clamp anything the shell sends into a range the API will tolerate.
    /// A panel bug must not turn into a request storm against GitHub.
    pub fn sanitized(mut self) -> Self {
        self.focused_interval = self.focused_interval.clamp(10, 3600);
        self.active_interval = self.active_interval.clamp(15, 3600);
        self.idle_interval = self.idle_interval.clamp(30, 21600);
        self.reserve_percent = self.reserve_percent.min(90);
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            focused_interval: Self::default_focused(),
            active_interval: Self::default_active(),
            idle_interval: Self::default_idle(),
            reserve_percent: Self::default_reserve(),
            notify_failures: true,
            notify_recoveries: false,
        }
    }
}

/// A command from the shell. `id` correlates the [`Event::Reply`].
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    /// Handshake. The shell sends the version it expects to be talking to.
    Hello {
        #[serde(default)]
        version: String,
    },
    /// Report the current credential without touching the network.
    AuthStatus,
    /// Ask the `gh` CLI whether it has a token we could import.
    AuthDetect,
    /// Validate a token against `/user` and, if it is good, persist it.
    AuthSet { token: String, source: TokenSource },
    /// Forget the stored token.
    AuthClear,
    /// Check one `owner/name` exists and is readable. Drives the realtime
    /// tick in the add-repository field.
    RepoValidate { slug: String },
    /// Repos the token can see, newest push first, for the picker. Served
    /// from cache unless `refresh`.
    RepoSuggest {
        #[serde(default)]
        query: String,
        #[serde(default)]
        refresh: bool,
    },
    /// Replace the watch list and settings wholesale. The shell owns this
    /// state in `shell.json`; the helper never persists it.
    Configure {
        repos: Vec<RepoSpec>,
        #[serde(default)]
        settings: Settings,
    },
    /// Poll now. `force` bypasses the minimum-interval guard but never the
    /// rate-limit governor.
    Refresh {
        #[serde(default)]
        force: bool,
    },
    /// The panel opened or closed on some monitor; drives the cadence.
    Focus { active: bool },
    /// Flush caches and exit cleanly.
    Shutdown,
}

/// Where a repository stands, reduced to the one word the bar cares about.
/// Ordered worst-first so `max` over a set gives the glyph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Health {
    /// No runs, or nothing we could interpret.
    #[default]
    Unknown,
    /// Latest run of every workflow concluded successfully.
    Passing,
    /// Something is queued or in progress.
    Running,
    /// We have a status but it is out of date (poll failed, offline).
    Stale,
    /// At least one workflow's latest run failed or was cancelled.
    Failing,
}

/// One workflow run, flattened to what the panel draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: u64,
    pub workflow: String,
    pub branch: String,
    pub event: String,
    /// `queued`, `in_progress`, `completed`, …
    pub status: String,
    /// `success`, `failure`, `cancelled`, … Empty while still running.
    pub conclusion: String,
    pub health: Health,
    pub url: String,
    pub number: u64,
    pub actor: String,
    pub commit: String,
    pub message: String,
    /// Unix seconds. `0` when GitHub gave us nothing parseable.
    pub started_at: i64,
    pub updated_at: i64,
    /// Wall-clock seconds, or `-1` while the run is still going.
    pub duration: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoView {
    pub slug: String,
    pub label: String,
    pub health: Health,
    pub muted: bool,
    pub runs: Vec<RunView>,
    /// Populated when the last poll for this repo failed. The previous runs
    /// stay visible alongside it rather than being blanked out.
    pub error: String,
    /// Unix seconds of the last successful poll, `0` if never.
    pub checked_at: i64,
    /// True when the last poll cost nothing because GitHub answered 304.
    pub from_cache: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub failing: u32,
    pub running: u32,
    pub passing: u32,
    pub unknown: u32,
    pub stale: u32,
    /// The single state the bar glyph shows. Computed here rather than in QML
    /// so the bar and the panel can never disagree about what "worst" means.
    pub worst: Health,
}

impl Summary {
    /// Precedence: a broken build outranks a stale one, which outranks a
    /// running one. Green only when there is something green and nothing else.
    pub fn compute_worst(self) -> Health {
        if self.failing > 0 {
            Health::Failing
        } else if self.stale > 0 {
            Health::Stale
        } else if self.running > 0 {
            Health::Running
        } else if self.passing > 0 {
            Health::Passing
        } else {
            Health::Unknown
        }
    }
}

/// What the governor is doing, surfaced so the panel can show the user why it
/// is not refreshing faster instead of looking broken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetView {
    pub limit: u32,
    pub remaining: u32,
    /// Seconds until the hourly window resets.
    pub resets_in: i64,
    /// Requests we are willing to spend before the reset, after reserve.
    pub spendable: u32,
    /// The interval the governor settled on, in seconds.
    pub interval: u64,
    /// Requests issued this process lifetime.
    pub requests: u64,
    /// Of those, how many GitHub answered 304 — free, and the reason the
    /// idle cadence can be as tight as it is.
    pub conditional_hits: u64,
    /// Set while a secondary rate limit or `Retry-After` is being honoured.
    pub throttled_for: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthView {
    pub connected: bool,
    pub login: String,
    pub source: Option<TokenSource>,
    /// Human-readable form of `source`, so the panel does not carry a second
    /// copy of this mapping that can drift out of step with this one.
    pub source_label: String,
    pub storage: TokenStorage,
    /// Scopes GitHub reported. Empty for fine-grained tokens, which do not
    /// advertise scopes at all — not an error, and the UI says so.
    pub scopes: Vec<String>,
    /// True when the token is fine-grained (no `x-oauth-scopes` header).
    pub fine_grained: bool,
    pub error: String,
}

impl Default for AuthView {
    fn default() -> Self {
        Self {
            connected: false,
            login: String::new(),
            source: None,
            source_label: String::new(),
            storage: TokenStorage::None,
            scopes: Vec::new(),
            fine_grained: false,
            error: String::new(),
        }
    }
}

/// The whole picture, pushed whenever anything changes. The shell keeps no
/// incremental state of its own: one message is the truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub repos: Vec<RepoView>,
    pub summary: Summary,
    pub budget: BudgetView,
    pub auth: AuthView,
    /// Monotonic; lets the shell drop an out-of-order frame.
    pub generation: u64,
    /// True while a poll cycle is in flight, for the spinner.
    pub polling: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "ev", rename_all = "kebab-case")]
pub enum Event {
    /// First line the helper ever writes. The shell waits for it.
    Ready {
        version: &'static str,
        protocol: u32,
    },
    Reply {
        id: u64,
        ok: bool,
        #[serde(skip_serializing_if = "serde_json::Value::is_null")]
        data: serde_json::Value,
        #[serde(skip_serializing_if = "str::is_empty")]
        error: String,
    },
    State(Box<Snapshot>),
    /// A run crossed into a failing or recovered conclusion. Separate from
    /// `State` so the shell can notify without diffing snapshots itself.
    Transition {
        slug: String,
        label: String,
        workflow: String,
        from: Health,
        to: Health,
        url: String,
    },
}

#[cfg(test)]
mod tests {
    // Tests assert; a failed assertion is the point, so the panic-free
    // lints that guard the poll loop are relaxed here and only here.
    #![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn repo_spec_splits_owner_and_name() {
        let spec = RepoSpec {
            slug: "alexandre-vl/omarchy-pipelines".into(),
            ..blank()
        };
        assert_eq!(spec.parts(), Some(("alexandre-vl", "omarchy-pipelines")));
        assert_eq!(spec.display(), "omarchy-pipelines");
    }

    #[test]
    fn repo_spec_rejects_malformed_slugs() {
        for slug in ["", "owner", "/name", "owner/", "a/b/c"] {
            let spec = RepoSpec {
                slug: slug.into(),
                ..blank()
            };
            assert!(spec.parts().is_none(), "{slug} should not parse");
        }
    }

    #[test]
    fn label_overrides_repo_name() {
        let spec = RepoSpec {
            slug: "alexandre-vl/omarchy-pipelines".into(),
            label: "Pipelines".into(),
            ..blank()
        };
        assert_eq!(spec.display(), "Pipelines");
    }

    #[test]
    fn settings_clamp_out_of_range_values() {
        let wild = Settings {
            focused_interval: 0,
            active_interval: 1,
            idle_interval: 2,
            reserve_percent: 400,
            notify_failures: true,
            notify_recoveries: false,
        }
        .sanitized();
        assert_eq!(wild.focused_interval, 10);
        assert_eq!(wild.active_interval, 15);
        assert_eq!(wild.idle_interval, 30);
        assert_eq!(wild.reserve_percent, 90);
    }

    #[test]
    fn worst_health_wins_the_summary() {
        let summary = Summary {
            failing: 1,
            running: 3,
            passing: 9,
            ..Summary::default()
        };
        assert_eq!(summary.compute_worst(), Health::Failing);
        assert_eq!(
            Summary {
                running: 2,
                passing: 1,
                ..Summary::default()
            }
            .compute_worst(),
            Health::Running
        );
        assert_eq!(Summary::default().compute_worst(), Health::Unknown);
        // Stale outranks running: a status we could not refresh is worse news
        // than one we watched start.
        assert_eq!(
            Summary {
                stale: 1,
                running: 4,
                ..Summary::default()
            }
            .compute_worst(),
            Health::Stale
        );
    }

    #[test]
    fn requests_parse_from_the_shell_wire_format() {
        let envelope: Envelope =
            serde_json::from_str(r#"{"id":7,"cmd":"repo-validate","slug":"a/b"}"#)
                .expect("valid request");
        assert_eq!(envelope.id, 7);
        match envelope.request {
            Request::RepoValidate { slug } => assert_eq!(slug, "a/b"),
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn configure_defaults_settings_when_omitted() {
        let envelope: Envelope =
            serde_json::from_str(r#"{"id":1,"cmd":"configure","repos":[]}"#).expect("valid");
        match envelope.request {
            Request::Configure { settings, repos } => {
                assert!(repos.is_empty());
                assert_eq!(settings.idle_interval, Settings::default_idle());
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    fn blank() -> RepoSpec {
        RepoSpec {
            slug: String::new(),
            label: String::new(),
            branch: String::new(),
            workflow: String::new(),
            muted: false,
        }
    }
}
