// Tests Service.qml's reads and writes of shell.json against the hosts it runs
// under.
//
// This is the one code path that `node tests/model.test.js` cannot reach.
// Model.js is pure and covered there; what it cannot cover is whether
// Service.persist finds the right entry in a real config shape, writes only
// the keys this plugin owns, and leaves every neighbouring widget alone. That
// path backs adding, removing, muting and reordering — all of which were once
// broken at the same time by a single bad guard, silently, with a clean build.
//
// It runs twice, because Omarchy has injected two different things into a
// plugin's `shell` property:
//
// - Before Omarchy 4, the shell itself: `shellConfig` holding all of
//   shell.json, and a `mutateShellConfig` that runs over the whole of it.
// - From Omarchy 4, a capability-scoped PluginShellApi: no `shellConfig`, a
//   copy of the bar subtree as `barConfig`, a `mutateShellConfig` that refuses
//   anything but a full bar, and an `updateEntryInline` that replaces the
//   plugin's own entry. The plugin read nothing and wrote nothing under it —
//   every configured repository vanished from the panel — while this file,
//   which only knew the first host, kept passing.
//
// The second host is built from the shell's own PluginShellApi.qml rather than
// from a description of it, so the next change to that surface fails here.
//
// Run it with `tests/persist.sh`, which needs Quickshell and an Omarchy shell
// for the `qs.Ui` imports and PluginShellApi.qml. CI cannot run it.

import QtQuick
import Quickshell

