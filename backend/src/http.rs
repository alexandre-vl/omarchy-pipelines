//! A very small HTTP surface: one agent, one method, no redirect surprises.
//!
//! `http_status_as_error` is turned off deliberately. A `304` is the *success*
//! case for a conditional request, a `403` carries the rate-limit headers we
//! most need to read, and a `404` on repository validation is an answer rather
//! than a fault. Treating any of those as a transport error would throw away
//! the very information they exist to convey.

use std::time::Duration;

use ureq::Agent;

/// A response reduced to the four things any caller here cares about.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub etag: String,
    /// Raw `x-oauth-scopes`. Present and possibly empty for classic tokens,
    /// absent entirely for fine-grained ones — which is the only way to tell
    /// the two apart, so absence is tracked separately from emptiness.
    pub oauth_scopes: Option<String>,
    pub headers: RateHeaders,
}

/// The `x-ratelimit-*` family, plus `retry-after`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RateHeaders {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    pub reset: Option<i64>,
    pub retry_after: Option<u64>,
}

#[derive(Debug)]
pub struct Http {
    agent: Agent,
    user_agent: String,
}

impl Http {
    pub fn new(version: &str) -> Self {
        let config = Agent::config_builder()
            // A poll that has not answered in fifteen seconds has already
            // missed its slot; the next cycle will ask again.
            .timeout_global(Some(Duration::from_secs(15)))
            .timeout_connect(Some(Duration::from_secs(8)))
            .http_status_as_error(false)
            // GitHub redirects a renamed repository to its new location. One
            // hop is useful; a chain is a misconfiguration we should surface.
            .max_redirects(3)
            .build();

        Self {
            agent: Agent::new_with_config(config),
            user_agent: format!(
                "omarchy-pipelines/{version} (+https://github.com/jfg96/omarchy-pipelines)"
            ),
        }
    }

    /// Issue an authenticated `GET`.
    ///
    /// `etag` turns the request conditional. A body larger than 8 MiB is
    /// refused: the endpoints we call return a few dozen kilobytes, and an
    /// unbounded read is a memory bug waiting for a proxy to misbehave.
    pub fn get(&self, url: &str, token: &str, etag: Option<&str>) -> Result<Reply, String> {
        let mut request = self
            .agent
            .get(url)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", &self.user_agent);

        if !token.is_empty() {
            request = request.header("authorization", &format!("Bearer {token}"));
        }
        if let Some(etag) = etag.filter(|value| !value.is_empty()) {
            request = request.header("if-none-match", etag);
        }

        let mut response = request
            .call()
            .map_err(|error| crate::auth::redact(&describe(&error)))?;

        let status = response.status().as_u16();
        let headers = RateHeaders {
            limit: header_u32(&response, "x-ratelimit-limit"),
            remaining: header_u32(&response, "x-ratelimit-remaining"),
            reset: header_i64(&response, "x-ratelimit-reset"),
            retry_after: header_u64(&response, "retry-after"),
        };
        let etag = header_string(&response, "etag");
        let oauth_scopes = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        // A 304 has no body by definition; asking for one wastes a read.
        let body = if status == 304 {
            String::new()
        } else {
            response
                .body_mut()
                .with_config()
                .limit(8 * 1024 * 1024)
                .read_to_string()
                .map_err(|error| crate::auth::redact(&error.to_string()))?
        };

        Ok(Reply {
            status,
            body,
            etag,
            oauth_scopes,
            headers,
        })
    }
}

/// Turn a transport error into something a user could act on. ureq's own
/// `Display` leans technical, and the panel has one line to explain itself.
fn describe(error: &ureq::Error) -> String {
    let raw = error.to_string();
    let lowered = raw.to_lowercase();
    if lowered.contains("timeout") || lowered.contains("timed out") {
        "GitHub did not answer in time".into()
    } else if lowered.contains("dns") || lowered.contains("resolve") {
        "Could not resolve api.github.com — check the network".into()
    } else if lowered.contains("connect") || lowered.contains("connection") {
        "Could not reach api.github.com".into()
    } else if lowered.contains("tls") || lowered.contains("certificate") {
        "The TLS connection to GitHub could not be established".into()
    } else {
        raw
    }
}

fn header_string(response: &ureq::http::Response<ureq::Body>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn header_u32(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<u32> {
    header_string(response, name).parse().ok()
}

fn header_u64(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<u64> {
    header_string(response, name).parse().ok()
}

fn header_i64(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<i64> {
    header_string(response, name).parse().ok()
}
