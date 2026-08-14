//! The poll loop and the only place mutable state lives.
//!
//! Threading is deliberately boring. One thread reads stdin and does nothing
//! but parse and forward; one thread — this one — owns every piece of state
//! and is the sole writer of stdout. That means no lock protects the snapshot,
//! replies cannot interleave with events mid-line, and there is exactly one
//! order in which things happen. The only shared state is the rate-limit
//! governor, because the per-repository fetches do run in parallel.

use std::collections::HashMap;
use std::io::Write;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::auth::{self, Credential};
use crate::cache::Cache;
use crate::http::Http;
use crate::protocol::{
    AuthView, Envelope, Event, Health, RepoSpec, RepoView, Request, Settings, Snapshot, Summary,
    TokenSource, TokenStorage, PROTOCOL,
};
use crate::provider::github::GitHub;
use crate::provider::{Fetch, Provider, ProviderError};
use crate::ratelimit::Governor;

/// How many repositories are fetched at once. GitHub tolerates far more, but
/// a burst of concurrent requests from a desktop widget is exactly the shape
/// that trips the *secondary* rate limiter, which is much less forgiving than
/// the primary one and is measured in requests per minute rather than per hour.
const FETCH_CONCURRENCY: usize = 4;

/// Guard against a panel that spams refresh. `force` bypasses the cadence, not
/// this floor.
const MIN_FORCED_GAP: Duration = Duration::from_secs(3);

pub struct Engine {
    provider: Arc<GitHub>,
    governor: Arc<Mutex<Governor>>,
    cache: Cache,
    repos: Vec<RepoSpec>,
    settings: Settings,
    credential: Option<Credential>,
    auth: AuthView,
    /// Suggestion list, fetched once and filtered locally so typing in the
    /// picker costs nothing.
    suggestions: Vec<String>,
    open_panels: u32,
    generation: u64,
    /// Last health per `slug/workflow`, for transition detection.
    previous: HashMap<String, Health>,
    views: HashMap<String, RepoView>,
    last_poll: Option<Instant>,
    polling: bool,
    running: bool,
}

impl Engine {
    pub fn new(version: &str) -> Self {
        let governor = Arc::new(Mutex::new(Governor::new()));
        let provider = Arc::new(GitHub::new(Http::new(version), Arc::clone(&governor)));
        let cache = Cache::load(Cache::default_path());

        let mut engine = Self {
            provider,
            governor,
            cache,
            repos: Vec::new(),
            settings: Settings::default().sanitized(),
            credential: None,
            auth: AuthView::default(),
            suggestions: Vec::new(),
            open_panels: 0,
            generation: 0,
            previous: HashMap::new(),
            views: HashMap::new(),
            last_poll: None,
            polling: false,
            running: true,
        };
        engine.adopt_stored_credential();
        engine
    }

    /// Pick up a token from the keyring or environment without spending a
    /// request to validate it. The first poll will find out soon enough, and
    /// blocking startup on a network round trip makes the bar look broken on
    /// a laptop that woke up without Wi-Fi.
    fn adopt_stored_credential(&mut self) {
        if let Some(credential) = auth::resolve() {
            self.auth = AuthView {
                connected: true,
                login: String::new(),
                source: Some(credential.source),
                source_label: credential.source.label().to_string(),
                storage: credential.storage,
                ..AuthView::default()
            };
            self.credential = Some(credential);
        }
    }

