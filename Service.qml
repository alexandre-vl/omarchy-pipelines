import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// One pipelines engine per shell session, not one per monitor.
//
// The bar instantiates its widgets once per screen (Variants over
// Quickshell.screens). A helper owned by the widget would therefore be started
// once per monitor: two copies polling GitHub on the same token, doubling the
// request rate for nothing and racing each other for the same rate-limit
// budget. Registering the IPC target that many times has the same problem —
// the shell keeps the first registration and warns about the rest.
//
// The shell loads a `service` kind exactly once, so the helper, the snapshot
// and the IPC target live here. Every Panel.qml is a view onto this object and
// owns nothing but its own cursor and popup state.
Item {
  id: root

  // Injected by the shell when the service is created.
  property var shell: null
  property var manifest: null

  readonly property string manifestPluginId: "oma.pipelines"
  readonly property int expectedProtocol: 1

  readonly property string metadataSourceDir: manifest && manifest.__sourceDir
    ? String(manifest.__sourceDir)
    : ""
  readonly property string pluginDir: metadataSourceDir !== ""
    ? metadataSourceDir
    : (Quickshell.env("HOME") || "") + "/.config/omarchy/plugins/" + manifestPluginId

  // Configuration comes from shell.json, not from the widgets. A widget cannot
  // be the source: the bar builds one per monitor and injects `settings` a tick
  // after creation, so the first thing a widget can report is the default
  // rather than the persisted value.
  readonly property var configEntry: shell && shell.shellConfig
    ? Model.barEntry(shell.shellConfig, manifestPluginId)
    : null
  readonly property var pluginSettings: configEntry ? configEntry.settings : ({})
  readonly property var repos: Model.reposIn(pluginSettings)
  readonly property var tuning: Model.settingsIn(pluginSettings)

  // ------------------------------------------------------------------- state

  property string pluginVersion: ""
  property bool backendReady: false
  property bool protocolOk: true
  property string errorText: ""

  property var snapshot: ({ repos: [], summary: {}, budget: {}, auth: {}, polling: false })
  readonly property var summary: snapshot && snapshot.summary ? snapshot.summary : ({})
  readonly property var budget: snapshot && snapshot.budget ? snapshot.budget : ({})
  readonly property var auth: snapshot && snapshot.auth ? snapshot.auth : ({})
  readonly property var repoViews: snapshot && Array.isArray(snapshot.repos) ? snapshot.repos : []
  readonly property bool connected: auth && auth.connected === true
  readonly property bool polling: snapshot && snapshot.polling === true

  // A clock the views can bind to for "4m ago" without each one running its
  // own timer. Ticks once a minute, because nothing here is shown to the
  // second.
  property int nowSeconds: Math.floor(Date.now() / 1000)

  // Views own their own cursor and fields; the engine only asks.
  signal viewResetRequested()
  signal repoAdded(string slug)

  // In-flight request bookkeeping. `id` correlates a reply to its caller.
  property int nextRequestId: 1
  property var pending: ({})

  readonly property bool anyViewOpen: openViewCount > 0
  property int openViewCount: 0

  // ---------------------------------------------------------------- lifecycle

  FileView {
    path: root.pluginDir + "/manifest.json"
    printErrors: false
    onLoaded: {
      var parsed = Model.parseLine(text())
      root.pluginVersion = parsed && String(parsed.id || "") === root.manifestPluginId
        ? String(parsed.version || "")
        : ""
    }
    onLoadFailed: root.pluginVersion = ""
  }

  Timer {
    interval: 60000
    running: true
    repeat: true
    onTriggered: root.nowSeconds = Math.floor(Date.now() / 1000)
  }

  // The theme's terminal palette, for the status colours.
  //
  // Omarchy's `Color` singleton exposes foreground, background, accent, muted
  // and urgent — there is no green and no amber in it. Every theme's
  // colors.toml has the full palette, and those are the exact colours the rest
  // of the desktop is already using, so reading it here is what makes a passing
  // build the same green as everything else on screen.
  //
  // Loaded once per session rather than once per monitor, and watched so a
  // theme change is picked up without a restart.
  property var palette: ({})

  FileView {
    path: (Quickshell.env("HOME") || "") + "/.local/state/omarchy/current/theme/colors.toml"
    watchChanges: true
    printErrors: false
    onLoaded: root.palette = Model.parsePalette(text())
    onFileChanged: reload()
    onLoadFailed: root.palette = ({})
  }

  // Registered once, from the object there is one of. Opening a popup is still
  // the widget's job, so those calls are handed back to the shell, which routes
  // them to the focused monitor rather than to whichever instance spoke first.
  function summonView(action) {
    if (!shell || typeof shell[action] !== "function") return "unknown"
    return shell[action](manifestPluginId, "{}") === false ? "unknown" : "ok"
  }

  IpcHandler {
    target: "oma.pipelines"
    function open(): string { return root.summonView("summon") }
    function close(): string { return root.summonView("hide") }
    function toggle(): string { return root.summonView("toggle") }
    function refresh(): string { root.refresh(true); return "ok" }
    function status(): string {
      return JSON.stringify({
        connected: root.connected,
        repos: root.repoViews.length,
        summary: root.summary,
        remaining: root.budget ? root.budget.remaining : 0
      })
    }
  }

  Process {
    id: backend
    property bool launched: false
    command: [root.pluginDir + "/bin/omarchy-pipelines-helper"]
    running: false
    stdinEnabled: true
    stdout: SplitParser { onRead: function(line) { root.handleEvent(Model.parseLine(line)) } }
    stderr: SplitParser { onRead: function(line) { console.warn("pipelines helper:", line) } }

    onStarted: backend.launched = true

    // Quickshell reports a command it could not launch by returning `running`
    // to false without ever emitting `started` or `exited`, so a missing binary
    // has to be caught here. An exit code never carries that news: nothing runs
    // a shell on our behalf, so the 127 a shell would report cannot reach us.
    onRunningChanged: {
      if (running) { backend.launched = false; return }
      if (!backend.launched) {
        root.backendReady = false
        root.errorText = "The pipelines helper is missing — run ./build.sh in the plugin folder"
        return
      }
      root.backendReady = false
      if (!root.shuttingDown) restartTimer.restart()
    }

    onExited: function(code) {
      root.backendReady = false
      if (code !== 0 && !root.shuttingDown) {
        root.errorText = "The pipelines helper stopped unexpectedly (exit " + code + ")"
      }
    }
  }

  property bool shuttingDown: false

  // Restart with a delay rather than immediately: a helper that crashes on
  // startup would otherwise be respawned in a tight loop for as long as the
  // session lasts.
  Timer {
    id: restartTimer
    interval: 5000
    repeat: false
    onTriggered: if (!root.shuttingDown && !backend.running) backend.running = true
  }

  Component.onCompleted: backend.running = true
  Component.onDestruction: {
    root.shuttingDown = true
    if (backend.running) root.send({ cmd: "shutdown" })
  }

  // ------------------------------------------------------------------- IPC

  function send(command, onReply) {
    if (!backend.running) return -1
    var id = nextRequestId++
    var payload = { id: id }
    for (var key in command) payload[key] = command[key]
    if (onReply) {
      var copy = pending
      copy[id] = onReply
      pending = copy
    }
    backend.write(JSON.stringify(payload) + "\n")
    return id
  }

  function handleEvent(event) {
    if (!event) return
    switch (String(event.ev || "")) {
    case "ready":
      // Refuse a helper whose protocol this build does not know. A plugin
      // folder can be replaced under a running shell by `omarchy plugin
      // update`, so "the binary on disk matches this QML" is not a given.
      if (!Model.protocolAccepted(event, expectedProtocol)) {
        protocolOk = false
        errorText = "The pipelines helper speaks protocol " + event.protocol
          + ", this plugin expects " + expectedProtocol + " — restart the shell"
        backend.running = false
        return
      }
      protocolOk = true
      backendReady = true
      errorText = ""
      send({ cmd: "hello", version: pluginVersion })
      pushConfiguration()
      break

    case "state":
      snapshot = event
      nowSeconds = Math.floor(Date.now() / 1000)
      break

    case "transition":
      if (Model.shouldNotify(event, pluginSettings)) {
        var note = Model.notificationFor(event)
        Quickshell.execDetached([
          "notify-send", "-a", "Pipelines",
          "-u", note.urgency,
          note.title, note.body
        ])
      }
      break

    case "reply":
      var handler = pending[event.id]
      if (handler) {
        var copy = pending
        delete copy[event.id]
        pending = copy
        handler(event)
      }
      break
    }
  }

  // Hand the helper the watch list and tuning. Called on every config change,
  // because the helper deliberately persists none of this itself.
  function pushConfiguration() {
    if (!backendReady) return
    send({ cmd: "configure", repos: repos, settings: tuning })
  }

  onReposChanged: pushConfiguration()
  onTuningChanged: pushConfiguration()

  // ------------------------------------------------------------------ actions

  function refresh(force) { send({ cmd: "refresh", force: force === true }) }

  function connectFromGh(callback) { send({ cmd: "auth-detect" }, callback) }
  function connectWithToken(token, callback) {
    send({ cmd: "auth-set", token: token, source: "pat" }, callback)
  }
  function disconnect(callback) { send({ cmd: "auth-clear" }, callback) }
  function validateRepo(slug, callback) { send({ cmd: "repo-validate", slug: slug }, callback) }
  function suggestRepos(query, refreshList, callback) {
    send({ cmd: "repo-suggest", query: query, refresh: refreshList === true }, callback)
  }

  function openRun(url) {
    if (!url) return
    Quickshell.execDetached(["xdg-open", String(url)])
  }

  // --------------------------------------------------------------- persistence

  // Write the watch list back into shell.json through the shell's own mutator,
  // so the change lands in the same file, with the same formatting, as every
  // other setting the user can change from the UI.
  function persist(nextRepos, nextTuning) {
    if (!shell || typeof shell.mutateShellConfig !== "function") {
      errorText = "This Omarchy build cannot save plugin settings"
      return false
    }
    var payload = Model.persistPayload(nextRepos, nextTuning)
    var id = manifestPluginId
    shell.mutateShellConfig(function(config) {
      var groups = []
      if (config.bar && config.bar.layout) {
        var regions = ["left", "center", "right"]
        for (var r = 0; r < regions.length; r++) {
          if (Array.isArray(config.bar.layout[regions[r]])) groups.push(config.bar.layout[regions[r]])
        }
      }
      if (Array.isArray(config.plugins)) groups.push(config.plugins)
      for (var g = 0; g < groups.length; g++) {
        for (var e = 0; e < groups[g].length; e++) {
          var entry = groups[g][e]
          if (!entry || typeof entry !== "object" || String(entry.id || "") !== id) continue
          // Only touch the keys this plugin owns. Omarchy's own bar-widget
          // settings editor writes into this same entry from the schema in
          // manifest.json, and a wholesale replace here would silently discard
          // whatever it had set — including keys added by a later Omarchy.
          var managed = ["repos", "focusedInterval", "activeInterval", "idleInterval",
                         "reservePercent", "notifyFailures", "notifyRecoveries"]
          for (var m = 0; m < managed.length; m++) {
            if (!(managed[m] in payload)) delete entry[managed[m]]
          }
          for (var field in payload) entry[field] = payload[field]
          return
        }
      }
    })
    return true
  }

  function addRepo(slug) {
    var next = Model.addRepo(repos, slug)
    if (next.length === repos.length) return false
    if (!persist(next, tuning)) return false
    repoAdded(slug)
    return true
  }

  function removeRepo(index) { persist(Model.removeAt(repos, index), tuning) }
  function moveRepo(from, to) { persist(Model.moveItem(repos, from, to), tuning) }
  function setRepoField(index, field, value) {
    persist(Model.setFieldAt(repos, index, field, value), tuning)
  }

  // ------------------------------------------------------------- view counting

  // The bar builds one panel per monitor, so "a panel is open" is a count.
  // A flag would let closing the popup on one screen slow the poll cadence
  // while it is still open on another.
  function viewOpened() {
    openViewCount++
    if (openViewCount === 1) {
      send({ cmd: "focus", active: true })
      refresh(false)
    }
  }

  function viewClosed() {
    if (openViewCount > 0) openViewCount--
    if (openViewCount === 0) send({ cmd: "focus", active: false })
  }
}
