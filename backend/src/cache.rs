//! Conditional-request cache.
//!
//! Every repository we poll keeps the `ETag` GitHub returned alongside the
//! parsed runs. The next poll sends `If-None-Match`; when nothing has changed
//! GitHub answers `304 Not Modified` with an empty body, which costs no quota
//! and no bandwidth, and we redraw from the stored copy.
//!
//! The cache is written to `$XDG_CACHE_HOME/omarchy/pipelines/state.json` so a
//! shell restart — which happens on every theme change and every plugin update
//! — repaints instantly from disk and revalidates for free, instead of
//! spending a request per repository to rediscover what it already knew.
//!
//! It holds no credential. Only public-ish run metadata the token could fetch
//! again, so a stale or hand-edited file is a correctness question, never a
//! security one.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::RunView;

/// Bumped when [`Entry`] changes shape. A file from an older helper is
/// discarded rather than migrated: it costs one conditional request per
/// repository to rebuild, and migration code for a throwaway cache is a
/// liability with no upside.
const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub runs: Vec<RunView>,
    /// Unix seconds of the last response, conditional or not.
    #[serde(default)]
    pub checked_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    version: u32,
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Default)]
pub struct Cache {
    entries: HashMap<String, Entry>,
    path: Option<PathBuf>,
    dirty: bool,
}

impl Cache {
    /// Load from disk, or start empty. A missing, unreadable, corrupt or
    /// version-mismatched file is not an error: the cache is an optimisation,
    /// and refusing to start because of one would be absurd.
    pub fn load(path: Option<PathBuf>) -> Self {
        let entries = path
            .as_deref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<Persisted>(&text).ok())
            .filter(|persisted| persisted.version == CACHE_VERSION)
            .map(|persisted| persisted.entries)
            .unwrap_or_default();

        Self {
            entries,
            path,
            dirty: false,
        }
    }

    pub fn get(&self, slug: &str) -> Option<&Entry> {
        self.entries.get(slug)
    }

    pub fn etag(&self, slug: &str) -> Option<&str> {
        self.entries
            .get(slug)
            .map(|entry| entry.etag.as_str())
            .filter(|etag| !etag.is_empty())
    }

    /// Store a fresh 200 response.
    pub fn store(&mut self, slug: &str, etag: String, runs: Vec<RunView>, now: i64) {
        self.entries.insert(
            slug.to_string(),
            Entry {
                etag,
                runs,
                checked_at: now,
            },
        );
        self.dirty = true;
    }

    /// Record that a 304 confirmed what we already hold.
    pub fn touch(&mut self, slug: &str, now: i64) {
        if let Some(entry) = self.entries.get_mut(slug) {
            entry.checked_at = now;
            self.dirty = true;
        }
    }

    /// Drop entries for repositories no longer being watched, so removing a
    /// project actually reclaims the space instead of leaking it forever.
    pub fn retain(&mut self, keep: &[String]) {
        let before = self.entries.len();
        self.entries
            .retain(|slug, _| keep.iter().any(|k| k == slug));
        if self.entries.len() != before {
            self.dirty = true;
        }
    }

    /// Write through a temporary file and rename, so a crash mid-write leaves
    /// the previous cache intact rather than a truncated one that parses to
    /// garbage.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = self.path.clone() else {
            self.dirty = false;
            return;
        };
        let Some(parent) = path.parent() else { return };
        if fs::create_dir_all(parent).is_err() {
            return;
        }

        let payload = Persisted {
            version: CACHE_VERSION,
            entries: self.entries.clone(),
        };
        let Ok(text) = serde_json::to_string(&payload) else {
            return;
        };

        let temp = path.with_extension("json.tmp");
        let written = fs::File::create(&temp).and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        });
        if written.is_ok() && fs::rename(&temp, &path).is_ok() {
            self.dirty = false;
        } else {
            let _ = fs::remove_file(&temp);
        }
    }

    /// Default location, honouring `XDG_CACHE_HOME`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))?;
        Some(base.join("omarchy").join("pipelines").join("state.json"))
    }
}

#[cfg(test)]
mod tests {
    // Tests assert; a failed assertion is the point, so the panic-free
    // lints that guard the poll loop are relaxed here and only here.
    #![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::protocol::Health;

    fn run(id: u64) -> RunView {
        RunView {
            id,
            workflow: "CI".into(),
            branch: "main".into(),
            event: "push".into(),
            status: "completed".into(),
            conclusion: "success".into(),
            health: Health::Passing,
            url: "https://example.invalid".into(),
            number: id,
            actor: "someone".into(),
            commit: "abcdef1".into(),
            message: "a change".into(),
            started_at: 100,
            updated_at: 200,
            duration: 100,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "omarchy-pipelines-test-{name}-{}",
            std::process::id()
        ));
        path.push("state.json");
        path
    }

    #[test]
    fn an_absent_file_yields_an_empty_cache() {
        let cache = Cache::load(Some(temp_path("absent")));
        assert!(cache.get("a/b").is_none());
        assert!(cache.etag("a/b").is_none());
    }

    #[test]
    fn a_corrupt_file_does_not_prevent_starting() {
        let path = temp_path("corrupt");
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, "{ not json at all");
        let cache = Cache::load(Some(path.clone()));
        assert!(cache.get("a/b").is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_stored_entry_round_trips_through_disk() {
        let path = temp_path("roundtrip");
        let mut cache = Cache::load(Some(path.clone()));
        cache.store("a/b", "\"tag-1\"".into(), vec![run(1)], 1_700_000_000);
        cache.flush();

        let reloaded = Cache::load(Some(path.clone()));
        assert_eq!(reloaded.etag("a/b"), Some("\"tag-1\""));
        assert_eq!(reloaded.get("a/b").map(|e| e.runs.len()), Some(1));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_version_bump_discards_the_old_file() {
        let path = temp_path("version");
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, r#"{"version":999,"entries":{"a/b":{"etag":"x"}}}"#);
        let cache = Cache::load(Some(path.clone()));
        assert!(
            cache.etag("a/b").is_none(),
            "a future cache must be ignored"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn an_empty_etag_is_treated_as_no_etag() {
        let mut cache = Cache::load(None);
        cache.store("a/b", String::new(), vec![run(1)], 10);
        assert!(cache.etag("a/b").is_none());
    }

    #[test]
    fn touch_updates_the_timestamp_without_losing_runs() {
        let mut cache = Cache::load(None);
        cache.store("a/b", "\"tag\"".into(), vec![run(1)], 10);
        cache.touch("a/b", 99);
        let entry = cache.get("a/b").expect("entry present");
        assert_eq!(entry.checked_at, 99);
        assert_eq!(entry.runs.len(), 1);
    }

    #[test]
    fn retain_drops_unwatched_repositories() {
        let mut cache = Cache::load(None);
        cache.store("a/b", "\"1\"".into(), vec![run(1)], 10);
        cache.store("c/d", "\"2\"".into(), vec![run(2)], 10);
        cache.retain(&["a/b".to_string()]);
        assert!(cache.get("a/b").is_some());
        assert!(cache.get("c/d").is_none());
    }
}
