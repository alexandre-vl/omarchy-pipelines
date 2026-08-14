# Repository guidance

These instructions apply to the entire repository and to every AI coding agent
working in it. Prioritise correctness, security and user-visible regressions
over stylistic preference. Keep patches minimal and never mix an unrelated
refactor into a bug fix.

## What this is

An Omarchy bar plugin that shows GitHub Actions status for a list of
repositories. Two halves, and the split is load-bearing:

- **`Service.qml`** is loaded once per shell session (`kind: service`). It owns
  the helper process, the snapshot, the `oma.pipelines` IPC target and every
  write to `shell.json`.
- **`Panel.qml`** is built **once per monitor** (`kind: bar-widget`). It is a
  view. Anything there can only be one of belongs in `Service.qml`.
- **`Model.js`** is pure functions, shared by both and tested under node. It
  takes clocks and palettes as arguments rather than reading them, which is the
  only reason it can be tested at all — keep it that way.
- **`backend/`** is the Rust helper: HTTP, auth, rate limiting, caching.

`Service.qml` also reads the active theme's `colors.toml` and exposes the
palette, because Quickshell's `Color` singleton has no green and no amber in it
— only foreground, background, accent, muted and urgent. Status colours are
resolved from that palette through the single `statusColor()` function in
`Panel.qml`. Do not introduce a second one: the bar badge and the panel rows
once had separate ideas of what "running" looked like, and the same state
showed up amber in one place and blue in the other.

If you put per-session state in `Panel.qml`, a user with two monitors gets two
of it. That has already been the cause of real bugs in sibling plugins: two
helper processes, doubled API traffic, and an IPC target registered twice with
only the first registration used.

## Rules that are not negotiable

- **Never weaken the rate limiter.** The governor in `backend/src/ratelimit.rs`
  exists so this widget cannot exhaust a token that the user's `gh`, editor and
  scripts also depend on. Do not remove the reserve, do not remove the
  worst-case cost assumption, do not poll on a fixed timer that ignores it.
- **Never drop conditional requests.** `If-None-Match` is why the idle cadence
  can be minutes instead of an hour. A 304 costs no quota. Verified against the
  live API: two consecutive conditional requests both reported the same
  `x-ratelimit-remaining`.
- **Never log a token.** Every string that leaves the helper on stderr or in an
  error field goes through `auth::redact`. The token is never returned to the
  shell over IPC, not even in a reply to `auth-detect`.
- **Never trust a repository slug.** It comes from a text field. It is validated
  against GitHub's character set in `split_slug` before it can reach a URL, and
  the same rule is duplicated in `Model.isValidSlug` so the UI and the helper
  agree. If you change one, change both — `tests/manifest.test.js` and
  `tests/model.test.js` will not catch every divergence for you.
- **Treat Quickshell `Process` lifecycle as separate cases.** A binary that is
  missing never emits `exited`; it only returns `running` to false without ever
  having emitted `started`. An exit code cannot carry that news, because nothing
  runs a shell on our behalf, so the `127` a shell would report never arrives.
  `Service.qml` handles this with a `launched` flag. Do not "simplify" it.
- **Never guard a list with `Array.isArray`.** Use `Model.asList`. A value that
  has crossed a QML property boundary comes back as a sequence-backed wrapper:
  it indexes, it has a `length`, and `Array.isArray` reports **false**. Guarding
  with `Array.isArray(x) ? x : []` therefore silently throws real data away.
  This was not hypothetical — it blanked every overview caption against rows
  that plainly had seven runs each, and the same guard sat in front of
  reordering, removal, mute and the settings round trip, all of which were
  quietly broken. `tests/model.test.js` builds the wrapper shape as a fixture;
  any new list-taking function needs a case there.
- **Do not probe a tool to decide whether to use it.** Attempt the real
  operation and treat only a genuine `NotFound` spawn failure as "not
  installed". The keyring was never once used on any machine because the
  availability check ran `secret-tool --version`, which is not a supported flag
  — it prints usage and exits 2. Every token silently went to the plaintext
  fallback instead. A proxy check that fails toward the less secure option is
  worse than no check at all.
