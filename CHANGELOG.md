# Changelog

All notable changes to this plugin are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`manifest.json` and `backend/Cargo.toml` always carry the same version, and the
release workflow refuses a tag where they disagree or where the heading below is
missing.

## Unreleased

Nothing yet.

## 0.1.1

### Fixed

- Works under Omarchy 4. Omarchy 4 no longer hands a third-party plugin the
  shell itself, only a capability-scoped API with no `shellConfig` on it, so the
  plugin found no configuration at all: every configured repository disappeared
  from the panel while `shell.json` still listed them, and adding, removing,
  muting or reordering one did nothing. The configuration is now read from the
  API's copy of the bar, and written back through its own-entry update with
  every key this plugin does not own carried across unchanged. Nothing needs
  reconfiguring: the repositories already in `shell.json` come back as they
  were.
- A write the shell refuses is reported as a failure instead of as a save.

### Security

- The helper's TLS library, `rustls`, is updated from 0.23.43 to 0.23.45 for
  [GHSA-2mjx-qc3c-rqvc](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc):
  TLS 1.3 handshake messages were accepted across encryption-level boundaries.
  The handshake transcript stays authenticated, so this does not let an
  attacker alter or complete a connection to GitHub.

## 0.1.0

First release.

### The bar

- A constant GitHub mark carrying a small status badge, the way a CI favicon
  works in a browser tab: check when everything passed, dot while something
  runs, cross when something failed, muted dot when a status could not be
  refreshed, and a bare mark when nothing is configured. Recognition and status
  travel on separate channels, so the widget always looks like itself and the
  slot never changes width. Nothing about it animates.
- Status colours are read from the active theme's `colors.toml` and re-read on
  a theme change, so they are the same green, amber and red the rest of the
  desktop uses. One table feeds the bar badge and every row in the panel.
- Hovering gives the worst project, what happened to it, and how long ago.

### The panel

- Project overview showing owner and repository — the owner dimmed and inline,
  because two projects called `core` in different orgs are otherwise
  indistinguishable — captioned with the workflow the row is actually about and
  the branch it ran on, so a broken project names what broke without having to
  be opened. The age is the run's, not the last poll's.
- The caption names the workflow that justifies the row's state, not simply the
  newest one: a project can have `deploy` broken while `lint` passed a minute
  ago, and captioning a red row with a green workflow's name is worse than
  captioning it with nothing.
- Per-project workflow list: state, name, run number pinned right, and a
  branch · author · duration line. Each part is truncated on its own terms, so a
  long branch never pushes the duration off the end.
- A run in flight shows the time it has been going, counting up live, in the
  same slot a finished run shows its duration. The clock behind it ticks once a
  second only while a panel is open and something is actually running, and once
  a minute otherwise.
- Explicit subpage navigation: a back arrow where the status glyph sits, the
  subtitle doubling as a breadcrumb, and content that slides in the direction
  you moved. Closing on a subpage returns you to the top level with no replayed
  transition.
- Fully keyboard-driven: arrows move, `Enter` opens, `Escape` goes back, `,`
  opens settings, `r` refreshes, `Tab` reaches a text field.
- Repository field that validates as you type — shape and duplicates
  synchronously, then existence and access against the API — and accepts a
  pasted GitHub URL as well as `owner/repository`. The add button stays inert
  until the repository has actually been confirmed.
- Drag-to-reorder, with `J` / `K` on the selected row going through the same
  code path.
- Per-project mute, so a known-broken repository keeps being polled without
  colouring the bar.

### Polling and rate limits

- Conditional requests with stored `ETag`s, so an unchanged repository costs no
  quota. Verified against the live API: two consecutive conditional requests
  report the same `x-ratelimit-remaining`.
- A governor that reserves a configurable share of the hourly quota (25% by
  default) and computes a poll interval fitting the remaining budget into the
  remaining window at worst-case cost, so a thin budget stretches the cadence
  instead of failing at the end of the hour. Honours `Retry-After` and secondary
  rate limits, and backs off exponentially with jitter on transport failures.
- Adaptive cadence: faster while the panel is open, medium while a run is in
  progress, slow when idle.
- The cache is persisted to `$XDG_CACHE_HOME`, so a shell restart repaints
  instantly and revalidates for free.
- None of this is surfaced in the panel. Staying inside the budget is the
  plugin's job, not something the user should have to watch.

### Accounts

- Connect by importing the `gh` CLI's existing token, or by pasting a
  fine-grained one. Both are validated against the API before being stored.
  `gh` output is parsed line by line, because version managers such as mise
  print a banner on stdout ahead of the token.
- Stored in the login keyring via `secret-tool`, handed over on stdin rather
  than as a process argument. Falls back to a `0600` file only when the keyring
  is genuinely unavailable, and says so in the panel when it does.

### Notifications

- On failure, and optionally on recovery. Fired on transitions only, seeded from
  the on-disk cache so a restart does not re-announce a failure already seen.

### Integration

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
