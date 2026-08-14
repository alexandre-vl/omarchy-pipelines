# Changelog

All notable changes to this plugin are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`manifest.json` and `backend/Cargo.toml` always carry the same version, and the
release workflow refuses a tag where they disagree or where the heading below is
missing.

## Unreleased

### Changed

- The bar icon is now a constant GitHub mark carrying a small status badge —
  green check, amber dot, red cross — the way a CI favicon works in a browser
  tab. Swapping the whole glyph between states meant the widget had no stable
  identity in the bar; recognition and status now travel on separate channels,
  and the slot never changes width. An empty watch list shows the bare mark.
  The failure count is gone from the bar with it; the tooltip and the panel
  carry the detail.
- Only the badge animates while a run is in progress. The mark holding still is
  what makes the motion legible.

### Fixed

- A run's subtitle elided as a single string, so a long branch name pushed the
  author and the duration off the end — and the duration is the shortest and
  most informative part of the line. Each part is now measured on its own:
  widths are assigned right to left, the duration is never cut, the author
  gives way next, and the branch absorbs what is left.

- Reopening the panel after closing it on a subpage replayed the navigation
  transition and came back on the subpage. The view now resets while the panel
  is closed, and the transition only plays for a move the user made while
  looking at it.
- Workflow names ran off the edge of the panel. The name elides and the run
  number is pinned to the right, where it is never truncated — a shortened
  `#1610…` is worse than no number at all.
- Hovering a workflow highlighted only its title line, because the title was a
  button and the branch/author line was loose beneath it. Each workflow is now
  one block with one hover surface and one hit area.
- The account name never appeared for a token adopted from storage when no
  projects were configured, which is the state a new install and a just-cleared
  list are both in: the poll short-circuits on having nothing to poll, and
  identity resolution was inside it. It is now a fact about the account rather
  than about the watch list.
- The "Add project" button looked live before the repository had been
  confirmed. It stays borderless and dimmed until validation succeeds.

### Added

- Keyboard access to the parts of the panel that previously needed a mouse:
  `,` opens settings, `r` refreshes, `Tab` focuses the repository or token
  field, and `Escape` hands focus back from a field.

### Fixed (earlier in this cycle)

- **The keyring was never used.** The check for whether `secret-tool` was
  available ran `secret-tool --version`, which is not a supported flag — it
  prints usage and exits 2. So the probe reported the keyring as unavailable on
  every machine, and every token silently went to the plaintext fallback file
  instead, on systems where the keyring worked perfectly. There is no proxy
  check any more: each operation attempts the real thing and treats only a
  genuine "not installed" spawn failure as unavailable.
- A token adopted from storage at startup left the account name unknown, which
  the panel rendered as "Connected as ?". The login is now resolved on the
  first poll that reaches the network — one request, once per session.

### Changed

- The bar label showed the failing and running counts side by side, which
  rendered as `✗ 2 1`: two bare numbers with nothing to say which was which,
  both in the failure colour. It now shows one number, counting the state the
  glyph is already showing, and omits it entirely when the count is one.
- The project list shows the owner as well as the repository, dimmed and
  inline, so two projects called `core` in different orgs are distinguishable.
  The detail screen carries the owner in its subtitle.
- Subpage navigation is explicit: a back arrow replaces the status glyph in the
  hero, the subtitle doubles as a breadcrumb, rows carry a chevron to show they
  lead somewhere, and the content slides in the direction you moved.
- Removed the API-budget readouts from the panel. The governor still does all
  of it; none of it is the user's problem to read.

## 0.1.0

First release.

### Added

- Aggregate bar indicator for GitHub Actions across any number of repositories.
  Silent while everything is green; shows failing and running counts otherwise,
  and breathes while a run is in progress.
- Panel with a project overview, a per-project workflow list, and one click
  through to the run on GitHub.
- Rate-limit governor that reserves a configurable share of the hourly quota
  (25% by default) and computes a poll interval that fits the remaining budget
  into the remaining window, assuming worst-case cost. Honours `Retry-After`,
  secondary rate limits, and backs off exponentially with jitter on transport
  failures.
- Conditional requests with stored `ETag`s, so an unchanged repository costs no
  quota. The cache is persisted to `$XDG_CACHE_HOME`, so a shell restart
  repaints instantly and revalidates for free.
- Adaptive cadence: faster while the panel is open, medium while a run is in
  progress, slow when idle.
- GitHub connection by importing the `gh` CLI's existing token, or by pasting a
  fine-grained token. Both are validated against the API before being stored.
- Token stored in the login keyring via `secret-tool`, handed over on stdin
  rather than as a process argument, with a `0600` file fallback that the panel
  reports explicitly.
- Repository field that validates as you type — shape and duplicates
  synchronously, existence and access against the API after a pause — and
  accepts a pasted GitHub URL as well as `owner/repository`.
- Drag-to-reorder for the project list, with `Alt+Up` / `Alt+Down` as a keyboard
  equivalent going through the same code path.
- Per-project mute, so a known-broken repository keeps being polled without
  colouring the bar.
- Desktop notifications on failure and, optionally, on recovery. Fired on
  transitions only, seeded from the on-disk cache so a restart does not
  re-announce a failure already seen.
- Refresh cadence, quota reserve and notification settings declared in the
  manifest schema and rendered by Omarchy's own bar-widget settings editor.
- IPC verbs `toggle`, `open`, `close`, `refresh` and `status` on the
  `oma.pipelines` target.

### Security

- Repository slugs validated against GitHub's character set before reaching a
  URL; query values percent-encoded; response bodies capped at 8 MiB.
- Tokens redacted from every error string and log line, and never returned over
  IPC.
- `unsafe_code` forbidden; `unwrap`, `expect`, `panic` and slice indexing denied
  by lint outside tests.
