// Tests Service.qml's writes to shell.json against a stand-in shell.
//
// This is the one code path that `node tests/model.test.js` cannot reach.
// Model.js is pure and covered there; what it cannot cover is whether
// Service.persist finds the right entry in a real config shape, writes only
// the keys this plugin owns, and leaves every neighbouring widget alone. That
// path backs adding, removing, muting and reordering — all of which were once
// broken at the same time by a single bad guard, silently, with a clean build.
//
// Run it with `tests/persist.sh`, which needs Quickshell and an Omarchy shell
// checkout for the `qs.Ui` imports. CI cannot run it.

import QtQuick
import Quickshell

ShellRoot {
  id: harness

  // A config with a neighbour on either side, so a mutation that is too
  // enthusiastic shows up as a missing sibling rather than passing quietly.
  property var cfg: ({
    bar: {
      layout: {
        left: [{ id: "omarchy.menu" }],
        right: [
          { id: "omarchy.clock", format: "HH:mm" },
          { id: "oma.pipelines", repos: [{ slug: "a/1" }, { slug: "a/2" }, { slug: "a/3" }], idleInterval: 600 },
          { id: "omarchy.power" }
        ]
      }
    }
  })

  property int failures: 0

  function check(what, actual, expected) {
    if (String(actual) === String(expected)) {
      console.warn("  ok   " + what)
    } else {
      failures++
      console.warn("  FAIL " + what + "\n         expected: " + expected + "\n         actual:   " + actual)
    }
  }

  function entry() { return harness.cfg.bar.layout.right[1] }
  function slugs() {
    var repos = entry().repos || []
    var out = []
    for (var i = 0; i < repos.length; i++) out.push(repos[i].slug)
    return out.join(",")
  }

  QtObject {
    id: fakeShell
    property var shellConfig: harness.cfg
    // Mirrors shell.qml: hand the mutator a deep clone and keep the result.
    function mutateShellConfig(mutator) {
      var clone = JSON.parse(JSON.stringify(shellConfig))
      mutator(clone)
      shellConfig = clone
      harness.cfg = clone
    }
    function serviceFor(id) { return null }
  }

  Loader {
    source: "plugin/Service.qml"
    onLoaded: {
      // Point the plugin directory at somewhere with no helper binary, so the
      // test neither spawns a process nor touches the GitHub API.
      item.manifest = { __sourceDir: "/tmp/omarchy-pipelines-nobin" }
      item.shell = fakeShell

      Qt.callLater(function() {
        console.warn("shell.json persistence")
        harness.check("starts from the configured order", harness.slugs(), "a/1,a/2,a/3")

        item.moveRepo(0, 2)
        harness.check("moves a row to the end", harness.slugs(), "a/2,a/3,a/1")
        item.moveRepo(2, 0)
        harness.check("moves it back", harness.slugs(), "a/1,a/2,a/3")

        item.setRepoField(1, "muted", true)
        harness.check("mutes one row", JSON.stringify(harness.entry().repos[1]),
                      '{"slug":"a/2","muted":true}')

        item.removeRepo(0)
        harness.check("removes a row", harness.slugs(), "a/2,a/3")
        item.addRepo("z/9")
        harness.check("adds a row", harness.slugs(), "a/2,a/3,z/9")
        item.addRepo("z/9")
        harness.check("refuses a duplicate", harness.slugs(), "a/2,a/3,z/9")
        item.addRepo("not-a-slug")
        harness.check("refuses a malformed slug", harness.slugs(), "a/2,a/3,z/9")

        harness.check("keeps a non-default setting", harness.entry().idleInterval, 600)
        harness.check("keeps the id", harness.entry().id, "oma.pipelines")
        harness.check("leaves the widget before it alone",
                      JSON.stringify(harness.cfg.bar.layout.right[0]),
                      '{"id":"omarchy.clock","format":"HH:mm"}')
        harness.check("leaves the widget after it alone",
                      JSON.stringify(harness.cfg.bar.layout.right[2]), '{"id":"omarchy.power"}')
        harness.check("leaves the other section alone",
                      JSON.stringify(harness.cfg.bar.layout.left), '[{"id":"omarchy.menu"}]')

        console.warn(harness.failures === 0
          ? "persist.qml: all assertions passed"
          : "persist.qml: " + harness.failures + " FAILED")
        Qt.quit()
      })
    }
  }
}
