// Pure state logic for oma.pipelines.
//
// Everything here is a plain function of its arguments: no QML types, no
// engine, no clock read that is not passed in. That is what makes it runnable
// under `node tests/model.test.js`, and it is why the panel and the service
// can both use it without either one owning it.
//
// QML loads this with `import "Model.js" as Model`; node loads it through the
// `module.exports` at the bottom.

// Coerce anything list-shaped into a real JavaScript array.
//
// `Array.isArray` is not safe on values that have crossed a QML property
// boundary. QML hands JavaScript a sequence-backed wrapper for some list
// properties: it indexes and it has `length`, but `Array.isArray` reports
// false. Guarding with `Array.isArray(x) ? x : []` therefore silently
// discarded real data — the overview captions came out blank even though the
// rows plainly had seven runs each, and the same guard sat in front of
// reordering and removal.
//
// Everything in this file that takes a list goes through here first.
function asList(value) {
  if (!value) return []
  if (Array.isArray(value)) return value
  // A string is indexable and has a length, so the duck-typing below would
  // happily turn "main" into ["m","a","i","n"]. Anything scalar is not a list.
  if (typeof value !== "object") return []
  var n = Number(value.length)
  if (!isFinite(n) || n <= 0) return []
  var out = []
  for (var i = 0; i < n; i++) out.push(value[i])
  return out
}

// ---------------------------------------------------------------- shell.json

// Find this plugin's entry in the bar layout, and split it into an id and the
// settings hanging off it.
//
// The entry is the source of truth rather than the `settings` the bar injects
// into a widget: the bar builds one widget per monitor and injects a tick
// later, so a widget reports the default first and a copy after. Reading the
// config directly means the persisted state and the effective state are the
// same value and cannot drift.
function barEntry(config, pluginId) {
  var id = String(pluginId || "")
  if (!config || typeof config !== "object" || id === "") return null
  var groups = []
  var layout = config.bar && typeof config.bar === "object" ? config.bar.layout : null
  var regions = ["left", "center", "right"]
  for (var r = 0; r < regions.length; r++) {
    if (layout) groups.push(asList(layout[regions[r]]))
  }
  groups.push(asList(config.plugins))
  for (var g = 0; g < groups.length; g++) {
    for (var e = 0; e < groups[g].length; e++) {
      var entry = groups[g][e]
      if (!entry || typeof entry !== "object") continue
      if (String(entry.id || "") !== id) continue
      var settings = {}
      for (var key in entry) if (key !== "id") settings[key] = entry[key]
      return { id: String(entry.id), settings: settings }
    }
  }
  return null
}

// The configuration as far as this plugin can see it, in the shape `barEntry`
// reads.
//
// Before Omarchy 4 the shell injected itself, and it carries all of shell.json
// as `shellConfig`. Omarchy 4 injects a capability-scoped PluginShellApi into a
// third-party plugin instead: it has no `shellConfig` at all, only a copy of
// the bar subtree as `barConfig`. Reading `shellConfig` alone found no entry
// there, so every configured repository disappeared from the panel while
// shell.json still listed them all. Either shape is accepted; the whole config
// wins when both exist, because only it has `plugins[]`.
function configView(shellConfig, barConfig) {
  if (shellConfig && typeof shellConfig === "object") return shellConfig
  if (barConfig && typeof barConfig === "object") return { bar: barConfig }
  return null
}

// The entry to write back: `entry` with the keys this plugin owns replaced by
// `payload`, and every other key kept exactly where it was.
//
// Omarchy's bar-widget settings editor writes into the same entry from the
// schema in manifest.json, and a later Omarchy may add keys of its own. The
// write Omarchy 4 offers a plugin replaces the whole entry with what it is
// given, so anything not carried over here is deleted from shell.json. An
// owned key the payload leaves out is dropped, because `persistPayload` omits
// defaults on purpose. Key order is kept, so a write that changes nothing
// serialises exactly as the entry already does.
function withOwnedKeys(entry, payload) {
  var owned = ["repos", "focusedInterval", "activeInterval", "idleInterval",
               "reservePercent", "notifyFailures", "notifyRecoveries", "notifyTimeout"]
  var current = entry && typeof entry === "object" ? entry : {}
  var next = payload && typeof payload === "object" ? payload : {}
  var out = {}
  for (var key in current) {
    if (key in next) out[key] = next[key]
    else if (owned.indexOf(key) === -1) out[key] = current[key]
  }
  for (var field in next) if (!(field in out)) out[field] = next[field]
  return out
}

