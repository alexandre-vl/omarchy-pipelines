// Pure-logic tests for Model.js. Run with `node tests/model.test.js`.
//
// These cover the parts of the UI that can be wrong without anyone noticing:
// slug validation drifting from the helper's, settings clamping, the bar
// staying silent when it should, and reordering arithmetic.

const assert = require("node:assert/strict")
const Model = require("../Model.js")

// ------------------------------------------------------------------ shell.json

const config = {
  bar: {
    layout: {
      left: [{ id: "omarchy.menu" }],
      right: [
        { id: "oma.pipelines", repos: [{ slug: "a/b" }], idleInterval: 300 },
        { id: "omarchy.clock" }
      ]
    }
  }
}

const entry = Model.barEntry(config, "oma.pipelines")
assert.equal(entry.id, "oma.pipelines")
assert.equal(entry.settings.idleInterval, 300)
assert.equal(Model.barEntry(config, "oma.missing"), null)
assert.equal(Model.barEntry(null, "oma.pipelines"), null)
assert.equal(Model.barEntry(config, ""), null)

// A plugin can also be listed outside the bar layout.
assert.ok(Model.barEntry({ plugins: [{ id: "oma.pipelines", repos: [] }] }, "oma.pipelines"))

// ------------------------------------------------------------------ repo list

assert.deepEqual(Model.reposIn({}), [])
assert.deepEqual(Model.reposIn({ repos: "nope" }), [])
assert.equal(Model.reposIn({ repos: [{ slug: "jfg96/omarchy-pipelines" }] }).length, 1)
// A bare string is accepted so hand-editing shell.json is practical.
assert.equal(Model.reposIn({ repos: ["jfg96/omarchy-pipelines"] })[0].slug, "jfg96/omarchy-pipelines")
// Malformed entries are dropped rather than failing once per poll forever.
assert.deepEqual(Model.reposIn({ repos: [{ slug: "no-slash" }, null, 7, { slug: "a/b" }] }).map(r => r.slug), ["a/b"])
// Duplicates would cost two requests an hour for one row.
assert.equal(Model.reposIn({ repos: [{ slug: "a/b" }, { slug: "a/b" }] }).length, 1)
assert.equal(Model.reposIn({ repos: [{ slug: "a/b", muted: true }] })[0].muted, true)

// ------------------------------------------------------------------- settings

const defaults = Model.settingsIn({})
assert.equal(defaults.idleInterval, 180)
assert.equal(defaults.notifyFailures, true)
assert.equal(defaults.notifyRecoveries, false)

// Clamps must match the helper's `Settings::sanitized`, or the UI shows a
// value the helper silently refuses.
assert.equal(Model.settingsIn({ focusedInterval: 0 }).focusedInterval, 10)
assert.equal(Model.settingsIn({ focusedInterval: 99999 }).focusedInterval, 3600)
assert.equal(Model.settingsIn({ idleInterval: 1 }).idleInterval, 30)
assert.equal(Model.settingsIn({ reservePercent: 400 }).reservePercent, 90)
assert.equal(Model.settingsIn({ reservePercent: -5 }).reservePercent, 0)
assert.equal(Model.settingsIn({ idleInterval: "junk" }).idleInterval, 180)

// ----------------------------------------------------------------- validation

for (const good of ["a/b", "jfg96/omarchy-pipelines", "org.name/repo.name", "a_b/c-d"]) {
  assert.ok(Model.isValidSlug(good), `${good} should be valid`)
}
for (const bad of ["", "a", "/b", "a/", "a/b/c", "a b/c", "../../etc", "a/b?x=1", "-lead/name"]) {
  assert.ok(!Model.isValidSlug(bad), `${bad} should be invalid`)
}

assert.equal(Model.slugVerdict("").state, "empty")
assert.equal(Model.slugVerdict("noslash").state, "invalid")
assert.equal(Model.slugVerdict("a b/c").state, "invalid")
assert.equal(Model.slugVerdict("a/b").state, "ready")
assert.equal(Model.slugVerdict("a/b", [{ slug: "a/b" }]).state, "duplicate")
// Case-insensitive, because GitHub is.
assert.equal(Model.slugVerdict("A/B", [{ slug: "a/b" }]).state, "duplicate")

// Pasting the address bar is what people actually do.
assert.equal(Model.slugFromInput("https://github.com/jfg96/omarchy-pipelines"), "jfg96/omarchy-pipelines")
assert.equal(Model.slugFromInput("https://github.com/jfg96/omarchy-pipelines.git"), "jfg96/omarchy-pipelines")
assert.equal(Model.slugFromInput("github.com/jfg96/omarchy-pipelines/actions"), "jfg96/omarchy-pipelines")
assert.equal(Model.slugFromInput("https://github.com/a/b?tab=readme"), "a/b")
assert.equal(Model.slugFromInput("  jfg96/omarchy-pipelines  "), "jfg96/omarchy-pipelines")
assert.equal(Model.slugFromInput(""), "")

// ------------------------------------------------------------------- protocol

