//! GitHub Actions.
//!
//! One request per project per cycle, conditional on the stored `ETag`, using
//! the repository-wide runs endpoint rather than one call per workflow. That
//! endpoint returns runs newest-first across every workflow, so a single
//! response is enough to derive the current state of all of them: group by
//! workflow and keep the newest entry in each group.
//!
//! Path segments are validated against GitHub's own character set before they
//! reach a URL, and query values are percent-encoded. A repository slug comes
//! from a text field the user typed into, and a slug containing `../` or a
//! stray `?` must not be able to redirect a request at a different endpoint.

use std::sync::Mutex;

use serde::Deserialize;

use crate::http::{Http, Reply};
use crate::protocol::{Health, RepoSpec, RunView};
use crate::ratelimit::Governor;
use crate::timefmt::parse_rfc3339;

use super::{Fetch, Identity, ProjectInfo, Provider, ProviderError};

const API: &str = "https://api.github.com";
/// Enough history to cover every workflow in a busy repository, without
/// dragging a payload nobody reads across the wire.
const RUNS_PER_PAGE: u32 = 40;
/// How many workflows we surface per repository once grouped.
const MAX_WORKFLOWS: usize = 12;

#[derive(Debug)]
pub struct GitHub {
    http: Http,
    governor: std::sync::Arc<Mutex<Governor>>,
}

impl GitHub {
    pub fn new(http: Http, governor: std::sync::Arc<Mutex<Governor>>) -> Self {
        Self { http, governor }
    }

    /// Feed the governor, then classify the status into a provider error.
    /// Every response goes through here, including the successful ones, so
    /// the rate-limit window is never updated from only half the traffic.
    fn account(&self, reply: &Reply, context: &str) -> Result<(), ProviderError> {
        if let Ok(mut governor) = self.governor.lock() {
            governor.observe(
                reply.status,
                reply.headers.limit,
                reply.headers.remaining,
                reply.headers.reset,
            );
        }

        match reply.status {
            200 | 304 => Ok(()),
            401 => Err(ProviderError::Unauthorized(
                "GitHub rejected the token — reconnect in settings".into(),
            )),
            // A 403 is either a permissions problem or the rate limiter. The
            // difference is in the headers: an exhausted primary limit reports
            // zero remaining, and a secondary limit sends Retry-After.
            403 | 429 => {
                let exhausted = reply.headers.remaining == Some(0);
                let retry_after = reply.headers.retry_after;
                if exhausted || retry_after.is_some() || reply.body.contains("secondary rate limit")
                {
                    if let Ok(mut governor) = self.governor.lock() {
                        governor.throttle(retry_after);
                    }
                    return Err(ProviderError::RateLimited {
                        message: "GitHub rate limit reached — backing off".into(),
                        retry_after,
                    });
                }
                Err(ProviderError::Forbidden(format!(
                    "Not permitted to read {context}"
                )))
            }
            404 => Err(ProviderError::NotFound(format!("{context} was not found"))),
            status @ 500..=599 => Err(ProviderError::Transient(format!(
                "GitHub returned {status} for {context}"
            ))),
            status => Err(ProviderError::Transient(format!(
                "Unexpected response {status} for {context}"
            ))),
        }
    }

    fn get(&self, url: &str, token: &str, etag: Option<&str>) -> Result<Reply, ProviderError> {
        self.http.get(url, token, etag).map_err(|error| {
            if let Ok(mut governor) = self.governor.lock() {
                governor.note_failure();
            }
            ProviderError::Transient(error)
        })
    }
}

