import QtQuick
import Quickshell
import qs.Ui
import qs.Commons
import "Model.js" as Model

// The bar button and its popup, built once per monitor.
//
// This file owns no helper, no snapshot and no timer that matters: all of that
// lives in Service.qml, which the shell loads exactly once. Everything here is
// a view onto that engine plus this popup's own cursor, scroll position and
// text fields — the things that genuinely differ per screen.
Panel {
  id: root
  moduleName: "oma.pipelines"

  // The `oma.pipelines` IPC target belongs to the service, which exists once.
  // A widget registering it would register it once per monitor.
  manageIpc: false

  // The host may replace moduleName with an instance id. Keep the manifest id
  // stable for registry lookups and filesystem paths.
  readonly property string manifestPluginId: "oma.pipelines"

  readonly property var engine: bar && bar.shell && typeof bar.shell.serviceFor === "function"
    ? bar.shell.serviceFor(manifestPluginId)
    : null

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color accent: Color.accent
  readonly property color dim: Qt.darker(foreground, 1.4)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  // ------------------------------------------------------------ engine state

  readonly property var repoViews: engine ? engine.repoViews : []
  readonly property var repos: engine ? engine.repos : []
  readonly property var summary: engine ? engine.summary : ({})
  readonly property var auth: engine ? engine.auth : ({})
  readonly property var tuning: engine ? engine.tuning : Model.settingsIn({})
  readonly property bool connected: engine ? engine.connected : false
  readonly property bool polling: engine ? engine.polling : false
  readonly property bool backendReady: engine ? engine.backendReady : false
  readonly property string errorText: engine ? engine.errorText : "The pipelines engine is not loaded."
  readonly property int nowSeconds: engine ? engine.nowSeconds : 0
  readonly property string worst: summary && summary.worst ? String(summary.worst) : "unknown"

  // ---------------------------------------------------------- per-view state

  // "overview" | "detail" | "settings" | "connect"
  property string view: "overview"
  property int detailIndex: -1
  property int selectedIndex: -1
  property bool cursorActive: false
  property bool viewRegistered: false

  // Add-repository field state. Validation is two-stage: a synchronous verdict
  // as the user types, then an asynchronous confirmation from GitHub.
  property string addText: ""
  property string addState: "empty"     // empty|invalid|duplicate|ready|checking|ok|missing
  property string addMessage: ""
  property int addSequence: 0

  // Token field state.
  property string tokenText: ""
  property string tokenError: ""
  property bool tokenBusy: false

  // Drag-to-reorder state.
  property int dragIndex: -1
  property real dragOffset: 0

  readonly property var detailRepo: detailIndex >= 0 && detailIndex < repoViews.length
    ? repoViews[detailIndex]
    : null

  // Animations stay off until the widget has settled one beat after creation,
  // so a shell reload or a theme swap does not replay every entrance at once.
  property bool armed: false
  readonly property bool animate: armed && (!bar || bar.foregroundAnimationEnabled !== false)

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Component.onCompleted: { armTimer.start(); syncOpenState() }
  Timer { id: armTimer; interval: 400; onTriggered: root.armed = true }

  // ------------------------------------------------------------- bar presence

  // Silent when everything is green. An indicator that speaks when there is
  // nothing to say is one people learn to stop reading.
  readonly property string barCount: Model.barLabel(summary)
  // A fresh install has nothing to report, and the unknown glyph there reads as
  // "something is broken" rather than "nothing is set up yet". The two empty
  // states are also distinct actions, so they get distinct glyphs: sign in
  // versus add a project. The GitHub mark is reserved for the former — several
  // other widgets use it, and two identical marks side by side tell the user
  // nothing.
  readonly property string barGlyph: !backendReady ? "\u{f071}"
    : !connected ? "\u{f09b}"
    : repoViews.length === 0 ? "\u{f055}"
    : Model.glyphFor(worst)
  readonly property color barColor: {
    if (!backendReady) return urgent
    if (repoViews.length === 0) return dim
    if (worst === "failing" || worst === "stale") return urgent
    if (worst === "running") return accent
    return foreground
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.barCount === "" ? root.barGlyph : root.barGlyph + "  " + root.barCount
    fontFamily: root.fontFamily
    active: root.worst === "failing" || !root.backendReady
    activeColor: root.urgent
    tooltipText: root.backendReady
      ? Model.tooltipFor({ repos: root.repoViews, auth: root.auth }, root.nowSeconds)
      : root.errorText
    onPressed: function(code) { root.toggle() }

    // A run in progress makes the glyph breathe. It is the only motion in the
    // bar, so it reads as "something is happening" without a spinner's noise.
    SequentialAnimation on opacity {
      running: root.worst === "running" && root.animate
      loops: Animation.Infinite
      alwaysRunToEnd: true
      NumberAnimation { to: 0.45; duration: 900; easing.type: Easing.InOutSine }
      NumberAnimation { to: 1.0; duration: 900; easing.type: Easing.InOutSine }
    }
    onVisibleChanged: if (!visible) opacity = 1.0
  }

  // ---------------------------------------------------------------- lifecycle

  function syncOpenState() {
    if (!engine || opened === viewRegistered) return
    viewRegistered = opened
    if (opened) engine.viewOpened()
    else engine.viewClosed()
  }
  onEngineChanged: { viewRegistered = false; syncOpenState() }
  Component.onDestruction: if (engine && viewRegistered) engine.viewClosed()

  onOpenedChanged: {
    if (opened) {
      cursorActive = false
      selectedIndex = -1
      // Land on the connect screen when there is nothing else to show: the
      // first thing a new user can do should be the thing they need to do.
      view = connected ? "overview" : "connect"
      syncOpenState()
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    } else {
      syncOpenState()
      resetTransient()
    }
  }

  function resetTransient() {
    addText = ""
    addState = "empty"
    addMessage = ""
    tokenText = ""
    tokenError = ""
    dragIndex = -1
    dragOffset = 0
  }

  Connections {
    target: root.engine
    function onRepoAdded(slug) { root.addText = ""; root.addState = "empty"; root.addMessage = "" }
  }

  // ------------------------------------------------------------- add-repo flow

  // Two-stage validation. The synchronous verdict is instant and covers shape
  // and duplicates; only a syntactically valid, non-duplicate slug is worth
  // spending a request to confirm.
  function onAddTextChanged(text) {
    addText = text
    var slug = Model.slugFromInput(text)
    var verdict = Model.slugVerdict(slug, repos)
    addState = verdict.state
    addMessage = verdict.message
    addSequence++
    if (verdict.state === "ready") validateTimer.restart()
    else validateTimer.stop()
  }

  Timer {
    id: validateTimer
    interval: 420   // long enough that typing a slug is one request, not ten
    repeat: false
    onTriggered: {
      if (!root.engine || root.addState !== "ready") return
      var slug = Model.slugFromInput(root.addText)
      var sequence = root.addSequence
      root.addState = "checking"
      root.addMessage = "Checking…"
      root.engine.validateRepo(slug, function(reply) {
        // A reply for a slug the user has since edited must not overwrite the
        // verdict for what is in the field now.
        if (sequence !== root.addSequence) return
        if (reply.ok) {
          root.addState = "ok"
          root.addMessage = reply.data && reply.data.private ? "Private repository" : "Found"
        } else {
          root.addState = "missing"
          root.addMessage = String(reply.error || "Not found")
        }
      })
    }
  }

  function commitAdd() {
    if (!engine) return
    if (addState !== "ok" && addState !== "ready") return
    engine.addRepo(Model.slugFromInput(addText))
  }

  function submitToken() {
    if (!engine || tokenBusy) return
    var token = String(tokenText || "").trim()
    if (token === "") { tokenError = "Paste a token first"; return }
    tokenBusy = true
    tokenError = ""
    engine.connectWithToken(token, function(reply) {
      root.tokenBusy = false
      if (reply.ok) { root.tokenText = ""; root.popView("overview") }
      else root.tokenError = String(reply.error || "That token was refused")
    })
  }

  function importFromGh() {
    if (!engine || tokenBusy) return
    tokenBusy = true
    tokenError = ""
    engine.connectFromGh(function(reply) {
      root.tokenBusy = false
      if (reply.ok) root.popView("overview")
      else root.tokenError = String(reply.error || "The GitHub CLI has no usable token")
    })
  }

  // ------------------------------------------------------------- reordering

  readonly property real rowHeight: Style.space(34)

  function endDrag() {
    if (dragIndex < 0) { dragOffset = 0; return }
    var target = Model.dropIndex(dragIndex, dragOffset, rowHeight, repos.length)
    if (target !== dragIndex && engine) engine.moveRepo(dragIndex, target)
    dragIndex = -1
    dragOffset = 0
  }

  function nudge(index, delta) {
    if (!engine) return
    engine.moveRepo(index, index + delta)
  }

  // ------------------------------------------------------------- keyboard

  function moveCursor(dx, dy) {
    cursorActive = true
    var count = view === "overview" ? repoViews.length
      : (view === "detail" && detailRepo ? detailRepo.runs.length : 0)
    if (count > 0 && dy !== 0) {
      selectedIndex = Math.max(0, Math.min(count - 1, selectedIndex + dy))
    }
  }

  function activateCursor() {
    if (view === "overview" && selectedIndex >= 0 && selectedIndex < repoViews.length) {
      openRepo(selectedIndex)
    } else if (view === "detail" && detailRepo && selectedIndex >= 0) {
      var run = detailRepo.runs[selectedIndex]
      if (run && engine) engine.openRun(run.url)
    }
  }

  // Navigation. `navDirection` is +1 going deeper and -1 coming back, and the
  // content slides that way, so the transition itself says which direction you
  // moved rather than leaving the view to change with no explanation.
  property int navDirection: 1

  function pushView(next) {
    if (view === next) return
    navDirection = 1
    selectedIndex = -1
    cursorActive = false
    view = next
  }

  function popView(next) {
    if (view === next) return
    navDirection = -1
    selectedIndex = -1
    cursorActive = false
    view = next
  }

  function openRepo(index) {
    detailIndex = index
    pushView("detail")
  }

  readonly property bool inSubpage: view !== "overview"

  function goBack() {
    // The connect screen is the root when there is no account: there is
    // nothing behind it to go back to, so Escape closes the panel instead of
    // stranding the user on an empty overview.
    if (view === "connect" && !connected) { close(); return }
    if (view === "detail") { popView("overview"); return }
    if (view === "connect") { popView("settings"); return }
    if (view === "settings") { popView("overview"); return }
    close()
  }

  onViewChanged: navSlide.restart()

  ParallelAnimation {
    id: navSlide
    running: false
    NumberAnimation {
      target: content
      property: "x"
      from: root.navDirection * Style.space(18)
      to: 0
      duration: root.animate ? 190 : 0
      easing.type: Easing.OutCubic
    }
    NumberAnimation {
      target: content
      property: "opacity"
      from: 0.4
      to: 1.0
      duration: root.animate ? 190 : 0
      easing.type: Easing.OutQuad
    }
  }

  // ------------------------------------------------------------------ popup

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(content.implicitHeight, Style.space(620))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: addField.activeFocus || tokenField.activeFocus
      onMoveRequested: function(dx, dy) { root.moveCursor(dx, dy) }
      onActivateRequested: root.activateCursor()
      onCloseRequested: root.goBack()

      Column {
        id: content
        width: parent.width
        spacing: Style.space(12)

        // ------------------------------------------------------------- hero

        PanelHero {
          id: hero
          title: root.view === "detail" && root.detailRepo ? Model.rowTitle(root.detailRepo)
            : root.view === "settings" ? "Settings"
            : root.view === "connect" ? "Connect GitHub"
            : "Pipelines"
          meta: root.heroMeta
          detail: ""
          foreground: root.foreground
          fontFamily: root.fontFamily

          // In a subpage the leading glyph becomes the way out. A back arrow
          // where the icon was is the one place people already look for it,
          // and it makes the hierarchy visible instead of implied.
          iconComponent: root.inSubpage ? backControl : statusGlyph
          trailingControl: root.view === "overview" ? overviewActions
            : root.view === "detail" ? detailActions : null
        }

        Component {
          id: statusGlyph
          Text {
            text: root.barGlyph
            color: root.barColor
            font.family: root.fontFamily
            font.pixelSize: Style.font.display
            Behavior on color { enabled: root.animate; ColorAnimation { duration: 220 } }
          }
        }

        Component {
          id: backControl
          Item {
            implicitWidth: Style.font.display
            implicitHeight: Style.font.display
            Text {
              id: backArrow
              anchors.centerIn: parent
              text: "\u{f053}"
              color: backMouse.containsMouse ? root.accent : root.foreground
              font.family: root.fontFamily
              font.pixelSize: Style.font.title
              Behavior on color { enabled: root.animate; ColorAnimation { duration: 120 } }
              // Nudges toward the direction it will take you.
              x: backMouse.containsMouse ? -Style.space(2) : 0
              Behavior on x { enabled: root.animate; NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
            }
            MouseArea {
              id: backMouse
              anchors.fill: parent
              anchors.margins: -Style.space(6)
              hoverEnabled: true
              cursorShape: Qt.PointingHandCursor
              onClicked: root.goBack()
            }
            PanelToolTip {
              visible: backMouse.containsMouse
              text: "Back  ·  Esc"
              fontFamily: root.fontFamily
            }
          }
        }

        Component {
          id: overviewActions
          Row {
            spacing: Style.space(4)
            Button {
              iconText: "\u{f021}"
              bordered: false
              foreground: root.foreground
              fontFamily: root.fontFamily
              iconSpinning: root.polling
              tooltipText: "Refresh now"
              onClicked: if (root.engine) root.engine.refresh(true)
            }
            Button {
              iconText: "\u{f013}"
              bordered: false
              foreground: root.foreground
              fontFamily: root.fontFamily
              tooltipText: "Settings"
              onClicked: root.pushView("settings")
            }
          }
        }

        Component {
          id: detailActions
          Button {
            iconText: "\u{f09b}"
            bordered: false
            foreground: root.foreground
            fontFamily: root.fontFamily
            tooltipText: "Open this repository on GitHub"
            onClicked: if (root.engine && root.detailRepo) {
              root.engine.openRun("https://github.com/" + root.detailRepo.slug + "/actions")
            }
          }
        }

        PanelSeparator { width: parent.width }

        // --------------------------------------------------------- overview

        Column {
          visible: root.view === "overview"
          width: parent.width
          spacing: Style.space(4)

          PanelSectionHeader {
            text: "PROJECTS"
            foreground: root.foreground
            fontFamily: root.fontFamily
          }

          Text {
            visible: root.repoViews.length === 0
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: !root.backendReady ? root.errorText
              : (!root.connected ? "Connect a GitHub account to start tracking."
                                 : "No projects yet — add one in settings.")
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            topPadding: Style.space(12)
            bottomPadding: Style.space(12)
          }

          Repeater {
            model: root.repoViews
            delegate: Item {
              id: overviewRow
              required property var modelData
              required property int index
              width: parent ? parent.width : 0
              height: root.rowHeight

              readonly property bool hot: (root.cursorActive && root.selectedIndex === index)
                || rowMouse.containsMouse
              readonly property color stateColor: modelData.muted ? root.dim
                : (modelData.health === "failing" || modelData.health === "stale") ? root.urgent
                : modelData.health === "running" ? root.accent
                : root.foreground

              Rectangle {
                anchors.fill: parent
                radius: Style.cornerRadius
                color: overviewRow.hot ? Style.hoverFill : "transparent"
                Behavior on color { enabled: root.animate; ColorAnimation { duration: 120 } }
              }

              Row {
                anchors.left: parent.left
                anchors.leftMargin: Style.space(10)
                anchors.right: rowMeta.left
                anchors.rightMargin: Style.space(8)
                anchors.verticalCenter: parent.verticalCenter
                spacing: Style.space(8)

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  text: Model.glyphFor(overviewRow.modelData.health)
                  color: overviewRow.stateColor
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.body
                }

                // Owner and name on one line, the owner dimmed. Two projects
                // called `core` in different orgs were previously
                // indistinguishable, and the org is the part you need to tell
                // them apart, not the part to hide.
                Row {
                  anchors.verticalCenter: parent.verticalCenter
                  spacing: 0
                  Text {
                    text: Model.ownerPrefix(overviewRow.modelData.slug)
                    color: root.dim
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                  }
                  Text {
                    text: Model.rowTitle(overviewRow.modelData)
                    color: overviewRow.stateColor
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                  }
                }
              }

              // Age plus a chevron. The chevron is what says "this row goes
              // somewhere" — without it the subpage is a surprise.
              Row {
                id: rowMeta
                anchors.right: parent.right
                anchors.rightMargin: Style.space(10)
                anchors.verticalCenter: parent.verticalCenter
                spacing: Style.space(6)

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  textFormat: Text.PlainText
                  text: overviewRow.modelData.error !== "" ? "unreachable"
                    : Model.relativeTime(overviewRow.modelData.checkedAt, root.nowSeconds)
                  color: root.dim
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                }

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  text: "\u{f054}"
                  color: overviewRow.hot ? overviewRow.stateColor : root.dim
                  opacity: overviewRow.hot ? 1.0 : 0.55
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                  Behavior on opacity { enabled: root.animate; NumberAnimation { duration: 120 } }
                }
              }

              MouseArea {
                id: rowMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onEntered: { root.cursorActive = true; root.selectedIndex = overviewRow.index }
                onClicked: root.openRepo(overviewRow.index)
              }
            }
          }
        }

        // ----------------------------------------------------------- detail

        Column {
          visible: root.view === "detail" && root.detailRepo !== null
          width: parent.width
          spacing: Style.space(4)

          PanelSectionHeader {
            text: "WORKFLOWS"
            foreground: root.foreground
            fontFamily: root.fontFamily
          }

          Text {
            visible: root.detailRepo && root.detailRepo.error !== ""
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: root.detailRepo ? root.detailRepo.error : ""
            color: root.urgent
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            bottomPadding: Style.space(6)
          }

          Text {
            visible: root.detailRepo && root.detailRepo.runs.length === 0
            width: parent.width
            textFormat: Text.PlainText
            text: "No workflow runs yet"
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            topPadding: Style.space(12)
            bottomPadding: Style.space(12)
          }

          Repeater {
            model: root.detailRepo ? root.detailRepo.runs : []
            delegate: Column {
              required property var modelData
              required property int index
              width: parent ? parent.width : 0

              Button {
                width: parent.width
                leftAlign: true
                bordered: false
                iconText: Model.glyphFor(modelData.health)
                text: modelData.workflow + "  #" + modelData.number
                foreground: modelData.health === "failing" ? root.urgent
                  : modelData.health === "running" ? root.accent
                  : root.foreground
                fontFamily: root.fontFamily
                hasCursor: root.cursorActive && root.selectedIndex === index
                tooltipText: modelData.message
                onHovered: function(on) { if (on) { root.cursorActive = true; root.selectedIndex = index } }
                onClicked: if (root.engine) root.engine.openRun(modelData.url)
              }

              Text {
                x: Style.space(28)
                width: parent.width - Style.space(28)
                textFormat: Text.PlainText
                elide: Text.ElideRight
                text: Model.runSubtitle(modelData)
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                bottomPadding: Style.space(6)
              }
            }
          }
        }

        // --------------------------------------------------------- settings

        Column {
          visible: root.view === "settings"
          width: parent.width
          spacing: Style.space(8)

          PanelSectionHeader {
            text: "ACCOUNT"
            foreground: root.foreground
            fontFamily: root.fontFamily
          }

          Text {
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            // The login is unknown until the first poll resolves it. "Connected
            // as ?" reads as a fault; saying only what we actually know does not.
            text: !root.connected ? "Not connected"
              : root.auth.login
                ? ("Connected as " + root.auth.login + "  ·  " + (root.auth.sourceLabel || ""))
                : ("Connected  ·  " + (root.auth.sourceLabel || "GitHub"))
            color: root.connected ? root.foreground : root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
          }

          // A plaintext fallback is a thing the user is entitled to know about.
          Text {
            visible: root.auth.storage === "state-file"
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: "Stored in a plain file — unlock your login keyring to move it"
            color: root.urgent
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }

          Row {
            spacing: Style.space(6)
            Button {
              text: root.connected ? "Reconnect" : "Connect"
              bordered: true
              foreground: root.foreground
              fontFamily: root.fontFamily
              onClicked: { root.tokenError = ""; root.pushView("connect") }
            }
            Button {
              visible: root.connected
              text: "Disconnect"
              bordered: true
              foreground: root.urgent
              fontFamily: root.fontFamily
              onClicked: if (root.engine) root.engine.disconnect(function(reply) { root.pushView("connect") })
            }
          }

          PanelSeparator { width: parent.width }

          PanelSectionHeader {
            text: "PROJECTS"
            foreground: root.foreground
            fontFamily: root.fontFamily
          }

          // Reorderable list. Dragging moves a row; Alt+Up/Alt+Down does the
          // same thing from the keyboard, and both go through Model.moveItem
          // so they cannot disagree about the result.
          Column {
            id: repoEditor
            width: parent.width
            spacing: 0

            Repeater {
              model: root.repos
              delegate: Item {
                id: repoRow
                required property var modelData
                required property int index
                width: parent ? parent.width : 0
                height: root.rowHeight
                z: root.dragIndex === index ? 2 : 1

                // While a row is dragged, the others slide out of its way.
                readonly property real settledY: {
                  if (root.dragIndex < 0) return 0
                  if (index === root.dragIndex) return root.dragOffset
                  var target = Model.dropIndex(root.dragIndex, root.dragOffset, root.rowHeight, root.repos.length)
                  if (root.dragIndex < index && index <= target) return -root.rowHeight
                  if (target <= index && index < root.dragIndex) return root.rowHeight
                  return 0
                }
                y: index * root.rowHeight + settledY

                Behavior on y {
                  enabled: root.animate && root.dragIndex !== repoRow.index
                  NumberAnimation { duration: 160; easing.type: Easing.OutCubic }
                }

                Rectangle {
                  anchors.fill: parent
                  radius: Style.cornerRadius
                  color: root.dragIndex === index ? Style.hoverFill : "transparent"
                  border.width: root.dragIndex === index ? 1 : 0
                  border.color: Style.hoverBorderColor
                }

                Row {
                  anchors.verticalCenter: parent.verticalCenter
                  anchors.left: parent.left
                  anchors.right: parent.right
                  spacing: Style.space(6)

                  // Drag handle. Only the handle starts a drag, so the row's
                  // other controls stay clickable.
                  Item {
                    width: Style.space(20)
                    height: root.rowHeight
                    Text {
                      anchors.centerIn: parent
                      text: "\u{f0c9}"
                      color: root.dim
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.caption
                    }
                    MouseArea {
                      anchors.fill: parent
                      cursorShape: Qt.SizeVerCursor
                      onPressed: { root.dragIndex = repoRow.index; root.dragOffset = 0 }
                      onPositionChanged: function(mouse) {
                        if (root.dragIndex !== repoRow.index) return
                        root.dragOffset += mouse.y - (root.rowHeight / 2)
                      }
                      onReleased: root.endDrag()
                      onCanceled: root.endDrag()
                    }
                  }

                  Text {
                    width: parent.width - Style.space(96)
                    anchors.verticalCenter: parent.verticalCenter
                    textFormat: Text.PlainText
                    elide: Text.ElideMiddle
                    text: modelData.slug
                    color: modelData.muted ? root.dim : root.foreground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                  }

                  Button {
                    iconText: modelData.muted ? "\u{f070}" : "\u{f06e}"
                    bordered: false
                    foreground: root.dim
                    fontFamily: root.fontFamily
                    tooltipText: modelData.muted ? "Unmute" : "Mute — keep polling but never colour the bar"
                    onClicked: if (root.engine) root.engine.setRepoField(repoRow.index, "muted", !modelData.muted)
                  }

                  Button {
                    iconText: "\u{f00d}"
                    bordered: false
                    foreground: root.urgent
                    fontFamily: root.fontFamily
                    tooltipText: "Remove"
                    onClicked: if (root.engine) root.engine.removeRepo(repoRow.index)
                  }
                }

                Keys.onPressed: function(event) {
                  if (!(event.modifiers & Qt.AltModifier)) return
                  if (event.key === Qt.Key_Up) { root.nudge(repoRow.index, -1); event.accepted = true }
                  else if (event.key === Qt.Key_Down) { root.nudge(repoRow.index, 1); event.accepted = true }
                }
              }
            }
          }

          // The Repeater's delegates are absolutely positioned for the drag
          // animation, so the Column needs an explicit height for them.
          Item {
            width: parent.width
            height: root.repos.length * root.rowHeight
            visible: false
          }

          // ------------------------------------------------------- add repo

          Column {
            width: parent.width
            spacing: Style.space(4)

            TextField {
              id: addField
              width: parent.width
              foreground: root.foreground
              accent: root.accent
              placeholderText: "owner/repository, or paste a GitHub URL"
              text: root.addText
              onTextChanged: if (text !== root.addText) root.onAddTextChanged(text)
              Keys.onReturnPressed: root.commitAdd()
              Keys.onEnterPressed: root.commitAdd()
            }

            // The realtime verdict. Colour carries the same information as the
            // text so it is legible at a glance and still legible without it.
            Row {
              width: parent.width
              spacing: Style.space(6)
              visible: root.addMessage !== ""

              Text {
                text: root.addState === "ok" ? "\u{f00c}"
                  : root.addState === "checking" ? "\u{f021}"
                  : "\u{f071}"
                color: root.addState === "ok" ? root.accent
                  : root.addState === "checking" ? root.dim
                  : root.urgent
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption

                RotationAnimation on rotation {
                  running: root.addState === "checking" && root.animate
                  loops: Animation.Infinite
                  from: 0; to: 360; duration: 1100
                }
              }

              Text {
                textFormat: Text.PlainText
                text: root.addMessage
                color: root.addState === "ok" ? root.foreground : root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
            }

            Button {
              width: parent.width
              text: "Add project"
              bordered: true
              enabled: root.addState === "ok"
              foreground: root.addState === "ok" ? root.foreground : root.dim
              fontFamily: root.fontFamily
              onClicked: root.commitAdd()
            }
          }

          PanelSeparator { width: parent.width }

          // Refresh cadence and notifications are declared in manifest.json's
          // `barWidget.schema`, so Omarchy renders them in its own bar-widget
          // settings editor. Duplicating them here would mean two UIs writing
          // the same shell.json keys, which is how they end up disagreeing.
          Text {
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: "Refresh timing and notifications live in Bar settings → Pipelines."
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }
        }

        // ---------------------------------------------------------- connect

        Column {
          visible: root.view === "connect"
          width: parent.width
          spacing: Style.space(8)

          Text {
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: "Pipelines reads workflow runs through the GitHub API. "
              + "The quickest way is to reuse the token the GitHub CLI already has."
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }

          Button {
            width: parent.width
            text: root.tokenBusy ? "Checking…" : "Import from GitHub CLI"
            iconText: "\u{f09b}"
            bordered: true
            enabled: !root.tokenBusy
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.importFromGh()
          }

          PanelSectionHeader {
            text: "OR PASTE A TOKEN"
            foreground: root.foreground
            fontFamily: root.fontFamily
          }

          TextField {
            id: tokenField
            width: parent.width
            password: true
            foreground: root.foreground
            accent: root.accent
            placeholderText: "github_pat_… or ghp_…"
            text: root.tokenText
            onTextChanged: root.tokenText = text
            Keys.onReturnPressed: root.submitToken()
            Keys.onEnterPressed: root.submitToken()
          }

          Text {
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            visible: root.tokenError !== ""
            text: root.tokenError
            color: root.urgent
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }

          Text {
            width: parent.width
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: "A fine-grained token needs read access to Actions and Metadata "
              + "on the repositories you want to watch. It is stored in your login keyring."
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }

          Row {
            spacing: Style.space(6)
            Button {
              text: root.tokenBusy ? "Checking…" : "Save token"
              bordered: true
              enabled: !root.tokenBusy && root.tokenText !== ""
              foreground: root.foreground
              fontFamily: root.fontFamily
              onClicked: root.submitToken()
            }
            Button {
              visible: root.connected
              text: "Back"
              bordered: false
              foreground: root.dim
              fontFamily: root.fontFamily
              onClicked: root.goBack()
            }
          }
        }
      }
    }
  }

  // The one-line status under the title, per view. In a subpage it doubles as
  // the breadcrumb: the title is where you are, this says what it belongs to
  // and where you came from.
  readonly property string heroMeta: {
    if (!backendReady) return errorText
    if (view === "connect") {
      if (!connected) return "Not connected"
      return auth.login ? "Connected as " + auth.login : "Connected"
    }
    if (view === "settings") return "Pipelines  ›  " + repos.length
      + " project" + (repos.length === 1 ? "" : "s")
    if (view === "detail" && detailRepo) {
      // The owner belongs here rather than in the title: the title is the
      // thing you picked, this is the org it lives in.
      var owner = Model.ownerPrefix(detailRepo.slug).replace("/", "")
      var when = detailRepo.error !== ""
        ? "could not refresh"
        : "checked " + Model.relativeTime(detailRepo.checkedAt, nowSeconds)
      return (owner === "" ? "" : owner + "  ·  ") + when
    }
    if (!connected) return "Not connected"
    if (repoViews.length === 0) return "No projects yet"
    if (polling) return "Refreshing…"
    var s = summary
    if (s.failing > 0) return s.failing + " failing"
    if (s.running > 0) return s.running + " running"
    if (s.stale > 0) return s.stale + " not refreshed"
    return "All green"
  }
}