ShellRoot {
  id: harness

  // A config with a neighbour on either side, so a mutation that is too
  // enthusiastic shows up as a missing sibling rather than passing quietly.
  // `futureKey` stands for a key this plugin does not own: one the settings
  // editor wrote, or one a later Omarchy adds.
  function freshConfig() {
    return {
      bar: {
        layout: {
          left: [{ id: "omarchy.menu" }],
          right: [
            { id: "omarchy.clock", format: "HH:mm" },
            { id: "oma.pipelines", repos: [{ slug: "a/1" }, { slug: "a/2" }, { slug: "a/3" }],
              idleInterval: 600, futureKey: "kept" },
            { id: "omarchy.power" }
          ]
        }
      }
    }
  }

  property var cfg: freshConfig()
  property int failures: 0
  property var hosts: ["legacy", "scoped"]
  property int hostIndex: -1
  property var scopedShell: null

  function check(what, actual, expected) {
    if (String(actual) === String(expected)) {
      console.warn("  ok   " + what)
    } else {
      failures++
      console.warn("  FAIL " + what + "\n         expected: " + expected + "\n         actual:   " + actual)
    }
  }

  function entry() { return harness.cfg.bar.layout.right[1] }
  function slugsOf(list) {
    var out = []
    for (var i = 0; i < list.length; i++) out.push(list[i].slug)
    return out.join(",")
  }
  function slugs() { return slugsOf(entry().repos || []) }

  // ------------------------------------------------------------------ hosts

  // Before Omarchy 4: the shell itself. Mirrors shell.qml: hand the mutator a
  // deep clone and keep the result.
  QtObject {
    id: legacyShell
    property var shellConfig: null
    function mutateShellConfig(mutator) {
      var clone = JSON.parse(JSON.stringify(shellConfig))
      mutator(clone)
      shellConfig = clone
      harness.cfg = clone
    }
    function serviceFor(id) { return null }
  }

  // Omarchy 4: the shell's own PluginShellApi.qml, wired the way shell.qml
  // wires it for an ordinary third-party plugin.
  function makeScopedShell() {
    var component = Qt.createComponent(Qt.resolvedUrl("host/PluginShellApi.qml"))
    if (component.status !== Component.Ready) {
      failures++
      console.warn("  FAIL could not load the shell's PluginShellApi.qml: " + component.errorString())
      return null
    }
    return component.createObject(harness, {
      pluginId: "oma.pipelines",
      barConfig: JSON.parse(JSON.stringify(harness.cfg.bar)),
      // Bar capabilities belong to plugins of kind `bar` only, so shell.qml
      // refuses this without calling the mutator.
      _mutateBarConfig: function(mutator) { return false },
      _updateSettings: function(requestedId, settings) {
        if (String(requestedId) !== "oma.pipelines") return false
        return harness.updateEntryInline(requestedId, settings)
      }
    })
  }

  // Mirrors shell.qml's updateEntryInline: the entry becomes `{ id }` plus
  // exactly what it was handed, and the answer is whether anything changed.
  // The shell then pushes a fresh copy of the bar subtree into every plugin's
  // API, which is where the plugin reads its own configuration back from.
  function updateEntryInline(moduleName, settings) {
    var copy = JSON.parse(JSON.stringify(harness.cfg))
    var dirty = false
    var sections = ["left", "center", "right"]
    for (var s = 0; s < sections.length; s++) {
      var entries = copy.bar.layout[sections[s]] || []
      for (var i = 0; i < entries.length; i++) {
        if (!entries[i] || entries[i].id !== moduleName) continue
        var next = { id: moduleName }
        for (var key in settings) if (key !== "id") next[key] = settings[key]
        if (JSON.stringify(entries[i]) !== JSON.stringify(next)) {
          entries[i] = next
          dirty = true
        }
      }
    }
    if (!dirty) return false
    harness.cfg = copy
    if (harness.scopedShell) harness.scopedShell.barConfig = JSON.parse(JSON.stringify(copy.bar))
    return true
  }

  // --------------------------------------------------------------- scenarios

  Loader {
    id: loader
    onLoaded: Qt.callLater(harness.runScenario)
  }

  Component.onCompleted: nextHost()

  function nextHost() {
    hostIndex++
    if (hostIndex >= hosts.length) {
      console.warn(failures === 0
        ? "persist.qml: all assertions passed"
        : "persist.qml: " + failures + " FAILED")
      Qt.quit()
      return
    }
    cfg = freshConfig()
    var host = null
    if (hosts[hostIndex] === "legacy") {
      legacyShell.shellConfig = cfg
      host = legacyShell
    } else {
      scopedShell = makeScopedShell()
      host = scopedShell
    }
    if (!host) { nextHost(); return }
    // Initial properties, so the plugin directory points somewhere with no
    // helper binary before Component.onCompleted starts one: the test must
    // neither spawn a process nor touch the GitHub API. The real Omarchy 4
    // strips `__sourceDir` from a third-party manifest, which would point the
    // plugin at the helper actually installed on this machine — so it stays.
    loader.setSource("plugin/Service.qml", {
      manifest: { __sourceDir: "/tmp/omarchy-pipelines-nobin" },
      shell: host
    })
  }

  function runScenario() {
    var item = loader.item
    console.warn("shell.json persistence — " + hosts[hostIndex] + " host")

    check("reads the configured repositories", slugsOf(item.repos), "a/1,a/2,a/3")
    check("reads a non-default setting", item.tuning.idleInterval, 600)
    check("starts from the configured order", slugs(), "a/1,a/2,a/3")

    item.moveRepo(0, 2)
    check("moves a row to the end", slugs(), "a/2,a/3,a/1")
    item.moveRepo(2, 0)
    check("moves it back", slugs(), "a/1,a/2,a/3")

    item.setRepoField(1, "muted", true)
    check("mutes one row", JSON.stringify(entry().repos[1]), '{"slug":"a/2","muted":true}')

    item.removeRepo(0)
    check("removes a row", slugs(), "a/2,a/3")
    item.addRepo("z/9")
    check("adds a row", slugs(), "a/2,a/3,z/9")
    check("reads its own write back", slugsOf(item.repos), "a/2,a/3,z/9")
    item.addRepo("z/9")
    check("refuses a duplicate", slugs(), "a/2,a/3,z/9")
    item.addRepo("not-a-slug")
    check("refuses a malformed slug", slugs(), "a/2,a/3,z/9")
    check("saving unchanged state is not a failure", item.persist(item.repos, item.tuning), true)

    check("keeps a non-default setting", entry().idleInterval, 600)
    check("keeps a key it does not own", entry().futureKey, "kept")
    check("keeps the id", entry().id, "oma.pipelines")
    check("leaves the widget before it alone",
          JSON.stringify(cfg.bar.layout.right[0]), '{"id":"omarchy.clock","format":"HH:mm"}')
    check("leaves the widget after it alone",
          JSON.stringify(cfg.bar.layout.right[2]), '{"id":"omarchy.power"}')
    check("leaves the other section alone",
          JSON.stringify(cfg.bar.layout.left), '[{"id":"omarchy.menu"}]')

    if (hosts[hostIndex] === "scoped") {
      // A host that refuses every write must hear about it as a failure, not
      // a success: claiming to have saved is how this broke unnoticed.
      scopedShell._updateSettings = function(requestedId, settings) { return false }
      check("reports a refused write", item.addRepo("q/1"), false)
      check("changes nothing it was refused", slugs(), "a/2,a/3,z/9")
    }

    loader.source = ""
    Qt.callLater(harness.nextHost)
  }
}