impl Provider for GitHub {
    fn id(&self) -> &'static str {
        "github"
    }

    fn identify(&self, token: &str) -> Result<Identity, ProviderError> {
        let reply = self.get(&format!("{API}/user"), token, None)?;
        self.account(&reply, "the authenticated user")?;

        let user: ApiUser = serde_json::from_str(&reply.body)
            .map_err(|_| ProviderError::Transient("GitHub sent an unreadable profile".into()))?;

        // Classic tokens advertise their scopes in `x-oauth-scopes`.
        // Fine-grained tokens omit the header entirely, which is not a
        // failure — it is how you tell the two apart, and the panel says
        // "fine-grained" rather than "no permissions".
        let fine_grained = reply.oauth_scopes.is_none();
        let scopes = reply
            .oauth_scopes
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
            .collect();

        Ok(Identity {
            login: user.login,
            scopes,
            fine_grained,
        })
    }

    fn project(&self, token: &str, slug: &str) -> Result<ProjectInfo, ProviderError> {
        let (owner, name) = split_slug(slug)?;
        let reply = self.get(&format!("{API}/repos/{owner}/{name}"), token, None)?;
        self.account(&reply, slug)?;

        let repo: ApiRepo = serde_json::from_str(&reply.body)
            .map_err(|_| ProviderError::Transient("GitHub sent an unreadable repository".into()))?;
        Ok(repo.into_info())
    }

    fn projects(&self, token: &str) -> Result<Vec<ProjectInfo>, ProviderError> {
        let url = format!(
            "{API}/user/repos?per_page=100&sort=pushed&direction=desc\
             &affiliation=owner,collaborator,organization_member"
        );
        let reply = self.get(&url, token, None)?;
        self.account(&reply, "your repositories")?;

        let repos: Vec<ApiRepo> = serde_json::from_str(&reply.body).map_err(|_| {
            ProviderError::Transient("GitHub sent an unreadable repository list".into())
        })?;
        Ok(repos.into_iter().map(ApiRepo::into_info).collect())
    }

    fn runs(
        &self,
        token: &str,
        spec: &RepoSpec,
        etag: Option<&str>,
    ) -> Result<Fetch, ProviderError> {
        let (owner, name) = split_slug(&spec.slug)?;
        let mut url = format!(
            "{API}/repos/{owner}/{name}/actions/runs?per_page={RUNS_PER_PAGE}&exclude_pull_requests=true"
        );
        if !spec.branch.is_empty() {
            url.push_str("&branch=");
            url.push_str(&encode_query(&spec.branch));
        }

        let reply = self.get(&url, token, etag)?;
        self.account(&reply, &spec.slug)?;

        if reply.status == 304 {
            return Ok(Fetch::NotModified);
        }

        let payload: ApiRuns = serde_json::from_str(&reply.body)
            .map_err(|_| ProviderError::Transient("GitHub sent unreadable run data".into()))?;

        Ok(Fetch::Fresh {
            etag: reply.etag,
            runs: latest_per_workflow(payload.workflow_runs, &spec.workflow),
        })
    }
}

/// Validate and split `owner/name`.
///
/// GitHub allows alphanumerics, hyphen, underscore and dot in both halves and
/// nothing else. Enforcing that here means the slug can be interpolated into a
/// path without escaping, and that a typed `../../` cannot walk the API.
fn split_slug(slug: &str) -> Result<(&str, &str), ProviderError> {
    let (owner, name) = slug
        .split_once('/')
        .ok_or_else(|| ProviderError::NotFound(format!("{slug} is not owner/name")))?;
    if !is_safe_segment(owner) || !is_safe_segment(name) {
        return Err(ProviderError::NotFound(format!(
            "{slug} is not a valid repository name"
        )));
    }
    Ok((owner, name))
}

fn is_safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 100
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Percent-encode a query value. Branch names legitimately contain `/`, `#`
/// and other characters that would otherwise change the request.
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(hex_nibble(byte >> 4));
            out.push(hex_nibble(byte & 0x0f));
        }
    }
    out
}

/// `from_digit` cannot fail for a nibble, but returns an `Option` anyway; the
/// fallback keeps the poll loop panic-free without an `unwrap`.
fn hex_nibble(nibble: u8) -> char {
    char::from_digit(u32::from(nibble), 16)
        .unwrap_or('0')
        .to_ascii_uppercase()
}

/// Reduce a newest-first run list to the current state of each workflow.
///
/// GitHub returns every recent run, so a workflow that ran ten times today
/// appears ten times. Only the newest matters for "is this project green", and
/// the older entries would otherwise show a fixed failure forever.
fn latest_per_workflow(runs: Vec<ApiRun>, workflow_filter: &str) -> Vec<RunView> {
    let mut seen: Vec<u64> = Vec::new();
    let mut out: Vec<RunView> = Vec::new();

    for run in runs {
        if !workflow_filter.is_empty() && !path_matches(&run.path, workflow_filter) {
            continue;
        }
        if seen.contains(&run.workflow_id) {
            continue;
        }
        seen.push(run.workflow_id);
        out.push(run.into_view());
        if out.len() >= MAX_WORKFLOWS {
            break;
        }
    }
    out
}

/// Match `deploy.yml` against `.github/workflows/deploy.yml`, and also accept
/// the full path so either spelling works in the settings field.
fn path_matches(path: &str, filter: &str) -> bool {
    if path == filter {
        return true;
    }
    path.rsplit('/').next() == Some(filter)
}

/// Map a run's status and conclusion onto the one word the bar shows.
///
/// `cancelled`, `skipped` and `neutral` deliberately do not turn the indicator
/// red: a run someone stopped on purpose, or one that was skipped by a path
/// filter, is not a broken build, and treating it as one trains people to
/// ignore the light. `action_required` does count as failing — it is a build
/// waiting on a human, which is exactly what the indicator is for.
fn classify(status: &str, conclusion: &str) -> Health {
    if status != "completed" {
        return Health::Running;
    }
    match conclusion {
        "success" => Health::Passing,
        "failure" | "timed_out" | "startup_failure" | "action_required" => Health::Failing,
        _ => Health::Unknown,
    }
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    #[serde(default)]
    login: String,
}

