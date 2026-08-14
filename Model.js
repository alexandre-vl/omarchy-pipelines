// Pure state logic for oma.pipelines.
//
// Everything here is a plain function of its arguments: no QML types, no
// engine, no clock read that is not passed in. That is what makes it runnable
// under `node tests/model.test.js`, and it is why the panel and the service
// can both use it without either one owning it.
//
// QML loads this with `import "Model.js" as Model`; node loads it through the
// `module.exports` at the bottom.

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
    if (layout && Array.isArray(layout[regions[r]])) groups.push(layout[regions[r]])
  }
  if (Array.isArray(config.plugins)) groups.push(config.plugins)
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

// Read the watch list out of a settings blob, dropping anything malformed.
// A hand-edited shell.json is a supported way to configure this, so bad input
// is expected rather than exceptional.
function reposIn(settings) {
  if (!settings || typeof settings !== "object") return []
  if (!Array.isArray(settings.repos)) return []
  var out = []
  var seen = {}
  for (var i = 0; i < settings.repos.length; i++) {
    var raw = settings.repos[i]
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
function settingsIn(settings) {
  var raw = settings && typeof settings === "object" ? settings : {}
  return {
    focusedInterval: clamp(numberOr(raw.focusedInterval, 15), 10, 3600),
    activeInterval: clamp(numberOr(raw.activeInterval, 30), 15, 3600),
    idleInterval: clamp(numberOr(raw.idleInterval, 180), 30, 21600),
    reservePercent: clamp(numberOr(raw.reservePercent, 25), 0, 90),
    notifyFailures: raw.notifyFailures !== false,
    notifyRecoveries: raw.notifyRecoveries === true
  }
}

function numberOr(value, fallback) {
  var n = Number(value)
  return isFinite(n) ? n : fallback
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
  var list = Array.isArray(existing) ? existing : []
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

// The bar label: silent when everything is green, and otherwise exactly one
// number, belonging to the glyph beside it.
//
// It used to show failing and running counts side by side, which rendered as
// "✗ 2 1" — two bare numbers with nothing to say which was which, both in the
// failure colour. A count is only legible when it is unambiguous which thing
// is being counted, so the label now counts the state the glyph is already
// showing and nothing else.
//
// A count of one is omitted: the glyph alone already says "something is
// failing", and "✗ 1" adds a character without adding information.
function barLabel(summary) {
  var s = summary && typeof summary === "object" ? summary : {}
  var worst = worstOf(s)
  // Only states that are asking for attention get a number. "4 projects are
  // green" is not news, and a permanent digit in the bar is exactly the kind
  // of always-on detail this indicator exists to avoid.
  if (worst !== "failing" && worst !== "stale" && worst !== "running") return ""
  var count = Number(s[worst])
  if (!isFinite(count) || count < 2) return ""
  return String(count)
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
  var repos = Array.isArray(snap.repos) ? snap.repos : []
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

  var run = Array.isArray(worst.runs) && worst.runs.length ? worst.runs[0] : null
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

// A run's own one-line summary for the detail list.
function runSubtitle(run) {
  var r = run && typeof run === "object" ? run : {}
  var bits = []
  if (r.branch) bits.push(r.branch)
  if (r.actor) bits.push(r.actor)
  var duration = formatDuration(r.duration)
  if (duration) bits.push(duration)
  return bits.join(" · ")
}

// --------------------------------------------------------------- list editing

// Move an item, returning a new array. Used by drag-to-reorder and by the
// keyboard shortcuts, so both paths provably agree.
function moveItem(list, from, to) {
  var array = Array.isArray(list) ? list.slice() : []
  if (from < 0 || from >= array.length) return array
  var target = Math.max(0, Math.min(array.length - 1, to))
  if (target === from) return array
  var item = array.splice(from, 1)[0]
  array.splice(target, 0, item)
  return array
}

function removeAt(list, index) {
  var array = Array.isArray(list) ? list.slice() : []
  if (index < 0 || index >= array.length) return array
  array.splice(index, 1)
  return array
}

function addRepo(list, slug) {
  var array = Array.isArray(list) ? list.slice() : []
  var clean = String(slug || "").trim()
  if (!isValidSlug(clean)) return array
  for (var i = 0; i < array.length; i++) {
    if (String(array[i].slug).toLowerCase() === clean.toLowerCase()) return array
  }
  array.push({ slug: clean, label: "", branch: "", workflow: "", muted: false })
  return array
}

function setFieldAt(list, index, field, value) {
  var array = Array.isArray(list) ? list.slice() : []
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
  var list = Array.isArray(repos) ? repos : []
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

function notificationFor(transition) {
  var t = transition && typeof transition === "object" ? transition : {}
  if (t.to === "failing") {
    return { urgency: "critical", title: t.label + " failed", body: t.workflow }
  }
  return { urgency: "normal", title: t.label + " recovered", body: t.workflow }
}

if (typeof module !== "undefined") module.exports = {
  barEntry, reposIn, settingsIn, isValidSlug, slugVerdict, slugFromInput,
  parseLine, protocolAccepted, glyphFor, barLabel,
  worstOf, ownerPrefix, repoName, rowTitle,
  relativeTime, formatDuration, tooltipFor, runSubtitle, moveItem, removeAt,
  addRepo, setFieldAt, dropIndex, persistPayload, shouldNotify, notificationFor
}