    pub fn run(mut self, commands: &Receiver<Envelope>) {
        Self::emit(&Event::Ready {
            version: env!("CARGO_PKG_VERSION"),
            protocol: PROTOCOL,
        });
        // Paint whatever the cache already holds before any network happens.
        self.rebuild_views_from_cache();
        self.publish();

        while self.running {
            let wait = self.next_interval();
            match commands.recv_timeout(wait) {
                Ok(envelope) => self.handle(envelope),
                Err(RecvTimeoutError::Timeout) => self.poll_cycle(false),
                // stdin closed: the shell is gone, so are we.
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.cache.flush();
    }

    fn next_interval(&mut self) -> Duration {
        let cooldown = self
            .governor
            .lock()
            .map_or(Duration::ZERO, |governor| governor.cooldown());
        if !cooldown.is_zero() {
            return cooldown;
        }
        let any_running = self
            .views
            .values()
            .any(|view| view.health == Health::Running);
        let focused = self.open_panels > 0;
        let repos = self.repos.len();
        self.governor.lock().map_or_else(
            |_| Duration::from_secs(self.settings.idle_interval),
            |mut governor| governor.plan(repos, focused, any_running, &self.settings),
        )
    }

    fn handle(&mut self, envelope: Envelope) {
        let id = envelope.id;
        match envelope.request {
            Request::Hello { version } => {
                // The shell tells us the plugin version it was loaded from. A
                // mismatch means the folder was updated under a running shell
                // — `omarchy plugin update` does exactly that — so it is
                // reported rather than ignored, and the panel can offer a
                // restart instead of showing state from two different builds.
                let helper = env!("CARGO_PKG_VERSION");
                Self::reply_ok(
                    id,
                    serde_json::json!({
                        "protocol": PROTOCOL,
                        "helper": helper,
                        "provider": self.provider.id(),
                        "matches": version.is_empty() || version == helper,
                    }),
                );
                self.publish();
            }
            Request::AuthStatus => {
                let view = self.auth.clone();
                Self::reply_ok(id, serde_json::to_value(view).unwrap_or_default());
            }
            Request::AuthDetect => match auth::detect_gh() {
                // The token itself is never returned to the shell. The shell
                // asks us to import it, and we import it; a credential that
                // never crosses the IPC boundary cannot be logged by it.
                Ok(token) => match self.connect(&token, TokenSource::GhCli) {
                    Ok(login) => {
                        Self::reply_ok(id, serde_json::json!({ "login": login, "imported": true }));
                        self.poll_cycle(true);
                    }
                    Err(error) => Self::reply_err(id, &error),
                },
                Err(error) => Self::reply_err(id, &error),
            },
            Request::AuthSet { token, source } => match self.connect(&token, source) {
                Ok(login) => {
                    Self::reply_ok(id, serde_json::json!({ "login": login }));
                    self.poll_cycle(true);
                }
                Err(error) => Self::reply_err(id, &error),
            },
            Request::AuthClear => {
                let result = auth::clear();
                self.credential = None;
                self.auth = AuthView::default();
                self.suggestions.clear();
                match result {
                    Ok(()) => Self::reply_ok(id, serde_json::Value::Null),
                    Err(error) => Self::reply_err(id, &error),
                }
                self.publish();
            }
            Request::RepoValidate { slug } => self.validate_repo(id, &slug),
            Request::RepoSuggest { query, refresh } => self.suggest(id, &query, refresh),
            Request::Configure { repos, settings } => {
                self.configure(repos, settings);
                Self::reply_ok(id, serde_json::json!({ "repos": self.repos.len() }));
                self.poll_cycle(true);
            }
            Request::Refresh { force } => {
                Self::reply_ok(id, serde_json::Value::Null);
                self.poll_cycle(force);
            }
            Request::Focus { active } => {
                // Counted, not flagged: the bar builds one panel per monitor,
                // so "a panel is open" is a count of open panels. A flag would
                // let closing the popup on one screen slow the cadence while
                // it is still open on another.
                if active {
                    self.open_panels = self.open_panels.saturating_add(1);
                } else {
                    self.open_panels = self.open_panels.saturating_sub(1);
                }
                Self::reply_ok(id, serde_json::json!({ "open": self.open_panels }));
                if active && self.should_poll(false) {
                    self.poll_cycle(false);
                }
            }
            Request::Shutdown => {
                Self::reply_ok(id, serde_json::Value::Null);
                self.running = false;
            }
        }
    }

    /// Validate a token against the API before storing it, so the panel can
    /// say "that token is not valid" instead of silently failing to poll.
    fn connect(&mut self, token: &str, source: TokenSource) -> Result<String, String> {
        let token = token.trim();
        auth::plausible(token)?;

        let identity = self
            .provider
            .identify(token)
            .map_err(|error| error.message().to_string())?;

        let storage = auth::store(token, source).unwrap_or(TokenStorage::None);
        self.auth = AuthView {
            connected: true,
            login: identity.login.clone(),
            source: Some(source),
            source_label: source.label().to_string(),
            storage,
            scopes: identity.scopes,
            fine_grained: identity.fine_grained,
            error: String::new(),
        };
        self.credential = Some(Credential {
            token: token.to_string(),
            source,
            storage,
        });
        self.suggestions.clear();
        Ok(identity.login)
    }

    fn validate_repo(&mut self, id: u64, slug: &str) {
        let slug = slug.trim();
        let Some(token) = self.token() else {
            Self::reply_err(id, "Connect a GitHub account first");
            return;
        };
        match self.provider.project(&token, slug) {
            Ok(info) => Self::reply_ok(
                id,
                serde_json::json!({
                    "slug": info.slug,
                    "name": info.name,
                    "private": info.private,
                    "defaultBranch": info.default_branch,
                    "hasActions": info.has_actions,
                }),
            ),
            Err(error) => {
                if error.is_fatal() {
                    self.mark_disconnected(error.message());
                }
                Self::reply_err(id, error.message());
            }
        }
    }

    fn suggest(&mut self, id: u64, query: &str, refresh: bool) {
        let Some(token) = self.token() else {
            Self::reply_err(id, "Connect a GitHub account first");
            return;
        };
        if self.suggestions.is_empty() || refresh {
            match self.provider.projects(&token) {
                Ok(projects) => {
                    self.suggestions = projects.into_iter().map(|p| p.slug).collect();
                }
                Err(error) => {
                    if error.is_fatal() {
                        self.mark_disconnected(error.message());
                    }
                    Self::reply_err(id, error.message());
                    return;
                }
            }
        }

        let needle = query.trim().to_lowercase();
        let matches: Vec<&String> = self
            .suggestions
            .iter()
            .filter(|slug| needle.is_empty() || slug.to_lowercase().contains(&needle))
            .take(50)
            .collect();
        Self::reply_ok(id, serde_json::json!({ "repos": matches }));
    }

    fn configure(&mut self, repos: Vec<RepoSpec>, settings: Settings) {
        // Drop anything malformed rather than let it fail once per cycle
        // forever, and de-duplicate so the same project added twice does not
        // cost two requests an hour for one row.
        let mut seen: Vec<String> = Vec::new();
        let mut accepted: Vec<RepoSpec> = Vec::new();
        for spec in repos {
            if spec.parts().is_none() || seen.contains(&spec.slug) {
                continue;
            }
            seen.push(spec.slug.clone());
            accepted.push(spec);
        }

        self.repos = accepted;
        self.settings = settings.sanitized();
        self.cache.retain(&seen);
        self.views.retain(|slug, _| seen.contains(slug));
        self.previous
            .retain(|key, _| seen.iter().any(|slug| key.starts_with(slug.as_str())));
        self.rebuild_views_from_cache();
    }

    fn token(&self) -> Option<String> {
        self.credential
            .as_ref()
            .map(|credential| credential.token.clone())
    }

    fn mark_disconnected(&mut self, reason: &str) {
        self.credential = None;
        self.auth.connected = false;
        self.auth.error = reason.to_string();
    }

    fn should_poll(&self, force: bool) -> bool {
        if self.repos.is_empty() || self.credential.is_none() {
            return false;
        }
        if self
            .governor
            .lock()
            .is_ok_and(|governor| governor.is_throttled())
        {
            return false;
        }
        match (force, self.last_poll) {
            (true, Some(last)) => last.elapsed() >= MIN_FORCED_GAP,
            _ => true,
        }
    }

    fn poll_cycle(&mut self, force: bool) {
        if !self.should_poll(force) {
            self.publish();
            return;
        }
        let Some(token) = self.token() else {
            self.publish();
            return;
        };

        self.last_poll = Some(Instant::now());
        self.polling = true;
        self.publish();

        let now = unix_now();
        let mut fatal: Option<String> = None;

        // Snapshot the watch list: the loop below mutates `self` as results
        // land, and a `configure` arriving mid-cycle must not resize the list
        // we are iterating.
        let repos = self.repos.clone();
        for chunk in repos.chunks(FETCH_CONCURRENCY) {
            let provider = Arc::clone(&self.provider);
            let token = token.as_str();

            // Scoped threads let each worker borrow the spec and the cached
            // etag directly; nothing has to be cloned into an owned task.
            let results: Vec<(RepoSpec, Result<Fetch, ProviderError>)> =
                std::thread::scope(|scope| {
                    let handles: Vec<_> = chunk
                        .iter()
                        .map(|spec| {
                            let provider = Arc::clone(&provider);
                            let etag = self.cache.etag(&spec.slug).map(str::to_string);
                            scope.spawn(move || {
                                let outcome = provider.runs(token, spec, etag.as_deref());
                                (spec.clone(), outcome)
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .filter_map(|handle| handle.join().ok())
                        .collect()
                });

            for (spec, outcome) in results {
                match outcome {
                    Ok(Fetch::NotModified) => {
                        self.cache.touch(&spec.slug, now);
                        self.apply_cached(&spec, now, true);
                    }
                    Ok(Fetch::Fresh { etag, runs }) => {
                        self.cache.store(&spec.slug, etag, runs, now);
                        self.apply_cached(&spec, now, false);
                    }
                    Err(error) => {
                        if error.is_fatal() {
                            fatal = Some(error.message().to_string());
                        }
                        self.apply_error(&spec, error.message());
                    }
                }
            }

            if fatal.is_some() {
                break;
            }
        }

        if let Some(reason) = fatal {
            self.mark_disconnected(&reason);
        }

        self.cache.flush();
        self.polling = false;
        self.publish();
    }

    /// Rebuild one repository's view from whatever the cache now holds, and
    /// report any workflow that crossed into or out of a failing state.
    fn apply_cached(&mut self, spec: &RepoSpec, now: i64, from_cache: bool) {
        let Some(entry) = self.cache.get(&spec.slug) else {
            return;
        };
        let runs = entry.runs.clone();
        let health = runs
            .iter()
            .map(|run| run.health)
            .max()
            .unwrap_or(Health::Unknown);

        let mut transitions = Vec::new();
        for run in &runs {
            let key = format!("{}#{}", spec.slug, run.workflow);
            let before = self.previous.insert(key, run.health);
            // A first sighting is not a transition. Announcing every workflow
            // as "newly failing" the first time the widget starts would make
            // the notifications worthless.
            if let Some(before) = before {
                if before != run.health
                    && (run.health == Health::Failing || before == Health::Failing)
                {
                    transitions.push(Event::Transition {
                        slug: spec.slug.clone(),
                        label: spec.display().to_string(),
                        workflow: run.workflow.clone(),
                        from: before,
                        to: run.health,
                        url: run.url.clone(),
                    });
                }
            }
        }

        self.views.insert(
            spec.slug.clone(),
            RepoView {
                slug: spec.slug.clone(),
                label: spec.display().to_string(),
                health,
                muted: spec.muted,
                runs,
                error: String::new(),
                checked_at: now,
                from_cache,
            },
        );

        for transition in transitions {
            Self::emit(&transition);
        }
    }

    /// A failed poll keeps the previous runs on screen and marks them stale.
    /// Blanking the panel on a dropped Wi-Fi connection throws away the last
    /// thing we knew, which is still the best answer available.
    fn apply_error(&mut self, spec: &RepoSpec, message: &str) {
        let existing = self.views.get(&spec.slug);
        let runs = existing.map(|view| view.runs.clone()).unwrap_or_default();
        let checked_at = existing.map_or(0, |view| view.checked_at);
        self.views.insert(
            spec.slug.clone(),
            RepoView {
                slug: spec.slug.clone(),
                label: spec.display().to_string(),
                health: if runs.is_empty() {
                    Health::Unknown
                } else {
                    Health::Stale
                },
                muted: spec.muted,
                runs,
                error: message.to_string(),
                checked_at,
                from_cache: true,
            },
        );
    }

    fn rebuild_views_from_cache(&mut self) {
        for spec in self.repos.clone() {
            let Some(entry) = self.cache.get(&spec.slug) else {
                continue;
            };
            let runs = entry.runs.clone();
            let checked_at = entry.checked_at;
            let health = runs
                .iter()
                .map(|run| run.health)
                .max()
                .unwrap_or(Health::Unknown);
            // Seed the transition baseline from disk so a shell restart does
            // not re-announce a failure the user already saw.
            for run in &runs {
                self.previous
                    .insert(format!("{}#{}", spec.slug, run.workflow), run.health);
            }
            self.views.insert(
                spec.slug.clone(),
                RepoView {
                    slug: spec.slug.clone(),
                    label: spec.display().to_string(),
                    health,
                    muted: spec.muted,
                    runs,
                    error: String::new(),
                    checked_at,
                    from_cache: true,
                },
            );
        }
    }

    fn snapshot(&mut self) -> Snapshot {
        self.generation = self.generation.saturating_add(1);

        // Order follows the configured list, so drag-to-reorder in the panel
        // is the order shown here. A HashMap iteration order would shuffle the
        // list on every frame.
        let repos: Vec<RepoView> = self
            .repos
            .iter()
            .map(|spec| {
                self.views
                    .get(&spec.slug)
                    .cloned()
                    .unwrap_or_else(|| RepoView {
                        slug: spec.slug.clone(),
                        label: spec.display().to_string(),
                        health: Health::Unknown,
                        muted: spec.muted,
                        runs: Vec::new(),
                        error: String::new(),
                        checked_at: 0,
                        from_cache: false,
                    })
            })
            .collect();

        let mut summary = Summary::default();
        for view in repos.iter().filter(|view| !view.muted) {
            match view.health {
                Health::Failing => summary.failing += 1,
                Health::Running => summary.running += 1,
                Health::Passing => summary.passing += 1,
                Health::Stale => summary.stale += 1,
                Health::Unknown => summary.unknown += 1,
            }
        }
        summary.worst = summary.compute_worst();

        let budget = self
            .governor
            .lock()
            .map(|governor| governor.view(&self.settings))
            .unwrap_or_default();

        Snapshot {
            repos,
            summary,
            budget,
            auth: self.auth.clone(),
            generation: self.generation,
            polling: self.polling,
        }
    }

    fn publish(&mut self) {
        let snapshot = self.snapshot();
        Self::emit(&Event::State(Box::new(snapshot)));
    }

    fn reply_ok(id: u64, data: serde_json::Value) {
        Self::emit(&Event::Reply {
            id,
            ok: true,
            data,
            error: String::new(),
        });
    }

    fn reply_err(id: u64, error: &str) {
        Self::emit(&Event::Reply {
            id,
            ok: false,
            data: serde_json::Value::Null,
            error: auth::redact(error),
        });
    }

    /// One line, flushed. Quickshell reads `stdout` line by line, so a partial
    /// line left in the buffer is a message the shell never sees.
    fn emit(event: &Event) {
        let Ok(line) = serde_json::to_string(event) else {
            return;
        };
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        if writeln!(handle, "{line}").is_ok() {
            let _ = handle.flush();
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