#[derive(Debug, Deserialize)]
struct ApiRepo {
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    default_branch: String,
    #[serde(default)]
    permissions: Option<ApiPermissions>,
}

#[derive(Debug, Deserialize)]
struct ApiPermissions {
    #[serde(default)]
    pull: bool,
}

impl ApiRepo {
    fn into_info(self) -> ProjectInfo {
        // Absent permissions means the endpoint did not report them, which is
        // the case for public repositories read anonymously — assume readable
        // and let the runs call be the real test.
        let has_actions = self.permissions.as_ref().is_none_or(|p| p.pull);
        ProjectInfo {
            slug: self.full_name,
            name: self.name,
            private: self.private,
            has_actions,
            default_branch: self.default_branch,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiRuns {
    #[serde(default)]
    workflow_runs: Vec<ApiRun>,
}

#[derive(Debug, Deserialize)]
struct ApiRun {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    workflow_id: u64,
    #[serde(default)]
    run_number: u64,
    #[serde(default)]
    head_branch: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    run_started_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    actor: Option<ApiActor>,
    #[serde(default)]
    head_commit: Option<ApiCommit>,
}

#[derive(Debug, Deserialize)]
struct ApiActor {
    #[serde(default)]
    login: String,
}

#[derive(Debug, Deserialize)]
struct ApiCommit {
    #[serde(default)]
    message: String,
}

impl ApiRun {
    fn into_view(self) -> RunView {
        let conclusion = self.conclusion.unwrap_or_default();
        let health = classify(&self.status, &conclusion);
        let started_at = self
            .run_started_at
            .as_deref()
            .and_then(parse_rfc3339)
            .unwrap_or(0);
        let updated_at = self
            .updated_at
            .as_deref()
            .and_then(parse_rfc3339)
            .unwrap_or(0);
        // A run still in flight has no meaningful duration yet; -1 tells the
        // panel to show a live timer instead of a finished one.
        let duration = if health == Health::Running {
            -1
        } else if started_at > 0 && updated_at >= started_at {
            updated_at - started_at
        } else {
            -1
        };

        RunView {
            id: self.id,
            workflow: if self.name.is_empty() {
                self.path
                    .rsplit('/')
                    .next()
                    .unwrap_or("workflow")
                    .to_string()
            } else {
                self.name
            },
            branch: self.head_branch,
            event: self.event,
            status: self.status,
            conclusion,
            health,
            url: self.html_url,
            number: self.run_number,
            actor: self.actor.map(|a| a.login).unwrap_or_default(),
            commit: self.head_sha.chars().take(7).collect(),
            message: self
                .head_commit
                .map(|c| c.message.lines().next().unwrap_or_default().to_string())
                .unwrap_or_default(),
            started_at,
            updated_at,
            duration,
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests assert; a failed assertion is the point, so the panic-free
    // lints that guard the poll loop are relaxed here and only here.
    #![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn valid_slugs_split() {
        assert_eq!(
            split_slug("alexandre-vl/omarchy-pipelines").ok(),
            Some(("alexandre-vl", "omarchy-pipelines"))
        );
        assert_eq!(split_slug("a_b/c.d").ok(), Some(("a_b", "c.d")));
    }

    #[test]
    fn path_traversal_cannot_reach_the_url() {
        for slug in [
            "../../etc/passwd",
            "owner/../../admin",
            "owner/name?per_page=1",
            "owner/name#frag",
            "owner/na me",
            "owner//name",
            "/name",
            "owner/",
            "own er/name",
        ] {
            assert!(split_slug(slug).is_err(), "{slug} must be refused");
        }
    }

    #[test]
    fn dot_segments_are_refused() {
        assert!(!is_safe_segment(".."));
        assert!(!is_safe_segment("."));
        assert!(!is_safe_segment(""));
    }

    #[test]
    fn query_encoding_escapes_branch_separators() {
        assert_eq!(encode_query("feature/thing"), "feature%2Fthing");
        assert_eq!(encode_query("main"), "main");
        assert_eq!(encode_query("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(encode_query("release-1.2_x~y"), "release-1.2_x~y");
    }

    #[test]
    fn classification_matches_the_documented_policy() {
        assert_eq!(classify("in_progress", ""), Health::Running);
        assert_eq!(classify("queued", ""), Health::Running);
        assert_eq!(classify("completed", "success"), Health::Passing);
        assert_eq!(classify("completed", "failure"), Health::Failing);
        assert_eq!(classify("completed", "timed_out"), Health::Failing);
        assert_eq!(classify("completed", "startup_failure"), Health::Failing);
        assert_eq!(classify("completed", "action_required"), Health::Failing);
        // Deliberately neutral — see the doc comment on `classify`.
        assert_eq!(classify("completed", "cancelled"), Health::Unknown);
        assert_eq!(classify("completed", "skipped"), Health::Unknown);
        assert_eq!(classify("completed", "neutral"), Health::Unknown);
    }

    fn api_run(id: u64, workflow_id: u64, path: &str, conclusion: &str) -> ApiRun {
        ApiRun {
            id,
            name: format!("wf{workflow_id}"),
            path: path.to_string(),
            workflow_id,
            run_number: id,
            head_branch: "main".into(),
            event: "push".into(),
            status: "completed".into(),
            conclusion: Some(conclusion.to_string()),
            html_url: "https://example.invalid".into(),
            head_sha: "abcdef1234567890".into(),
            run_started_at: Some("2024-01-15T10:00:00Z".into()),
            updated_at: Some("2024-01-15T10:05:00Z".into()),
            actor: None,
            head_commit: None,
        }
    }

    #[test]
    fn only_the_newest_run_of_each_workflow_survives() {
        // Newest first, as GitHub returns them.
        let runs = vec![
            api_run(3, 1, ".github/workflows/ci.yml", "success"),
            api_run(2, 1, ".github/workflows/ci.yml", "failure"),
            api_run(1, 2, ".github/workflows/deploy.yml", "failure"),
        ];
        let views = latest_per_workflow(runs, "");
        assert_eq!(views.len(), 2, "one entry per workflow");
        assert_eq!(views.first().map(|r| r.id), Some(3));
        assert_eq!(
            views.first().map(|r| r.health),
            Some(Health::Passing),
            "the superseded failure must not linger"
        );
    }

    #[test]
    fn the_workflow_filter_accepts_a_bare_filename_or_a_full_path() {
        let runs = vec![
            api_run(3, 1, ".github/workflows/ci.yml", "success"),
            api_run(1, 2, ".github/workflows/deploy.yml", "failure"),
        ];
        let by_name = latest_per_workflow(runs, "deploy.yml");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name.first().map(|r| r.id), Some(1));

        let runs = vec![api_run(1, 2, ".github/workflows/deploy.yml", "failure")];
        let by_path = latest_per_workflow(runs, ".github/workflows/deploy.yml");
        assert_eq!(by_path.len(), 1);
    }

    #[test]
    fn workflow_output_is_capped() {
        let runs: Vec<ApiRun> = (0..50)
            .map(|i| api_run(i, i, ".github/workflows/ci.yml", "success"))
            .collect();
        assert_eq!(latest_per_workflow(runs, "").len(), MAX_WORKFLOWS);
    }

    #[test]
    fn a_running_run_reports_no_duration() {
        let mut run = api_run(1, 1, ".github/workflows/ci.yml", "");
        run.status = "in_progress".into();
        run.conclusion = None;
        let view = run.into_view();
        assert_eq!(view.health, Health::Running);
        assert_eq!(view.duration, -1);
    }

    #[test]
    fn a_finished_run_reports_its_wall_clock() {
        let view = api_run(1, 1, ".github/workflows/ci.yml", "success").into_view();
        assert_eq!(view.duration, 300, "10:00:00 to 10:05:00 is five minutes");
        assert_eq!(view.commit, "abcdef1", "sha is shortened to seven");
    }

    #[test]
    fn a_run_without_a_name_falls_back_to_its_filename() {
        let mut run = api_run(1, 1, ".github/workflows/release.yml", "success");
        run.name = String::new();
        assert_eq!(run.into_view().workflow, "release.yml");
    }

    #[test]
    fn only_the_first_line_of_a_commit_message_is_kept() {
        let mut run = api_run(1, 1, ".github/workflows/ci.yml", "success");
        run.head_commit = Some(ApiCommit {
            message: "fix: the thing\n\nA long body nobody wants in a bar tooltip.".into(),
        });
        assert_eq!(run.into_view().message, "fix: the thing");
    }

    #[test]
    fn an_empty_runs_payload_is_not_an_error() {
        let payload: ApiRuns = serde_json::from_str("{}").expect("tolerates missing key");
        assert!(payload.workflow_runs.is_empty());
    }

    #[test]
    fn unknown_fields_in_the_api_response_are_ignored() {
        let payload: ApiRuns = serde_json::from_str(
            r#"{"total_count":1,"workflow_runs":[{"id":5,"workflow_id":2,"status":"completed",
                "conclusion":"success","something_new":{"a":1}}]}"#,
        )
        .expect("forward compatible");
        assert_eq!(payload.workflow_runs.len(), 1);
    }
}
