# Security policy

## Reporting a vulnerability

Report privately through GitHub's
[security advisory form](https://github.com/alexandre-vl/omarchy-pipelines/security/advisories/new).
Please do not open a public issue for a vulnerability.

Expect an acknowledgement within a week. If a fix is warranted it ships as a
patch release, and the advisory is published once users have had a chance to
update.

## What this plugin handles

A GitHub access token with read access to repository metadata and Actions, plus
whatever workflow-run data that token can see. It makes outbound HTTPS requests
to `api.github.com` and to nothing else.

## How the token is handled

- **Stored** in the login keyring via `secret-tool` (libsecret). If the keyring
  is unavailable or locked, it falls back to a `0600` file under
  `$XDG_STATE_HOME/omarchy/pipelines/`, and the panel says so explicitly rather
  than letting you assume otherwise.
- **Passed to `secret-tool` on stdin**, never as a process argument, so it does
  not appear in `/proc` where any local process could read it.
- **Never sent back over IPC.** The shell can ask the helper to import a token
  from the `gh` CLI, but the token itself never crosses that boundary.
- **Never logged.** Every string leaving the helper on stderr or in an error
  field passes through a redactor covering GitHub's token prefixes and the
  legacy 40-character hex format.
- **`OMARCHY_PIPELINES_TOKEN`**, if set, takes precedence and is never persisted.

## Deliberate hardening

- Repository slugs are validated against GitHub's own character set before they
  can reach a URL, so a typed `../` cannot redirect a request.
- Query values are percent-encoded.
- Response bodies are read with an 8 MiB ceiling.
- `unsafe_code` is forbidden crate-wide; `unwrap`, `expect`, `panic` and
  slice indexing are denied by lint outside tests.
- TLS goes through `rustls` with its default verification. There is no option to
  disable certificate checking and there will not be one.

## Scope

In scope: token disclosure, request forgery through user-controlled input,
anything that lets a repository's contents affect this machine.

Out of scope: an attacker who already has your unlocked user session, and
Omarchy or Quickshell vulnerabilities — report those upstream.