- **Parse the stdout of shelled-out tools line by line.** `gh` is commonly a
  shim installed by a version manager, and mise prints its banner on **stdout**
  ahead of the real output, so `gh auth token` returns two lines. Trimming the
  whole stream yields a "token" with a banner glued to it.
- **Avoid new runtime dependencies.** The helper uses `ureq`, `serde` and
  `serde_json`, and shells out to `secret-tool` and `gh` rather than linking
  their libraries. Anything new must be justified and must be present on a
  stock Omarchy install.

## Settings ownership

Refresh cadence, quota reserve and notification preferences are declared in
`manifest.json` under `barWidget.schema`, and Omarchy's own bar-widget settings
editor renders and writes them. The repository list is edited in our panel
because a reorderable list of objects is not expressible in that schema.

Both write into the *same* `shell.json` entry. `Service.persist` therefore
updates only the keys this plugin owns and leaves everything else alone. Do not
change it to replace the entry wholesale.

## Validation

Run everything relevant to what you touched. Before proposing a merge, run the
full suite:

```sh
node tests/model.test.js
node tests/manifest.test.js
shellcheck build.sh install.sh
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked
```

On an Omarchy machine, also run `omarchy plugin validate .`.

A regression test must exercise the reported failure and must fail against the
pre-fix code for the expected reason. A test that passes before and after the
fix has tested nothing.

## What you may not claim to have tested

Do not state that Omarchy, Quickshell, the bar, real GitHub polling, desktop
notifications or the settings editor were exercised unless you actually ran
them. Those paths need a running Omarchy session.

`qmllint` is useless here — on a stock Arch install it exits 0 even for
`Item { nonExistentProperty: 5 }`, so a clean run from it means nothing. What
does work is loading the QML in a throwaway Quickshell config:

```sh
H=/tmp/pipecheck; rm -rf $H; mkdir -p $H/plugin
ln -s ~/.local/share/omarchy/shell/Ui $H/Ui
ln -s ~/.local/share/omarchy/shell/Commons $H/Commons
cp Service.qml Panel.qml Model.js manifest.json $H/plugin/
cat > $H/shell.qml <<'EOF'
import QtQuick
import Quickshell
ShellRoot {
  Component.onCompleted: t.start()
  Timer { id: t; interval: 2500; onTriggered: Qt.quit() }
  Loader { source: "plugin/Service.qml"; onStatusChanged: if (status === Loader.Error) console.warn("SERVICE LOAD ERROR") }
  Loader { source: "plugin/Panel.qml"; onStatusChanged: if (status === Loader.Error) console.warn("PANEL LOAD ERROR") }
}
EOF
qs -p $H          # NOT under QT_QPA_PLATFORM=offscreen
```

That resolves every `qs.Ui` and `qs.Commons` type and reports unknown
properties, bad imports and syntax errors. Run it under the real Wayland
session: offscreen has no layer-shell backend, so `KeyboardPanel` fails to
build and the whole panel body goes unchecked.

It catches nothing about behaviour. A binding that resolves can still evaluate
to an empty string — which is exactly how the blank captions got through.

Rate-limit and cache behaviour against the real API cannot be asserted from a
unit test. If you change either, say plainly whether you verified it live.

## Reviews and releases

- Flag correctness bugs, security regressions, unnecessary dependencies and
  unrelated scope growth. Do not request style-only refactors without a concrete
  correctness or maintenance benefit.
- Preserve contributor authorship. Keep release preparation separate from a
  contributor's functional commits.
- A stable release needs matching versions in `manifest.json`,
  `backend/Cargo.toml` and `backend/Cargo.lock`, a matching `## X.Y.Z` heading
  in `CHANGELOG.md`, and a `vX.Y.Z` tag. The release workflow enforces all of it
  and will fail the tag rather than publish a mismatch.
- After a stable release, the next runtime source change advances the manifest
  and helper to the next `-dev` version together. Documentation- or CI-only
  changes that cannot alter the installed plugin do not need a version bump.
- Never create or move a release tag until the exact target commit has passed
  CI. Release binaries come from the tagged workflow, never from a local build.