assert.equal(Model.parseLine("not json"), null)
assert.equal(Model.parseLine("null"), null)
assert.equal(Model.parseLine("7"), null)
assert.deepEqual(Model.parseLine('{"ev":"ready"}'), { ev: "ready" })

assert.ok(Model.protocolAccepted({ ev: "ready", protocol: 1 }, 1))
assert.ok(!Model.protocolAccepted({ ev: "ready", protocol: 2 }, 1))
assert.ok(!Model.protocolAccepted({ ev: "state" }, 1))
assert.ok(!Model.protocolAccepted(null, 1))

// --------------------------------------------------------------- presentation

assert.equal(Model.worstOf({ failing: 0, stale: 2, running: 1 }), "stale")
assert.equal(Model.worstOf({}), "unknown")
assert.equal(Model.worstOf({ worst: "running", failing: 9 }), "running",
  "the precomputed field wins over derivation")

// --------------------------------------------------------------- org display

assert.equal(Model.ownerPrefix("home-assistant/core"), "home-assistant/")
assert.equal(Model.repoName("home-assistant/core"), "core")
assert.equal(Model.ownerPrefix("nonsense"), "", "no owner to show for a bare name")
assert.equal(Model.repoName("nonsense"), "nonsense")
assert.equal(Model.ownerPrefix("owner/"), "")
assert.equal(Model.ownerPrefix("/name"), "")

// A custom label replaces the repository name but never the owner: knowing a
// project is called "Deploy" is no help if two orgs both have one.
assert.equal(Model.rowTitle({ slug: "a/b", label: "" }), "b")
assert.equal(Model.rowTitle({ slug: "a/b", label: "Deploy" }), "Deploy")
assert.equal(Model.rowTitle(null), "")


assert.ok(Model.glyphFor("passing") !== Model.glyphFor("failing"))

const now = 1_700_000_000
assert.equal(Model.relativeTime(0, now), "never")
assert.equal(Model.relativeTime(now - 5, now), "just now")
assert.equal(Model.relativeTime(now - 240, now), "4m ago")
assert.equal(Model.relativeTime(now - 7200, now), "2h ago")
assert.equal(Model.relativeTime(now - 172800, now), "2d ago")
assert.equal(Model.relativeTime(now - 5184000, now), "2mo ago")
// A clock that jumped backwards must not print a negative age.
assert.equal(Model.relativeTime(now + 500, now), "just now")

assert.equal(Model.formatDuration(-1), "")
assert.equal(Model.formatDuration(45), "45s")
assert.equal(Model.formatDuration(120), "2m")
assert.equal(Model.formatDuration(160), "2m 40s")
assert.equal(Model.formatDuration(7200), "2h 0m")

const snapshot = {
  auth: { connected: true },
  repos: [
    { slug: "a/ok", label: "ok", health: "passing", checkedAt: now - 60, runs: [{ workflow: "CI" }] },
    { slug: "a/bad", label: "bad", health: "failing", checkedAt: now - 120, runs: [{ workflow: "Deploy" }] }
  ]
}
// The tooltip must surface the worst repo, not the first one.
assert.ok(Model.tooltipFor(snapshot, now).startsWith("bad · Deploy failed"))
assert.equal(Model.tooltipFor({ repos: [] }, now), "No repositories yet — open to add one")
assert.equal(Model.tooltipFor({ auth: { connected: false }, repos: [{}] }, now), "Not connected to GitHub")
// A muted repo must not be able to hold the tooltip red.
assert.equal(
  Model.tooltipFor({ auth: { connected: true }, repos: [{ muted: true, health: "failing" }] }, now),
  "1 repositories muted"
)

// Parts stay separate so each can be truncated on its own terms; joined, a
// long branch pushed the duration off the end entirely.
const parts = Model.runParts({ branch: "main", actor: "alex", duration: 90 })
assert.deepEqual(parts, { branch: "main", actor: "alex", duration: "1m 30s" })
assert.deepEqual(Model.runParts({ branch: "main", duration: -1 }),
  { branch: "main", actor: "", duration: "" })
assert.deepEqual(Model.runParts(null), { branch: "", actor: "", duration: "" })

// -------------------------------------------------------------- list editing

const list = [{ slug: "a/1" }, { slug: "a/2" }, { slug: "a/3" }]
assert.deepEqual(Model.moveItem(list, 0, 2).map(r => r.slug), ["a/2", "a/3", "a/1"])
assert.deepEqual(Model.moveItem(list, 2, 0).map(r => r.slug), ["a/3", "a/1", "a/2"])
assert.deepEqual(Model.moveItem(list, 1, 1).map(r => r.slug), ["a/1", "a/2", "a/3"])
// Out of range must not throw or corrupt the list.
assert.deepEqual(Model.moveItem(list, 9, 0).map(r => r.slug), ["a/1", "a/2", "a/3"])
assert.deepEqual(Model.moveItem(list, 0, 99).map(r => r.slug), ["a/2", "a/3", "a/1"])
// The input array is never mutated in place.
assert.deepEqual(list.map(r => r.slug), ["a/1", "a/2", "a/3"])

