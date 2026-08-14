//! Token acquisition and storage.
//!
//! Three sources, in descending order of preference:
//!
//! 1. `OMARCHY_PIPELINES_TOKEN` in the environment — never written anywhere,
//!    for CI and for users who manage secrets themselves.
//! 2. The login keyring, through `secret-tool`. Omarchy ships libsecret and
//!    gnome-keyring, and `gh` itself stores its credential there, so this is
//!    the path almost everyone gets.
//! 3. A `0600` file under `$XDG_STATE_HOME`, only when `secret-tool` is absent
//!    or the keyring is locked. This is plaintext, so the panel says so out
//!    loud rather than letting the user assume otherwise.
//!
//! Nothing in this module ever writes a token to a log, an event, or an error
//! string. [`redact`] exists because the easiest way to leak a credential is a
//! well-meaning error message that echoes its input.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::protocol::{TokenSource, TokenStorage};

const KEYRING_SERVICE: &str = "omarchy-pipelines";
const KEYRING_ACCOUNT: &str = "github";
const ENV_VAR: &str = "OMARCHY_PIPELINES_TOKEN";

/// A token plus where it came from and where it lives.
#[derive(Debug, Clone)]
pub struct Credential {
    pub token: String,
    pub source: TokenSource,
    pub storage: TokenStorage,
}

/// Replace anything that looks like a credential with a fixed marker.
///
/// GitHub's documented prefixes (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`,
/// `github_pat_`) plus the legacy 40-character hex tokens. Applied to every
/// string that leaves this process on `stderr` or in an error field.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end();
        if looks_like_token(trimmed) {
            out.push_str("<redacted>");
            out.push_str(&word[trimmed.len()..]);
        } else {
            out.push_str(word);
        }
    }
    out
}

fn looks_like_token(word: &str) -> bool {
    const PREFIXES: [&str; 6] = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    if PREFIXES.iter().any(|prefix| word.starts_with(prefix)) {
        return true;
    }
    word.len() == 40 && word.chars().all(|c| c.is_ascii_hexdigit())
}

/// Reject obvious nonsense before spending a request on `/user`.
///
/// Deliberately permissive about the prefix: GitHub has added token formats
/// before and will again, and a helper that refuses a valid new credential
/// because it did not recognise the prefix is worse than one that lets the API
/// do the judging. Whitespace and control characters are refused outright —
/// those are paste accidents, and they would corrupt the HTTP header.
pub fn plausible(token: &str) -> Result<(), String> {
    if token.is_empty() {
        return Err("Token is empty".into());
    }
    if token.len() < 20 {
        return Err("Token is too short to be a GitHub credential".into());
    }
    if token.len() > 512 {
        return Err("Token is implausibly long".into());
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Token contains whitespace — check for a truncated paste".into());
    }
    if !token.is_ascii() {
        return Err("Token contains non-ASCII characters".into());
    }
    Ok(())
}

/// Find a token without touching the network.
pub fn resolve() -> Option<Credential> {
    if let Some(token) = std::env::var(ENV_VAR).ok().filter(|t| !t.trim().is_empty()) {
        return Some(Credential {
            token: token.trim().to_string(),
            source: TokenSource::Environment,
            storage: TokenStorage::Environment,
        });
    }
    if let Some((token, source)) = keyring_lookup() {
        return Some(Credential {
            token,
            source,
            storage: TokenStorage::Keyring,
        });
    }
    state_file_lookup().map(|(token, source)| Credential {
        token,
        source,
        storage: TokenStorage::StateFile,
    })
}

/// Ask the `gh` CLI for its token.
///
/// `gh` is very often a shim installed by a version manager (mise, asdf,
/// direnv), and those print their own banner — `mise ~/.config/mise/config.toml
/// tools: gh@2.97.0` — **on stdout**, ahead of the real output. Observed on the
/// development machine, not hypothetical. Trimming the whole stream therefore
/// yields a two-line "token" that would be sent straight into an
/// `Authorization` header.
///
/// So `stderr` is discarded *and* stdout is scanned line by line, taking the
/// last line that could actually be a credential. Scanning from the end means
/// a preamble of any length is skipped without having to recognise every
/// version manager's banner format.
pub fn detect_gh() -> Result<String, String> {
    let output = Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => "The GitHub CLI (gh) is not installed".to_string(),
            _ => format!("Could not run gh: {}", redact(&error.to_string())),
        })?;

    if !output.status.success() {
        return Err("The GitHub CLI has no token for github.com — run `gh auth login`".into());
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "gh returned a token that is not valid UTF-8".to_string())?;

    token_from_gh_output(&stdout)
}

