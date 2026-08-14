## What this changes

<!-- One or two sentences. What behaviour is different afterwards? -->

## Why

<!-- The problem. If it fixes an issue, link it. -->

## How it was verified

<!-- Tick only what you actually ran. An unticked box is fine; a wrongly
     ticked one costs the next person their afternoon. -->

- [ ] `node tests/model.test.js`
- [ ] `node tests/manifest.test.js`
- [ ] `shellcheck build.sh install.sh`
- [ ] `cargo fmt --manifest-path backend/Cargo.toml --all -- --check`
- [ ] `cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings`
- [ ] `cargo test --manifest-path backend/Cargo.toml --locked`
- [ ] `omarchy plugin validate .` (Omarchy machine only)
- [ ] Loaded in a real Omarchy session and used the affected screen

<!-- If you changed polling, caching or auth, say whether you tested against
     the live GitHub API, and what you observed. "Should work" is not a
     verification. -->

## Risk

- [ ] Changes how often the GitHub API is called
- [ ] Changes how the token is obtained, stored or transmitted
- [ ] Changes what is written to `shell.json`
- [ ] Changes the helper/shell IPC protocol (`PROTOCOL` bumped?)
- [ ] None of the above

## Checklist

- [ ] Per-session state stayed in `Service.qml`, not `Panel.qml`
- [ ] Slug validation still matches between `Model.js` and `split_slug`
- [ ] No new runtime dependency, or it is justified above
- [ ] No token can reach a log, an error string or an IPC reply
- [ ] Version bumped in both `manifest.json` and `backend/Cargo.toml`, or the
      change cannot affect the installed plugin