assert.deepEqual(Model.removeAt(list, 1).map(r => r.slug), ["a/1", "a/3"])
assert.deepEqual(Model.removeAt(list, 9).map(r => r.slug), ["a/1", "a/2", "a/3"])

assert.equal(Model.addRepo([], "a/b").length, 1)
assert.equal(Model.addRepo([], "bad").length, 0)
assert.equal(Model.addRepo([{ slug: "a/b" }], "A/B").length, 1, "duplicate add is a no-op")

assert.equal(Model.setFieldAt(list, 1, "muted", true)[1].muted, true)
assert.equal(Model.setFieldAt(list, 1, "muted", true)[0].muted, undefined)
assert.equal(list[1].muted, undefined, "setFieldAt must not mutate its input")

// Drag arithmetic: offset in pixels, converted to a row index.
assert.equal(Model.dropIndex(0, 0, 40, 3), 0)
assert.equal(Model.dropIndex(0, 45, 40, 3), 1)
assert.equal(Model.dropIndex(0, 400, 40, 3), 2, "clamped to the last row")
assert.equal(Model.dropIndex(2, -400, 40, 3), 0, "clamped to the first row")
assert.equal(Model.dropIndex(1, -10, 40, 3), 1, "a small drag does not reorder")

// ---------------------------------------------------------------- persistence

// Defaults are deliberately not written, so shell.json stays readable and a
// future change to a default still reaches users who never touched it.
const payload = Model.persistPayload([{ slug: "a/b", label: "", muted: false }], {})
assert.deepEqual(payload, { repos: [{ slug: "a/b" }] })

const tuned = Model.persistPayload(
  [{ slug: "a/b", label: "Thing", branch: "main", muted: true }],
  { idleInterval: 600 }
)
assert.deepEqual(tuned.repos[0], { slug: "a/b", label: "Thing", branch: "main", muted: true })
assert.equal(tuned.idleInterval, 600)
assert.equal(tuned.focusedInterval, undefined, "unchanged defaults are omitted")

// A round trip through persist -> read must be stable.
assert.deepEqual(Model.reposIn(tuned).map(r => r.slug), ["a/b"])
assert.equal(Model.settingsIn(tuned).idleInterval, 600)

// -------------------------------------------------------------- notifications

assert.equal(Model.shouldNotify({ to: "failing" }, {}), true)
assert.equal(Model.shouldNotify({ to: "failing" }, { notifyFailures: false }), false)
assert.equal(Model.shouldNotify({ from: "failing", to: "passing" }, {}), false)
assert.equal(Model.shouldNotify({ from: "failing", to: "passing" }, { notifyRecoveries: true }), true)
assert.equal(Model.shouldNotify({ from: "passing", to: "running" }, {}), false)
assert.equal(Model.shouldNotify(null, {}), false)

assert.equal(Model.notificationFor({ to: "failing", label: "kops", workflow: "CI" }).urgency, "critical")
assert.equal(Model.notificationFor({ to: "passing", label: "kops", workflow: "CI" }).urgency, "normal")

console.log("model.test.js: all assertions passed")

// ------------------------------------------------------------ theme palette

// A real Omarchy colors.toml, trimmed. The `bright_` variants come after the
// plain ones and must not clobber them.
const themeToml = `
mode = "dark"
accent = "#89b4fa"
muted = "#585b70"
foreground = "#cdd6f4"
red = "#f38ba8"
yellow = "#f9e2af"
orange = "#f6b6ab"
green = "#a6e3a1"
bright_red = "#111111"
bright_green = "#222222"
`
const palette = Model.parsePalette(themeToml)
assert.equal(palette.green, "#a6e3a1")
assert.equal(palette.red, "#f38ba8")
assert.equal(palette.bright_red, "#111111", "bright variants are still available")
assert.equal(Model.parsePalette("").green, undefined)
assert.deepEqual(Model.parsePalette(null), {})
assert.deepEqual(Model.parsePalette("not a toml file at all"), {})

// One table drives the bar badge and every row, so the same state cannot look
// like two different things depending on where it is drawn.
assert.equal(Model.statusColor(palette, "passing"), "#a6e3a1")
assert.equal(Model.statusColor(palette, "running"), "#f9e2af")
assert.equal(Model.statusColor(palette, "failing"), "#f38ba8")
assert.equal(Model.statusColor(palette, "stale"), "#585b70")
assert.equal(Model.statusColor(palette, "unknown"), "#585b70")

// A theme without `yellow` falls back to `orange` rather than to nothing.
assert.equal(Model.statusColor({ orange: "#ff8800" }, "running"), "#ff8800")
// A theme defining neither hands the decision back to the caller.
assert.equal(Model.statusColor({}, "running"), "")
assert.equal(Model.statusColor(null, "passing"), "")
assert.deepEqual(Model.statusColorKeys("running"), ["yellow", "orange"])
