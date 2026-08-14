# Pipelines

Track GitHub Actions across all your projects from the Omarchy bar.

One indicator that stays silent while everything is green, turns red the moment
a build breaks, and spends almost none of your GitHub API quota doing it.

```
all green:      ✓
running:        ↻  3      (breathing)
failing:        ✗  2      (urgent colour)
```

One glyph, and at most one number — which always counts the state the glyph is
showing. Silent when everything is green.

Click it for the project list, click a project for its workflows, click a
workflow to open the run on GitHub.

## Why it is cheap to run

A naive CI widget polls every repository on a timer and burns through the hourly
rate limit — which matters, because that limit is shared with your `gh` CLI,
your editor and your scripts. This one is built around two facts:

1. **A conditional request that matches costs nothing.** Every repository's
   `ETag` is stored, every poll sends `If-None-Match`, and GitHub answers an
   unchanged repository with a `304` that is **not** charged against the rate
   limit. Verified against the live API: two consecutive conditional requests
   both reported the same `x-ratelimit-remaining`.
2. **The quota is not ours alone.** A configurable share of the hourly limit
   (25% by default) is reserved and never spent. The poll interval is then
   computed to fit the *remaining* budget into the *remaining* window, assuming
   the worst case of one charged request per repository. If the budget is thin,
   the interval stretches automatically instead of failing at the end of the
   hour.

On top of that the cadence adapts: fast while the panel is open, medium while a
run is in progress, slow when everything is green and nobody is looking. The
cache is written to disk, so a shell restart repaints instantly and revalidates
for free rather than re-fetching everything.

None of this is surfaced in the panel. It is the plugin's job to stay
within its budget, not the user's job to watch it do so.

## Install

```sh
omarchy plugin add https://github.com/jfg96/omarchy-pipelines.git
```

Then add **Pipelines** to your bar from *Bar settings*, or run the installer,
which fetches a checksum-verified prebuilt helper from the latest release:

```sh
./install.sh
```

To build the helper from source instead (needs a Rust toolchain):

```sh
./build.sh
```

## Connect a GitHub account

Open the panel. If you already use the GitHub CLI, **Import from GitHub CLI** is
one click and needs nothing else.

Otherwise paste a [fine-grained token](https://github.com/settings/tokens?type=beta)
with read access to **Actions** and **Metadata** on the repositories you want to
watch. It is validated against the API before it is stored, so a bad token tells
you immediately instead of silently never refreshing.

The token goes into your login keyring through `secret-tool`. If the keyring is
locked or unavailable it falls back to a `0600` file, and the panel says so
rather than letting you assume otherwise. See [SECURITY.md](SECURITY.md).

## Add projects

Type `owner/repository` in settings, or paste a GitHub URL — `https://github.com/owner/repo/actions`
works fine. The field validates as you type: shape and duplicates instantly,
then existence and access against the API a moment later.

Drag the handle to reorder, or use `Alt+Up` / `Alt+Down`. The order in the list
is the order in the panel.

**Mute** a project to keep polling it without letting it colour the bar — useful
for the one repository with a known-broken nightly that would otherwise hold the
indicator red forever.

## Settings

Refresh cadence, quota reserve and notifications live in *Bar settings →
Pipelines*, because they are declared in the manifest and Omarchy renders them
natively. The project list is edited in the panel.

| Setting | Default | What it does |
| --- | --- | --- |
| Refresh while open | 15s | Cadence while the panel is open on any monitor |
| Refresh while running | 30s | Cadence when a workflow is queued or running |
| Refresh while idle | 180s | Cadence when everything is green and closed |
| API quota kept free | 25% | Share of the hourly limit never spent |
| Notify on failure | on | Desktop notification when a workflow starts failing |
| Notify on recovery | off | Desktop notification when it goes green again |

Notifications fire on *transitions* only, so a build that is already red stays
quiet, and a shell restart does not re-announce failures you have already seen.

## Status meanings

| | State | Meaning |
| --- | --- | --- |
| `●` | Failing | Latest run of some workflow failed, timed out, or needs a human |
| `◐` | Running | Something is queued or in progress |
| `⚠` | Stale | The last poll failed; the status shown is the last one known |
| `○` | Passing | Latest run of every workflow succeeded |
| `?` | Unknown | No runs, or nothing we could interpret |

Cancelled and skipped runs are deliberately **not** failures. A run someone
stopped on purpose, or one skipped by a path filter, is not a broken build, and
counting it as one teaches people to ignore the light.

## Command line

```sh
omarchy ipc call oma.pipelines toggle
omarchy ipc call oma.pipelines refresh
omarchy ipc call oma.pipelines status
```

## How it is put together

| File | Role |
| --- | --- |
| `Service.qml` | Loaded once per session. Owns the helper, the snapshot, IPC and all writes to `shell.json`. |
| `Panel.qml` | Built once per monitor. A view; owns only its own cursor and fields. |
| `Model.js` | Pure functions shared by both, tested under node. |
| `backend/` | Rust helper: HTTP, auth, rate limiting, caching. |

The helper speaks newline-delimited JSON on stdin/stdout. It holds no
configuration of its own — the shell owns that in `shell.json` and pushes it
down on every change.

Contributions: [CONTRIBUTING.md](CONTRIBUTING.md). Working with an AI agent in
this repository: [AGENTS.md](AGENTS.md).

## Licence

MIT. See [LICENSE](LICENSE).
