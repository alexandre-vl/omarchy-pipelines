//! The seam a second CI provider would slot into.
//!
//! v1 ships GitHub Actions only, but the engine talks to this trait rather
//! than to `github.rs` directly, so adding GitLab means adding a file and a
//! match arm — not unpicking provider assumptions from the scheduler, the
//! cache and the governor, which is the expensive version of this change.
//!
//! The trait is deliberately narrow. Everything a provider does is: prove a
//! credential works, confirm a project exists, list the projects a credential
//! can see, and fetch the current runs for one project.

use crate::protocol::{RepoSpec, RunView};

pub mod github;

/// What a credential turned out to be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub login: String,
    /// Empty for fine-grained tokens, which advertise no scopes.
    pub scopes: Vec<String>,
    pub fine_grained: bool,
}

/// What a project turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub slug: String,
    pub name: String,
    pub private: bool,
    /// False when the token can see the repository but not its Actions data,
    /// which is a distinct and much more confusing failure than a 404.
    pub has_actions: bool,
    pub default_branch: String,
}

/// The result of a conditional fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// GitHub confirmed our cached copy. Cost: nothing.
    NotModified,
    Fresh {
        etag: String,
        runs: Vec<RunView>,
    },
}

/// A provider-level failure, separated so the engine can react differently to
/// "your token is bad" than to "the network blinked".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// 401. The credential is gone or revoked; polling must stop.
    Unauthorized(String),
    /// 403/404 on a specific project. Other projects may still be fine.
    Forbidden(String),
    NotFound(String),
    /// Primary or secondary rate limit. Carries `Retry-After` when given.
    RateLimited {
        message: String,
        retry_after: Option<u64>,
    },
    /// Anything else: transport, 5xx, unparseable body.
    Transient(String),
}

impl ProviderError {
    pub fn message(&self) -> &str {
        match self {
            Self::Unauthorized(m)
            | Self::Forbidden(m)
            | Self::NotFound(m)
            | Self::Transient(m)
            | Self::RateLimited { message: m, .. } => m,
        }
    }

    /// True when retrying this project immediately is pointless.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Unauthorized(_))
    }
}

pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    /// Prove a token works and report who it belongs to.
    fn identify(&self, token: &str) -> Result<Identity, ProviderError>;

    /// Confirm one project exists and is readable.
    fn project(&self, token: &str, slug: &str) -> Result<ProjectInfo, ProviderError>;

    /// Projects this credential can see, most recently pushed first.
    fn projects(&self, token: &str) -> Result<Vec<ProjectInfo>, ProviderError>;

    /// Current runs for one project, conditional on `etag`.
    ///
    /// Implementations feed the shared [`crate::ratelimit::Governor`] they were
    /// constructed with as responses arrive, so `&self` is enough here and
    /// several projects can be fetched in parallel without the engine having
    /// to thread a mutable borrow through the fan-out.
    fn runs(
        &self,
        token: &str,
        spec: &RepoSpec,
        etag: Option<&str>,
    ) -> Result<Fetch, ProviderError>;
}