// Read the watch list out of a settings blob, dropping anything malformed.
// A hand-edited shell.json is a supported way to configure this, so bad input
// is expected rather than exceptional.
function reposIn(settings) {
  if (!settings || typeof settings !== "object") return []
  var input = asList(settings.repos)
  var out = []
  var seen = {}
  for (var i = 0; i < input.length; i++) {
    var raw = input[i]
    var spec = null
    // A bare string is accepted so that hand-editing shell.json does not
    // require knowing the object shape.
    if (typeof raw === "string") spec = { slug: raw }
    else if (raw && typeof raw === "object") spec = raw
    if (!spec) continue
    var slug = String(spec.slug || "").trim()
    if (!isValidSlug(slug) || seen[slug]) continue
    seen[slug] = true
    out.push({
      slug: slug,
      label: String(spec.label || ""),
      branch: String(spec.branch || ""),
      workflow: String(spec.workflow || ""),
      muted: spec.muted === true
    })
  }
  return out
}

// Tuning, clamped to the same bounds the helper enforces so the settings UI
// cannot show a value the helper would silently reject.
//
// `notifyTimeout` never reaches the helper's logic, only its parser, which
// ignores it. Its ceiling is Omarchy's instead: the notification daemon caps a
// notification's time on screen at 30 seconds.
function settingsIn(settings) {
  var raw = settings && typeof settings === "object" ? settings : {}
  return {
    focusedInterval: clamp(numberOr(raw.focusedInterval, 15), 10, 3600),
    activeInterval: clamp(numberOr(raw.activeInterval, 30), 15, 3600),
    idleInterval: clamp(numberOr(raw.idleInterval, 180), 30, 21600),
    reservePercent: clamp(numberOr(raw.reservePercent, 25), 0, 90),
    notifyFailures: raw.notifyFailures !== false,
    notifyRecoveries: raw.notifyRecoveries === true,
    notifyTimeout: clamp(statedNumberOr(raw.notifyTimeout, 10), 0, 30)
  }
}

function numberOr(value, fallback) {
  var n = Number(value)
  return isFinite(n) ? n : fallback
}

// `numberOr` for a setting where 0 means something. `Number` reads null, "",
// [] and false as 0, so an emptied field or a hand-edited null would otherwise
// mean "keep every notification on screen" rather than "use the default".
function statedNumberOr(value, fallback) {
  var stated = typeof value === "number" || (typeof value === "string" && value.trim() !== "")
  return stated ? numberOr(value, fallback) : fallback
}

function clamp(value, low, high) {
  return Math.max(low, Math.min(high, Math.round(value)))
}

// ------------------------------------------------------------------ validity

// GitHub's own rule: alphanumerics, hyphen, underscore, dot; one slash.
// Kept identical to `split_slug` in the Rust helper — if these two disagree,
// the UI accepts something the helper refuses, which reads as a silent bug.
function isValidSlug(slug) {
  return /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/.test(String(slug || ""))
}

// The realtime verdict for the add-repository field, before any network call.
// `checking` and `ok` come later, from the helper.
function slugVerdict(text, existing) {
  var slug = String(text || "").trim()
  if (slug === "") return { state: "empty", message: "" }
  if (slug.indexOf("/") === -1) return { state: "invalid", message: "Needs owner/repository" }
  if (!isValidSlug(slug)) return { state: "invalid", message: "Not a valid repository name" }
  var list = asList(existing)
  for (var i = 0; i < list.length; i++) {
    if (String(list[i].slug || "").toLowerCase() === slug.toLowerCase()) {
      return { state: "duplicate", message: "Already being watched" }
    }
  }
  return { state: "ready", message: "" }
}