/// Pick the credential out of whatever `gh` printed. Split out from the
/// process plumbing so the version-manager banner case is directly testable.
fn token_from_gh_output(stdout: &str) -> Result<String, String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .find(|line| plausible(line).is_ok())
        .map(str::to_string)
        .ok_or_else(|| {
            // Deliberately does not echo the output: if a token *was* in
            // there and merely failed our checks, quoting it here would put
            // it in the panel and the journal.
            "gh did not return a usable token — run `gh auth login`".to_string()
        })
}

/// Persist a token, preferring the keyring. Returns where it landed.
pub fn store(token: &str, source: TokenSource) -> Result<TokenStorage, String> {
    if keyring_store(token, source).is_ok() {
        // Best effort: a token in the keyring makes a leftover plaintext copy
        // pure liability.
        let _ = state_file_clear();
        return Ok(TokenStorage::Keyring);
    }
    state_file_store(token, source).map(|()| TokenStorage::StateFile)
}

/// Remove the token from everywhere we could have put it.
pub fn clear() -> Result<(), String> {
    let keyring = keyring_clear();
    let file = state_file_clear();
    if keyring.is_err() && file.is_err() {
        return Err("Could not remove the stored token".into());
    }
    Ok(())
}

/// There is deliberately no "is secret-tool available" probe.
///
/// There used to be one, and it ran `secret-tool --version`. That flag does not
/// exist: secret-tool prints its usage and exits 2. So the probe reported the
/// keyring as unavailable on every machine, and every token silently landed in
/// the plaintext fallback file instead — on a system where the keyring worked
/// perfectly and `gh` was already using it.
///
/// The lesson is that a proxy check can fail in a direction that quietly
/// degrades security. Each operation below now attempts the real thing and
/// treats only a genuine `NotFound` spawn failure as "not installed".
fn spawn_failure(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "secret-tool is not installed".to_string(),
        _ => format!("Could not run secret-tool: {}", redact(&error.to_string())),
    }
}

fn keyring_lookup() -> Option<(String, TokenSource)> {
    let output = Command::new("secret-tool")
        .args([
            "lookup",
            "service",
            KEYRING_SERVICE,
            "account",
            KEYRING_ACCOUNT,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stored = String::from_utf8(output.stdout).ok()?;
    let stored = stored.trim();
    if stored.is_empty() {
        return None;
    }
    // The source is stored as a prefix so the panel can tell the user which
    // credential it is about to revoke. An unprefixed value is a token written
    // by an older helper; treat it as a pasted PAT.
    Some(match stored.split_once(':') {
        Some(("gh-cli", token)) => (token.to_string(), TokenSource::GhCli),
        Some(("pat", token)) => (token.to_string(), TokenSource::Pat),
        _ => (stored.to_string(), TokenSource::Pat),
    })
}

fn keyring_store(token: &str, source: TokenSource) -> Result<(), String> {
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label=Omarchy Pipelines (GitHub)",
            "service",
            KEYRING_SERVICE,
            "account",
            KEYRING_ACCOUNT,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| spawn_failure(&error))?;

    let tag = match source {
        TokenSource::GhCli => "gh-cli",
        _ => "pat",
    };
    // secret-tool reads the secret from stdin, so the token never appears in
    // the process arguments where /proc would expose it to every local user.
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(format!("{tag}:{token}").as_bytes())
            .map_err(|_| "Could not hand the token to secret-tool".to_string())?;
    }
    drop(child.stdin.take());

    let status = child
        .wait()
        .map_err(|error| format!("secret-tool did not finish: {}", redact(&error.to_string())))?;
    if status.success() {
        Ok(())
    } else {
        Err("The keyring refused to store the token — is it unlocked?".into())
    }
}

fn keyring_clear() -> Result<(), String> {
    let status = Command::new("secret-tool")
        .args([
            "clear",
            "service",
            KEYRING_SERVICE,
            "account",
            KEYRING_ACCOUNT,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| spawn_failure(&error))?;
    if status.success() {
        Ok(())
    } else {
        Err("secret-tool could not clear the token".into())
    }
}

fn state_file_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))?;
    Some(base.join("omarchy").join("pipelines").join("token"))
}

