# Contributing

Thanks for looking. This is a small plugin with a narrow job, so the bar for
"yes" is mostly about whether a change keeps that job small and correct.

## Before you start

For anything bigger than a bug fix, open an issue first. It is cheaper for both
of us than a rejected pull request. Feature requests that add per-repository API
calls need a case for why the cost is worth it — see the rate-limit section
below.

## Getting set up

You need Omarchy 4.0 or newer, a Rust toolchain and node.

```sh
git clone https://github.com/jfg96/omarchy-pipelines.git
cd omarchy-pipelines
./build.sh
```

To run your working copy against a live shell, copy the folder into
`~/.config/omarchy/plugins/oma.pipelines` and restart the shell. It must be a
copy: the plugin loader refuses a folder containing symlinks, because one could
point back at an arbitrary file once the folder is in the trusted plugins
directory.

To type-check the QML without a full session, load it in a throwaway Quickshell
config with Omarchy's `Ui` and `Commons` on the import path. That catches
unknown properties and bad imports. It catches nothing about behaviour.

## Running the checks

```sh
node tests/model.test.js
node tests/manifest.test.js
shellcheck build.sh install.sh
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked
omarchy plugin validate .
```

CI runs all of it except the last, which needs an Omarchy machine.

## The lints are strict on purpose

`backend/Cargo.toml` denies `unwrap`, `expect`, `panic`, slice indexing and
integer division, and forbids `unsafe`. This is not fashion: a panic in the poll
thread takes the widget down with it and leaves the user staring at a bar that
never updates again.

If you genuinely need one of them, scope the allow to the smallest possible item
and give a `reason`. There are three in the codebase — the calendar arithmetic
and the budget maths — and each says why. Blanket-allowing at the crate root
will not be merged.

Tests relax the panic lints, because an assertion failing is the point.

## Things that will be asked about in review

- **Per-monitor state.** `Panel.qml` is built once per screen. If your change
  puts state there that should exist once, a user with two monitors gets two of
  it. This has caused real bugs: two helper processes, doubled API traffic, an
  IPC target registered twice.
- **Rate limiting.** Do not remove the reserve, the worst-case cost assumption
  or the conditional requests. If you change polling, say whether you tested it
  against the live API and what you observed.
- **Token handling.** Nothing may log a token, return one over IPC, or pass one
  as a process argument. `auth::redact` exists because the easiest way to leak a
  credential is a well-meaning error message that echoes its input.
- **Duplicated validation.** `Model.isValidSlug` and `split_slug` enforce the
  same rule in two languages on purpose, so the UI never accepts something the
  helper refuses. Change both together.
- **`Array.isArray` on anything from QML.** Use `Model.asList`. A list that has
  crossed a QML property boundary is a sequence-backed wrapper: it indexes and
  has a `length`, but `Array.isArray` says false, so the usual guard throws the
  data away without a word. It broke the overview captions, reordering, removal
  and mute simultaneously and none of it raised an error.
- **A second source of status colour.** Everything goes through
  `statusColor()`, fed by the theme palette. Two sources is how "running" ended
  up amber in the bar and blue in the panel.
- **New dependencies.** The helper has three. Adding a fourth needs a reason,
  and it must work on a stock Omarchy install.

## Commits and pull requests

Commit messages: a short imperative subject, then why rather than what. The
diff already says what.

Keep unrelated changes in separate commits. A formatting sweep buried in a bug
fix makes the fix unreviewable.

Fill in the pull request template honestly. An unticked verification box costs
nothing; a wrongly ticked one costs the next person their afternoon.

## Releases

Maintainer only. A stable release needs matching versions in `manifest.json`,
`backend/Cargo.toml` and `backend/Cargo.lock`, a matching `## X.Y.Z` heading in
`CHANGELOG.md`, and a `vX.Y.Z` tag on a commit that has already passed CI. The
release workflow verifies all of it and fails the tag rather than publishing a
mismatch. Release binaries come from the tagged workflow, never from a local
build.
