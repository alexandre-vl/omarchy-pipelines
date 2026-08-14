// Manifest contract tests.
//
// `omarchy plugin validate` enforces most of this, but it only exists on an
// Omarchy machine and CI runs on stock Ubuntu. These assertions mirror the
// checks in that script — and in the shell's PluginRegistry.qml — so a manifest
// the shell would reject cannot reach main and be discovered by a user whose
// bar silently loses a widget.

const assert = require("node:assert/strict")
const fs = require("node:fs")
const path = require("node:path")

const root = path.join(__dirname, "..")
const manifest = JSON.parse(fs.readFileSync(path.join(root, "manifest.json"), "utf8"))

// --------------------------------------------------------------- shell schema

assert.equal(manifest.schemaVersion, 1, "the registry understands only schemaVersion 1")
for (const field of ["id", "name", "version", "kinds", "entryPoints"]) {
  assert.ok(Object.hasOwn(manifest, field), `missing required field '${field}'`)
}

assert.match(manifest.id, /^[A-Za-z0-9][A-Za-z0-9._-]*$/, "invalid plugin id")
assert.ok(!manifest.id.includes(".."), "plugin id may not contain '..'")
assert.ok(!manifest.id.startsWith("omarchy."), "omarchy.* is a reserved namespace")

assert.ok(Array.isArray(manifest.kinds) && manifest.kinds.length > 0, "kinds must be non-empty")
assert.equal(typeof manifest.entryPoints, "object")

// A kind is a promise to supply something to load. Claiming one without its
// entry point installs and enables a plugin that then does nothing.
const kindEntryPoints = {
  bar: "bar", "bar-widget": "barWidget", menu: "menu",
  overlay: "overlay", panel: "panel", service: "service"
}
for (const kind of manifest.kinds) {
  const key = kindEntryPoints[kind]
  if (!key) continue
  assert.ok(manifest.entryPoints[key], `kind '${kind}' requires entryPoints.${key}`)
}

// Entry points must be safe relative paths that actually exist.
for (const [key, value] of Object.entries(manifest.entryPoints)) {
  assert.ok(typeof value === "string" && value.length > 0, `entryPoints.${key} is empty`)
  assert.ok(!value.startsWith("/"), `entryPoints.${key} must be relative`)
  assert.ok(!value.includes(".."), `entryPoints.${key} may not contain '..'`)
  assert.ok(!value.includes("\n"), `entryPoints.${key} may not contain a newline`)
  assert.ok(fs.existsSync(path.join(root, value)), `entryPoints.${key} -> ${value} does not exist`)
}

if (manifest.barWidget && manifest.barWidget.defaultSection) {
  assert.ok(
    ["left", "center", "right"].includes(manifest.barWidget.defaultSection),
    "barWidget.defaultSection must be left, center or right"
  )
}

// The shell refuses a plugin folder containing a symlink, because one could
// point back at an arbitrary file once the folder lands in the trusted plugins
// directory.
function assertNoSymlinks(dir) {
  for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
    if (item.name === ".git" || item.name === "target" || item.name === "node_modules") continue
    const full = path.join(dir, item.name)
    assert.ok(!item.isSymbolicLink(), `symlinks are not allowed in a plugin folder: ${full}`)
    if (item.isDirectory()) assertNoSymlinks(full)
  }
}
assertNoSymlinks(root)

// ------------------------------------------------------ settings schema sanity

const widget = manifest.barWidget || {}
const defaults = widget.defaults || {}
const schema = widget.schema || []

const seen = new Set()
for (const field of schema) {
  assert.ok(field.key, "every schema field needs a key")
  assert.ok(!seen.has(field.key), `duplicate schema key '${field.key}'`)
  seen.add(field.key)
  assert.ok(field.label, `schema field '${field.key}' needs a label`)
  assert.ok(field.description, `schema field '${field.key}' needs a description`)
  assert.ok(
    ["integer", "boolean", "string"].includes(field.type),
    `schema field '${field.key}' has an unsupported type`
  )
  // Omarchy's settings editor renders from the schema but seeds from
  // `defaults`; a key in one and not the other reads as a control that resets
  // itself.
  assert.ok(Object.hasOwn(defaults, field.key), `'${field.key}' is missing from barWidget.defaults`)
  assert.equal(defaults[field.key], field.defaultValue, `'${field.key}' default disagrees with schema`)
  if (field.type === "integer") {
    assert.equal(typeof field.min, "number", `'${field.key}' needs a min`)
    assert.equal(typeof field.max, "number", `'${field.key}' needs a max`)
    assert.ok(field.min <= field.defaultValue && field.defaultValue <= field.max,
      `'${field.key}' default is outside its own range`)
  }
}
for (const key of Object.keys(defaults)) {
  assert.ok(seen.has(key), `default '${key}' has no schema entry, so nothing can change it`)
}

// ------------------------------------------- schema agrees with the helper

// The helper clamps every one of these in `Settings::sanitized`. If the schema
// lets the settings editor offer a value the helper silently clamps, the user
// sets 5 seconds and gets 10 with no explanation.
const helperClamps = {
  focusedInterval: [10, 3600],
  activeInterval: [15, 3600],
  idleInterval: [30, 21600],
  reservePercent: [0, 90]
}
for (const [key, [low, high]] of Object.entries(helperClamps)) {
  const field = schema.find(f => f.key === key)
  assert.ok(field, `schema is missing '${key}'`)
  assert.ok(field.min >= low, `'${key}' min ${field.min} is below the helper's floor ${low}`)
  assert.ok(field.max <= high, `'${key}' max ${field.max} is above the helper's ceiling ${high}`)
}

// --------------------------------------------------------- version agreement

const cargo = fs.readFileSync(path.join(root, "backend", "Cargo.toml"), "utf8")
const cargoVersion = (cargo.match(/^version\s*=\s*"([^"]+)"/m) || [])[1]
assert.equal(
  manifest.version, cargoVersion,
  "manifest.json and backend/Cargo.toml must carry the same version"
)

// The Model and the helper must agree on the protocol number, or the shell
// refuses its own helper at runtime.
const service = fs.readFileSync(path.join(root, "Service.qml"), "utf8")
const qmlProtocol = (service.match(/expectedProtocol:\s*(\d+)/) || [])[1]
const rustProtocol = (fs.readFileSync(path.join(root, "backend", "src", "protocol.rs"), "utf8")
  .match(/PROTOCOL:\s*u32\s*=\s*(\d+)/) || [])[1]
assert.equal(qmlProtocol, rustProtocol, "Service.qml and protocol.rs disagree about the protocol version")

console.log("manifest.test.js: all assertions passed")