fn state_file_lookup() -> Option<(String, TokenSource)> {
    let path = state_file_path()?;
    let contents = fs::read_to_string(path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(match trimmed.split_once(':') {
        Some(("gh-cli", token)) => (token.to_string(), TokenSource::GhCli),
        Some(("pat", token)) => (token.to_string(), TokenSource::Pat),
        _ => (trimmed.to_string(), TokenSource::Pat),
    })
}

fn state_file_store(token: &str, source: TokenSource) -> Result<(), String> {
    let path = state_file_path().ok_or("No writable state directory")?;
    let parent = path.parent().ok_or("Invalid state path")?;
    fs::create_dir_all(parent).map_err(|_| "Could not create the state directory".to_string())?;
    // Mode is set at creation rather than after writing: a token that exists
    // world-readable for even one scheduler tick has already leaked.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| "Could not open the token file".to_string())?;
    let tag = match source {
        TokenSource::GhCli => "gh-cli",
        _ => "pat",
    };
    file.write_all(format!("{tag}:{token}").as_bytes())
        .map_err(|_| "Could not write the token file".to_string())?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "Could not restrict the token file".to_string())?;
    Ok(())
}

fn state_file_clear() -> Result<(), String> {
    let path = state_file_path().ok_or("No state directory")?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove the token file".into()),
    }
}

#[cfg(test)]
mod tests {
    // Tests assert; a failed assertion is the point, so the panic-free
    // lints that guard the poll loop are relaxed here and only here.
    #![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn classic_tokens_are_redacted() {
        let leaked = redact("failed with ghp_abcdefghijklmnopqrstuvwxyz0123456789 here");
        assert!(!leaked.contains("ghp_"), "{leaked}");
        assert!(leaked.contains("<redacted>"));
    }

    #[test]
    fn fine_grained_tokens_are_redacted() {
        let leaked = redact("github_pat_11ABCDEFG0abcdefghijklmnop_zzzz failed");
        assert!(!leaked.contains("github_pat_"), "{leaked}");
    }

    #[test]
    fn legacy_hex_tokens_are_redacted() {
        let leaked = redact("token 0123456789abcdef0123456789abcdef01234567 rejected");
        assert!(leaked.contains("<redacted>"), "{leaked}");
    }

    #[test]
    fn redaction_preserves_surrounding_text_and_spacing() {
        let text = redact("before ghp_abcdefghijklmnopqrstuvwxyz0123456789 after");
        assert_eq!(text, "before <redacted> after");
    }

    #[test]
    fn ordinary_words_survive_redaction() {
        let text = "could not reach api.github.com after 3 attempts";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn empty_and_short_tokens_are_refused() {
        assert!(plausible("").is_err());
        assert!(plausible("ghp_short").is_err());
    }

    #[test]
    fn pasted_whitespace_is_refused_before_it_corrupts_a_header() {
        assert!(plausible("ghp_abcdefghijklmnopqrst uvwxyz").is_err());
        assert!(plausible("ghp_abcdefghijklmnopqrstuvwxyz\n").is_err());
    }

    #[test]
    fn a_plausible_token_passes() {
        assert!(plausible("ghp_abcdefghijklmnopqrstuvwxyz0123456789").is_ok());
        assert!(plausible("github_pat_11ABCDEFG0abcdefghijklmnop_zzzzzzzz").is_ok());
    }

    #[test]
    fn a_version_manager_banner_on_stdout_is_skipped() {
        // Exactly what `gh auth token` prints on a mise-managed install.
        let stdout = "mise ~/.config/mise/config.toml tools: gh@2.97.0\n\
                      ghp_abcdefghijklmnopqrstuvwxyz0123456789\n";
        assert_eq!(
            token_from_gh_output(stdout).as_deref(),
            Ok("ghp_abcdefghijklmnopqrstuvwxyz0123456789")
        );
    }

    #[test]
    fn a_bare_token_still_works() {
        assert_eq!(
            token_from_gh_output("ghp_abcdefghijklmnopqrstuvwxyz0123456789\n").as_deref(),
            Ok("ghp_abcdefghijklmnopqrstuvwxyz0123456789")
        );
    }

    #[test]
    fn trailing_noise_after_the_token_is_skipped() {
        let stdout = "ghp_abcdefghijklmnopqrstuvwxyz0123456789\nmise WARN missing: node@24\n";
        assert_eq!(
            token_from_gh_output(stdout).as_deref(),
            Ok("ghp_abcdefghijklmnopqrstuvwxyz0123456789")
        );
    }

    #[test]
    fn output_with_no_credential_is_an_error_that_leaks_nothing() {
        let error = token_from_gh_output("mise WARN missing: node@24\n\n")
            .expect_err("no token in this output");
        assert!(
            !error.contains("mise"),
            "the error must not echo the output"
        );
    }

    #[test]
    fn unknown_prefixes_are_allowed_through_to_the_api() {
        // GitHub has introduced new token formats before. Let /user decide.
        assert!(plausible("ghfuture_abcdefghijklmnopqrstuvwxyz").is_ok());
    }
}