// Accept a pasted GitHub URL as well as a slug: pasting the address bar is
// what people actually do.
function slugFromInput(text) {
  var raw = String(text || "").trim()
  if (raw === "") return ""
  var match = raw.match(/^(?:https?:\/\/)?(?:www\.)?github\.com\/([^/\s]+)\/([^/\s?#]+)/i)
  if (match) {
    var name = match[2].replace(/\.git$/i, "")
    return match[1] + "/" + name
  }
  return raw.replace(/^\/+|\/+$/g, "")
}

// ------------------------------------------------------------------ protocol

function parseLine(line) {
  try {
    var value = JSON.parse(String(line || ""))
    return value && typeof value === "object" ? value : null
  } catch (_) {
    return null
  }
}

// The helper is refused unless it speaks a protocol this build understands.
// A plugin folder can be replaced under a running shell by `omarchy plugin
// update`, so "the binary on disk is the one we were built against" is an
// assumption that does not hold.
function protocolAccepted(event, expected) {
  if (!event || event.ev !== "ready") return false
  return Number(event.protocol) === Number(expected)
}

// --------------------------------------------------------------- presentation

// Nerd Font glyphs. Omarchy ships Symbols Nerd Font, and the bar already
// assumes it for every first-party widget.
function glyphFor(health) {
  switch (String(health || "")) {
    case "passing": return "\u{f00c}"   // check
    case "failing": return "\u{f00d}"   // cross
    case "running": return "\u{f021}"   // refresh
    case "stale":   return "\u{f071}"   // warning triangle
    default:        return "\u{f128}"   // question
  }
}

// Pull the status colours out of a theme's `colors.toml`.
//
// Omarchy's `Color` singleton keeps only foreground, background, accent, muted
// and urgent, so there is no green and no amber to be had from it — but every
// theme's colors.toml defines the full terminal palette, and those are the
// colours the rest of the desktop is already using. Reading them directly is
// what makes the status badge match the theme instead of importing someone
// else's idea of green.
//
// Deliberately forgiving: a theme that omits a key gets the fallback rather
// than an error, and a hand-edited file cannot break the widget.
function parsePalette(text) {
  var out = {}
  var lines = String(text || "").split("\n")
  for (var i = 0; i < lines.length; i++) {
    var match = lines[i].match(/^\s*([A-Za-z0-9_-]+)\s*=\s*["']?(#[0-9A-Fa-f]{6})/)
    if (!match) continue
    var key = match[1].toLowerCase()
    // First definition wins: colors.toml lists the plain names before the
    // `bright_` variants, and the plain ones are the intended palette.
    if (out[key] === undefined) out[key] = match[2]
  }
  return out
}

// Map a health to a colour name in that palette, with the fallback chain to
// use when a theme does not define it.
//
// One table, used by the bar badge and by every row in the panel, because the
// alternative is what this replaced: the bar calling "running" amber while the
// panel called it accent-blue, so the same state looked like two different
// things depending on where you saw it.
function statusColorKeys(health) {
  switch (String(health || "")) {
    case "passing": return ["green"]
    case "running": return ["yellow", "orange"]
    case "failing": return ["red"]
    // Stale is "we could not refresh this", which is an absence of knowledge
    // rather than a state of the build. It gets the muted colour, not a
    // warning colour that would compete with a real failure.
    case "stale":   return ["muted", "dark_foreground"]
    default:        return ["muted", "dark_foreground"]
  }
}

// Resolve a health to a concrete colour string, or "" to mean "caller decides".
function statusColor(palette, health) {
  var p = palette && typeof palette === "object" ? palette : {}
  var keys = statusColorKeys(health)
  for (var i = 0; i < keys.length; i++) {
    if (typeof p[keys[i]] === "string" && p[keys[i]] !== "") return p[keys[i]]
  }
  return ""
}

// Derive the single state the bar shows. The helper sends this precomputed;
// the fallback keeps the function usable on a bare summary.
function worstOf(summary) {
  var s = summary && typeof summary === "object" ? summary : {}
  if (s.worst) return String(s.worst)
  if (Number(s.failing) > 0) return "failing"
  if (Number(s.stale) > 0) return "stale"
  if (Number(s.running) > 0) return "running"
  if (Number(s.passing) > 0) return "passing"
  return "unknown"
}

// "4m ago". Seconds are never shown: a CI dashboard that reports "3s ago"
// invites staring at it, and the poll cadence makes that precision a lie.
function relativeTime(unixSeconds, nowSeconds) {
  var then = Number(unixSeconds) || 0
  var now = Number(nowSeconds) || 0
  if (then <= 0) return "never"
  var delta = Math.max(0, now - then)
  if (delta < 60) return "just now"
  var minutes = Math.floor(delta / 60)
  if (minutes < 60) return minutes + "m ago"
  var hours = Math.floor(minutes / 60)
  if (hours < 24) return hours + "h ago"
  var days = Math.floor(hours / 24)
  if (days < 30) return days + "d ago"
  return Math.floor(days / 30) + "mo ago"
}

// "2m 40s". A negative duration means the run has not finished.
function formatDuration(seconds) {
  var total = Number(seconds)
  if (!isFinite(total) || total < 0) return ""
  if (total < 60) return Math.round(total) + "s"
  var minutes = Math.floor(total / 60)
  var rest = Math.round(total % 60)
  if (minutes < 60) return rest === 0 ? minutes + "m" : minutes + "m " + rest + "s"
  var hours = Math.floor(minutes / 60)
  return hours + "h " + (minutes % 60) + "m"
}

// One line for the bar tooltip: the most interesting repository, not the first.
function tooltipFor(snapshot, nowSeconds) {
  var snap = snapshot && typeof snapshot === "object" ? snapshot : {}
  var repos = asList(snap.repos)
  if (repos.length === 0) return "No repositories yet — open to add one"
  if (snap.auth && snap.auth.connected === false) return "Not connected to GitHub"

  var worst = null
  var rank = { failing: 4, stale: 3, running: 2, passing: 1, unknown: 0 }
  for (var i = 0; i < repos.length; i++) {
    var repo = repos[i]
    if (repo.muted) continue
    if (!worst || (rank[repo.health] || 0) > (rank[worst.health] || 0)) worst = repo
  }
  if (!worst) return repos.length + " repositories muted"

  var runs = asList(worst.runs)
  var run = runs.length ? runs[0] : null
  var when = relativeTime(worst.checkedAt, nowSeconds)
  if (!run) return worst.label + " · no runs yet"
  var verb = worst.health === "failing" ? "failed"
    : worst.health === "running" ? "running"
    : worst.health === "stale" ? "not refreshed" : "passed"
  return worst.label + " · " + run.workflow + " " + verb + " · " + when
}

// The owner half of a slug, with its slash, ready to be drawn dimmed in front
// of the repository name. Empty for anything that is not a valid slug, so the
// caller can fall back to showing the label alone.
function ownerPrefix(slug) {
  var raw = String(slug || "")
  var cut = raw.indexOf("/")
  if (cut <= 0 || cut === raw.length - 1) return ""
  return raw.slice(0, cut + 1)
}

// The repository half of a slug.
function repoName(slug) {
  var raw = String(slug || "")
  var cut = raw.indexOf("/")
  if (cut < 0 || cut === raw.length - 1) return raw
  return raw.slice(cut + 1)
}

// What to draw as the row's main label. A user-supplied label replaces the
// repository name but never the owner: knowing "Deploy" is called that is no
// use if you cannot tell which of two orgs it belongs to.
function rowTitle(repo) {
  var r = repo && typeof repo === "object" ? repo : {}
  var label = String(r.label || "")
  var name = repoName(r.slug)
  return label !== "" ? label : name
}

// The run a repository row is actually about.
//
// A repository's health is the worst of its workflows, so the newest run is
// often not the one that made the row red — a project can have `deploy` broken
// while `lint` passed a minute ago. Captioning a failing row with a green
// workflow's name is worse than captioning it with nothing.
//
// So: the run that justifies the row's state. The failing one when the row is
// failing, the running one when it is running, otherwise the most recent.
function leadRun(repo) {
  var r = repo && typeof repo === "object" ? repo : {}
  var runs = asList(r.runs)
  if (runs.length === 0) return null
  var wanted = String(r.health || "")
  if (wanted === "failing" || wanted === "running") {
    for (var i = 0; i < runs.length; i++) {
      if (String(runs[i].health || "") === wanted) return runs[i]
    }
  }
  return runs[0]
}

// The caption under a repository name: which workflow, and on what branch.
// Workflow first, so that when the line is too long it is the branch that gets
// cut rather than the name of the thing that broke.
function repoSubtitle(repo) {
  var run = leadRun(repo)
  if (!run) return ""
  var bits = []
  if (run.workflow) bits.push(String(run.workflow))
  if (run.branch) bits.push(String(run.branch))
  return bits.join(" · ")
}

// When the row's run last changed, for the right-hand column.
//
// This used to show when the *poll* last succeeded, which is a fact about this
// widget rather than about the project: "just now" on every row, always, and
// never an answer to "how long has this been broken".
function repoAge(repo, nowSeconds) {
  var run = leadRun(repo)
  if (!run) return ""
  // A running row answers "how long has this been going", not "how long ago
  // did it last change" — the latter is barely meaningful mid-run, and the
  // amber glyph beside it already says the count is climbing rather than
  // receding, so it needs no "ago".
  var elapsed = runElapsed(run, nowSeconds)
  if (elapsed !== "") return elapsed
  var stamp = Number(run.updatedAt) || 0
  if (stamp <= 0) return ""
  return relativeTime(stamp, nowSeconds)
}

// How long a run has been going, for one that has not finished.
//
// The helper reports duration -1 while a run is in flight, because it has no
// end to measure to. The elapsed time has to be derived against a clock, which
// is why it takes `nowSeconds` rather than reading one: this file stays a pure
// function of its arguments so it can be tested.
function runElapsed(run, nowSeconds) {
  var r = run && typeof run === "object" ? run : {}
  if (String(r.health || "") !== "running") return ""
  var started = Number(r.startedAt) || 0
  if (started <= 0) return ""
  return formatDuration(Math.max(0, (Number(nowSeconds) || 0) - started))
}

// The parts of a run's subtitle, kept separate so each can be truncated on its
// own terms.
//
// Joined into one string they had to elide as one, and since the branch comes
// first and is by far the most variable — `megrogge/fix-omni-window-voice` is a
// real example — a long branch pushed the author and the duration off the end
// entirely. The duration is the shortest and the most informative per
// character, so it is the last thing that should ever be dropped.
//
// Draw order and truncation priority are the reverse of each other: branch,
// author, duration left to right; duration, author, branch in what survives.
function runParts(run, nowSeconds) {
  var r = run && typeof run === "object" ? run : {}
  // The duration slot is the same slot whether the run has finished or is
  // still going; a run in flight fills it with time so far rather than
  // leaving a gap that reads as missing data.
  var elapsed = runElapsed(r, nowSeconds)
  return {
    branch: String(r.branch || ""),
    actor: String(r.actor || ""),
    duration: elapsed !== "" ? elapsed : formatDuration(r.duration)
  }
}

// --------------------------------------------------------------- list editing

// Move an item, returning a new array. Used by drag-to-reorder and by the
// keyboard shortcuts, so both paths provably agree.
function moveItem(list, from, to) {
  var array = asList(list).slice()
  if (from < 0 || from >= array.length) return array
  var target = Math.max(0, Math.min(array.length - 1, to))
  if (target === from) return array
  var item = array.splice(from, 1)[0]
  array.splice(target, 0, item)
  return array
}

function removeAt(list, index) {
  var array = asList(list).slice()
  if (index < 0 || index >= array.length) return array
  array.splice(index, 1)
  return array
}

function addRepo(list, slug) {
  var array = asList(list).slice()
  var clean = String(slug || "").trim()
  if (!isValidSlug(clean)) return array
  for (var i = 0; i < array.length; i++) {
    if (String(array[i].slug).toLowerCase() === clean.toLowerCase()) return array
  }
  array.push({ slug: clean, label: "", branch: "", workflow: "", muted: false })
  return array
}

function setFieldAt(list, index, field, value) {
  var array = asList(list).slice()
  if (index < 0 || index >= array.length) return array
  var copy = {}
  for (var key in array[index]) copy[key] = array[index][key]
  copy[field] = value
  array[index] = copy
  return array
}

// Where a dragged row should land, given its vertical offset in row heights.
// Split out from the QML so the arithmetic is testable without a scene graph.
function dropIndex(from, offsetPixels, rowHeight, count) {
  var height = Number(rowHeight) || 1
  var shift = Math.round(Number(offsetPixels) / height)
  return Math.max(0, Math.min(Math.max(0, count - 1), from + shift))
}

// The payload written back into shell.json. Kept minimal on purpose: defaults
// are not persisted, so a config file stays readable and a future change to a
// default actually reaches users who never touched that setting.
function persistPayload(repos, settings) {
  var defaults = settingsIn({})
  var out = { repos: [] }
  var list = asList(repos)
  for (var i = 0; i < list.length; i++) {
    var repo = list[i]
    var entry = { slug: String(repo.slug) }
    if (repo.label) entry.label = String(repo.label)
    if (repo.branch) entry.branch = String(repo.branch)
    if (repo.workflow) entry.workflow = String(repo.workflow)
    if (repo.muted === true) entry.muted = true
    out.repos.push(entry)
  }
  var tuned = settingsIn(settings)
  for (var key in tuned) {
    if (tuned[key] !== defaults[key]) out[key] = tuned[key]
  }
  return out
}

// A transition worth a desktop notification, given the user's preferences.
function shouldNotify(transition, settings) {
  var t = transition && typeof transition === "object" ? transition : {}
  var prefs = settingsIn(settings)
  if (t.to === "failing") return prefs.notifyFailures === true
  if (t.from === "failing" && t.to === "passing") return prefs.notifyRecoveries === true
  return false
}

// What a transition says on the desktop, how loudly, and for how long.
//
// Failures used to go out as `critical`, which Omarchy's notification daemon
// keeps on screen until it is dismissed by hand — so every failure did, and a
// repository with a few flaky scheduled workflows stacked up a column of them.
// They now go out as `normal`, with a timeout. Nothing is lost when one clears
// itself: the bar stays red, and the daemon keeps it in its history. A timeout
// of 0 opts back into `critical`, the one urgency Omarchy never expires.
//
// The title names the repository first and the outcome last, the way the bar
// tooltip does; `transition.run` is absent from a helper older than this file,
// and then the title still stands on its own.
function notificationFor(transition, settings) {
  var t = transition && typeof transition === "object" ? transition : {}
  var run = t.run && typeof t.run === "object" ? t.run : {}
  var prefs = settingsIn(settings)
  var failed = t.to === "failing"
  var subject = []
  if (t.label) subject.push(String(t.label))
  if (t.workflow) subject.push(String(t.workflow))
  var verb = failed ? failureVerb(run.conclusion) : "recovered"
  return {
    urgency: prefs.notifyTimeout === 0 ? "critical" : "normal",
    timeout: prefs.notifyTimeout * 1000,
    title: (subject.join(" · ") + " " + verb).trim(),
    body: notificationBody(run),
    glyph: glyphFor(failed ? "failing" : "passing"),
    url: String(t.url || "")
  }
}

// GitHub's conclusion in the words of the title. Every conclusion the helper
// counts as failing (`classify` in github.rs) is a different thing to go and
// fix, so each gets its own words rather than one "failed" for all of them.
function failureVerb(conclusion) {
  switch (String(conclusion || "")) {
    case "timed_out":       return "timed out"
    case "startup_failure": return "failed to start"
    case "action_required": return "needs approval"
    default:                return "failed"
  }
}

// The lines under the title: branch and author, then the commit.
//
// A scheduled run has no author worth naming — GitHub credits whoever last
// edited the cron line — and its commit is just whatever the default branch
// held at the time, so it says "scheduled" and leaves the commit out rather
// than pointing at someone who did nothing.
//
// The body is markup to any daemon that advertises `body-markup`, Omarchy's
// included, and a branch name or a commit message is written by whoever
// pushed it. Escaped, a `<b>` in a commit message is text, not formatting.
function notificationBody(run) {
  var r = run && typeof run === "object" ? run : {}
  var scheduled = String(r.event || "") === "schedule"
  var where = []
  if (r.branch) where.push(String(r.branch))
  if (scheduled) where.push("scheduled")
  else if (r.actor) where.push(String(r.actor))
  var lines = []
  if (where.length > 0) lines.push(where.join(" · "))
  if (!scheduled && r.message) lines.push(String(r.message))
  return escapeMarkup(lines.join("\n"))
}

function escapeMarkup(text) {
  return String(text || "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
}

// Where a click on the notification goes: a run on github.com, and nowhere
// else. The click runs a program, so the URL is held to the one host the
// helper talks to rather than trusted because the helper sent it.
function runUrl(url) {
  var raw = String(url || "")
  return /^https:\/\/github\.com\/\S+$/.test(raw) ? raw : ""
}

// The argv that puts a notification on screen.
//
// D-Bus through busctl, never notify-send. notify-send parses its entire argv
// for options, so a body of `--hint=string:omarchy-exec-argv:[…]` becomes a
// hint instead of text — and the workflow name in that position is written by
// whoever writes the workflow file, which on a public repository includes
// anyone who opens a pull request. Omarchy runs that hint's argv when the
// notification is clicked. busctl takes every value after `--` as one typed
// argument, verbatim; Omarchy's own omarchy-notification-send uses it for the
// same reason.
//
// The click rides in Omarchy's `omarchy-exec-argv` hint: an argv the daemon
// runs without a shell, and still runs after a shell restart. A daemon that
// does not know the hint ignores it.
function notifyCommand(note) {
  var n = note && typeof note === "object" ? note : {}
  var hints = ["urgency", "y", n.urgency === "critical" ? "2" : "1"]
  if (n.glyph) hints.push("omarchy-glyph", "s", String(n.glyph))
  var url = runUrl(n.url)
  if (url !== "") hints.push("omarchy-exec-argv", "s", JSON.stringify(["xdg-open", url]))
  var timeout = Math.round(Number(n.timeout))
  return [
    "busctl", "--user", "--quiet", "--", "call",
    "org.freedesktop.Notifications", "/org/freedesktop/Notifications",
    "org.freedesktop.Notifications", "Notify", "susssasa{sv}i",
    // app name, replaces id, app icon, summary, body
    "Pipelines", "0", "", String(n.title || ""), String(n.body || ""),
    // no actions; then the hints as (key, type, value) triples
    "0", String(hints.length / 3)
  ].concat(hints, [String(isFinite(timeout) ? timeout : -1)])
}

if (typeof module !== "undefined") module.exports = {
  barEntry, configView, withOwnedKeys, reposIn, settingsIn, isValidSlug, slugVerdict, slugFromInput,
  asList, parseLine, protocolAccepted, glyphFor, worstOf,
  parsePalette, statusColor, statusColorKeys,
  ownerPrefix, repoName, rowTitle,
  relativeTime, formatDuration, tooltipFor, runParts, moveItem, removeAt,
  leadRun, repoSubtitle, repoAge, runElapsed,
  addRepo, setFieldAt, dropIndex, persistPayload, shouldNotify, notificationFor,
  notificationBody, escapeMarkup, runUrl, notifyCommand
}
