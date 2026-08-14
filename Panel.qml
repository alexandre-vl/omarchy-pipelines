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
  readonly property string worst: Model.worstOf(summary)

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

  // The mark never changes; the badge on it does.
  //
  // Swapping the whole glyph between check, cross, spinner and question meant
  // the widget had no stable identity in the bar — you had to read it to know
  // it was even this plugin. A constant GitHub mark with a small status badge
  // is the pattern a CI favicon uses in a browser tab, and it works for the
  // same reason: recognition and status are carried by different channels, and
  // the slot never changes width.

  // Status colours come from the active Omarchy theme's own palette, resolved
  // once in the service. One function feeds the bar badge, every row glyph and
  // the hero, so a state cannot look like two different things depending on
  // where it is drawn — which is exactly what happened when the badge called
  // "running" amber while the panel rows called it accent-blue.
  readonly property var palette: engine ? engine.palette : ({})

  function statusColor(health) {
    var themed = Model.statusColor(palette, health)
    if (themed !== "") return themed
    // A theme with no green or amber still has these.
    if (health === "failing") return urgent
    if (health === "running") return accent
    if (health === "passing") return foreground
    return dim
  }

  // Empty is a state with nothing to report, so it gets no badge at all —
  // just the mark, exactly as an unstarted tab shows a bare favicon.
  readonly property bool badgeVisible: backendReady && connected && repoViews.length > 0

  readonly property string badgeGlyph: {
    if (!badgeVisible) return ""
    if (worst === "passing") return "\u{f00c}"   // check
    if (worst === "failing") return "\u{f00d}"   // cross
    return "\u{f111}"                            // filled dot
  }

  readonly property color badgeColor: statusColor(worst)

  // The mark itself stays the bar's own foreground and never takes a status
  // colour — a bar full of shouting icons is one nobody reads. The one
  // exception is the helper being gone, which is not a CI state at all.
  readonly property color markColor: backendReady ? foreground : urgent

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    fontFamily: root.fontFamily
    tooltipText: root.backendReady
      ? Model.tooltipFor({ repos: root.repoViews, auth: root.auth }, root.nowSeconds)
      : root.errorText
    onPressed: function(code) { root.toggle() }
    iconComponent: badgedMark
  }

  // Declared once and instantiated at two sizes: the bar slot and the panel
  // hero. Duplicating the badge geometry for the second one would guarantee
  // they drift apart.
  component BadgedMark: Item {
    id: mark
    property real markSize: Style.bar.iconFont

    implicitWidth: octocat.implicitWidth
    implicitHeight: octocat.implicitHeight

      Text {
        id: octocat
        anchors.centerIn: parent
        text: "\u{f09b}"
        color: root.markColor
        font.family: root.fontFamily
        font.pixelSize: mark.markSize
      }

      Item {
        id: badge
        visible: root.badgeGlyph !== ""
        // Just under half the mark, tucked into the corner with a little
        // overlap. Larger than this and it stops reading as a badge on the
        // mark and starts reading as a second icon beside it.
        readonly property real diameter: Math.round(mark.markSize * 0.46)
        width: diameter
        height: diameter

        // Positioned from the centre rather than from the Text's bounds: a
        // glyph's advance width includes side bearings, so anchoring to its
        // edge puts the badge somewhere slightly different in every font.
        //
        // The offsets are deliberately not rounded. The bar mark is 13px and
        // the hero mark is 24px, and rounding turned a 0.27 ratio into 0.308 at
        // 13px but 0.25 at 24px — a 20% difference between the two, which is
        // exactly the drift that made the two icons look like different
        // designs. Only the diameter is rounded, so the disc stays crisp.
        anchors.centerIn: parent
        anchors.horizontalCenterOffset: mark.markSize * 0.30
        anchors.verticalCenterOffset: mark.markSize * 0.27

        // Punched out of the mark in the bar's own background, so the badge
        // reads as sitting on top rather than colliding with the octocat's
        // silhouette.
        Rectangle {
          anchors.centerIn: parent
          width: badge.diameter * 1.45
          height: width
          radius: width / 2
          color: Color.bar.background
        }

        Text {
          anchors.centerIn: parent
          text: root.badgeGlyph
          color: root.badgeColor
          font.family: root.fontFamily
          font.pixelSize: Math.round(badge.diameter * 0.92)
        }
      }
  }

  Component {
    id: badgedMark
    BadgedMark { anchors.centerIn: parent; markSize: Style.bar.iconFont }
  }

  Component {
    id: heroMark
    BadgedMark { markSize: Style.font.display }
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
      // Only corrects the view if closing left it somewhere impossible.
      if (!connected && view !== "connect") resetToLanding()
      syncOpenState()
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    } else {
      syncOpenState()
      resetTransient()
      // Reset while nothing is on screen, so reopening is instant and shows
      // the top level rather than whichever subpage was last open.
      resetToLanding()
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
          // "Found" said nothing useful. Naming what was found confirms the
          // slug resolved to the repository the user meant, which is the whole
          // point of checking.
          var found = reply.data && reply.data.slug ? String(reply.data.slug) : ""
          var visibility = reply.data && reply.data.private ? "private" : "public"
          root.addMessage = found === "" ? "Ready to add" : found + " · " + visibility
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
  // Two lines in the overview, matching the workflow rows in the detail view.
  readonly property real overviewRowHeight: Style.space(42)

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

  // Move the row the cursor is on, and keep the cursor on it afterwards —
  // otherwise a second press moves whatever slid into its place.
  function shiftSelected(delta) {
    if (selectedIndex < 0 || selectedIndex >= repos.length) return
    var target = Math.max(0, Math.min(repos.length - 1, selectedIndex + delta))
    if (target === selectedIndex) return
    nudge(selectedIndex, delta)
    selectedIndex = target
    cursorActive = true
  }

  // ------------------------------------------------------------- keyboard

  function moveCursor(dx, dy) {
    cursorActive = true
    var count = view === "overview" ? repoViews.length
      : view === "settings" ? repos.length
      : (view === "detail" && detailRepo ? Model.asList(detailRepo.runs).length : 0)
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

  // Return to the landing view without animating.
  //
  // Reopening the panel after closing it on a subpage used to replay the slide,
  // because the reset assigned `view` and every assignment animated. The
  // transition exists to show the user which way *they* moved; a reset they did
  // not ask for, performed while the panel is invisible, is not that.
  //
  // Cheaper than any alternative: no extra Behavior, no state machine, and the
  // animation object is never started rather than started and cancelled.
  property bool navSuppressed: false

  function resetToLanding() {
    navSuppressed = true
    detailIndex = -1
    view = connected ? "overview" : "connect"
    navSuppressed = false
    navSlide.stop()
    content.x = 0
    content.opacity = 1.0
  }

  // Only animate a change the user made while looking at the panel.
  onViewChanged: if (opened && !navSuppressed) navSlide.restart()

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

      // Settings and refresh were reachable only by mouse, which made the
      // whole panel keyboard-navigable right up to the point where you wanted
      // to change something.
      onTextKey: function(text) {
        if (root.view === "settings") {
          // Reordering has to use keys the catcher passes through. It declares
          // `Keys.priority: Keys.BeforeItem` and consumes Up and Down without
          // consulting modifiers, so an Alt+Up binding is unreachable however
          // it is written; lowercase j/k are taken as cursor movement for the
          // same reason, leaving the shifted pair.
          if (text === "J") root.shiftSelected(1)
          else if (text === "K") root.shiftSelected(-1)
          return
        }
        if (root.view !== "overview") return
        if (text === ",") root.pushView("settings")
        else if (text === "r" && root.engine) root.engine.refresh(true)
      }

      // The settings screen was fully keyboard-navigable right up to the one
      // control you go there to use. Tab reaches the field; Escape hands focus
      // back so the arrow keys and Escape-to-leave keep working.
      onTabRequested: function(direction) {
        if (root.view === "settings") addField.forceActiveFocus()
        else if (root.view === "connect") tokenField.forceActiveFocus()
      }

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
          iconComponent: root.inSubpage ? backControl : heroMark
          trailingControl: root.view === "overview" ? overviewActions
            : root.view === "detail" ? detailActions : null
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
              height: root.overviewRowHeight

              readonly property bool hot: (root.cursorActive && root.selectedIndex === index)
                || rowMouse.containsMouse
              // The glyph carries the status; the text stays neutral unless the
              // build is actually broken. Colouring every row by state turned
              // a mostly-green list into a wall of colour that said nothing.
              readonly property color glyphColor: modelData.muted
                ? root.dim : root.statusColor(modelData.health)
              readonly property color textColor: modelData.muted ? root.dim
                : modelData.health === "failing" ? root.statusColor("failing")
                : root.foreground

              Rectangle {
                anchors.fill: parent
                radius: Style.cornerRadius
                color: overviewRow.hot ? Style.hoverFill : "transparent"
                Behavior on color { enabled: root.animate; ColorAnimation { duration: 120 } }
              }

              Row {
                id: nameRow
                anchors.left: parent.left
                anchors.leftMargin: Style.space(10)
                anchors.right: rowMeta.left
                anchors.rightMargin: Style.space(8)
                y: Style.space(6)
                spacing: Style.space(8)

                Text {
                  id: rowGlyph
                  anchors.verticalCenter: parent.verticalCenter
                  text: Model.glyphFor(overviewRow.modelData.health)
                  color: overviewRow.glyphColor
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.body
                }

                // Owner and name on one line, the owner dimmed. Two projects
                // called `core` in different orgs were previously
                // indistinguishable, and the org is the part you need to tell
                // them apart, not the part to hide.
                //
                // When the pair does not fit, the owner gives way first: the
                // repository name is what identifies the row, so eliding the
                // whole string left-to-right would cut exactly the wrong half.
                Row {
                  id: label
                  anchors.verticalCenter: parent.verticalCenter
                  spacing: 0
                  // The Row sits inside a parent Row, so it cannot anchor to
                  // the container's edges; it takes what the glyph leaves.
                  readonly property real available: overviewRow.width
                    - Style.space(10) - rowMeta.width - Style.space(8)
                    - rowGlyph.width - Style.space(8)

                  Text {
                    id: ownerText
                    text: Model.ownerPrefix(overviewRow.modelData.slug)
                    color: root.dim
                    elide: Text.ElideRight
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                    width: Math.max(0, Math.min(implicitWidth, label.available - nameText.width))
                  }
                  Text {
                    id: nameText
                    text: Model.rowTitle(overviewRow.modelData)
                    color: overviewRow.textColor
                    elide: Text.ElideRight
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                    width: Math.max(0, Math.min(implicitWidth, label.available))
                  }
                }
              }

              // Age plus a chevron. The chevron is what says "this row goes
              // somewhere" — without it the subpage is a surprise.
              Row {
                id: rowMeta
                anchors.right: parent.right
                anchors.rightMargin: Style.space(10)
                y: Style.space(6)
                spacing: Style.space(6)

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  textFormat: Text.PlainText
                  text: overviewRow.modelData.error !== "" ? "unreachable"
                    : Model.repoAge(overviewRow.modelData, root.nowSeconds)
                  color: root.dim
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                }

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  text: "\u{f054}"
                  color: overviewRow.hot ? overviewRow.textColor : root.dim
                  opacity: overviewRow.hot ? 1.0 : 0.55
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                  Behavior on opacity { enabled: root.animate; NumberAnimation { duration: 120 } }
                }
              }

              // Which workflow the row is about. The overview used to say only
              // that something was wrong; you had to open the subpage to find
              // out whether it was `deploy` or `lint`.
              Text {
                anchors.left: nameRow.left
                anchors.leftMargin: rowGlyph.width + Style.space(8)
                anchors.right: parent.right
                anchors.rightMargin: Style.space(10)
                y: Style.space(24)
                textFormat: Text.PlainText
                elide: Text.ElideRight
                text: overviewRow.modelData.error !== ""
                  ? overviewRow.modelData.error
                  : Model.repoSubtitle(overviewRow.modelData)
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
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
            // One block per workflow, covering both lines.
            //
            // This was a `Button` for the title with the subtitle loose beneath
            // it, which meant the highlight covered only the title's own bounds
            // and hovering the subtitle highlighted nothing — the row looked
            // like two unrelated things that happened to sit near each other.
            // One Item, one hover surface, one hit area.
            delegate: Item {
              id: runRow
              required property var modelData
              required property int index
              width: parent ? parent.width : 0
              height: Style.space(42)

              readonly property bool hot: (root.cursorActive && root.selectedIndex === index)
                || runMouse.containsMouse
              readonly property color glyphColor: root.statusColor(modelData.health)
              readonly property color textColor: modelData.health === "failing"
                ? root.statusColor("failing") : root.foreground

              Rectangle {
                anchors.fill: parent
                anchors.topMargin: Style.space(1)
                anchors.bottomMargin: Style.space(1)
                radius: Style.cornerRadius
                color: runRow.hot ? Style.hoverFill : "transparent"
                Behavior on color { enabled: root.animate; ColorAnimation { duration: 120 } }
              }

              Text {
                id: runGlyph
                x: Style.space(10)
                y: Style.space(7)
                text: Model.glyphFor(runRow.modelData.health)
                color: runRow.glyphColor
                font.family: root.fontFamily
                font.pixelSize: Style.font.body
              }

              // Workflow names are arbitrary strings from someone else's
              // repository — "Running Copilot Code Review" and worse. The run
              // number is pinned to the right of the name and never elided,
              // because a truncated "#101…" is a lie.
              Text {
                id: runTitle
                anchors.left: runGlyph.right
                anchors.leftMargin: Style.space(8)
                anchors.right: runNumber.left
                anchors.rightMargin: Style.space(6)
                y: Style.space(6)
                textFormat: Text.PlainText
                elide: Text.ElideRight
                text: runRow.modelData.workflow
                color: runRow.textColor
                font.family: root.fontFamily
                font.pixelSize: Style.font.body
              }

              Text {
                id: runNumber
                anchors.right: parent.right
                anchors.rightMargin: Style.space(10)
                y: Style.space(6)
                textFormat: Text.PlainText
                text: "#" + runRow.modelData.number
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.body
              }

              // Branch · author · duration, each truncated on its own terms.
              //
              // As one elided string the branch came first and ate everything:
              // a real branch like `megrogge/fix-omni-window-voice` pushed both
              // the author and the duration off the end. Widths are assigned
              // right to left — duration is never cut, the author gives way
              // next, and the branch absorbs whatever is left.
              Row {
                id: subtitle
                anchors.left: runTitle.left
                anchors.right: parent.right
                anchors.rightMargin: Style.space(10)
                y: Style.space(24)
                spacing: Style.space(5)

                readonly property real available: width
                readonly property var parts: Model.runParts(runRow.modelData, root.nowSeconds)

                Text {
                  id: branchText
                  visible: subtitle.parts.branch !== ""
                  text: subtitle.parts.branch
                  color: root.dim
                  elide: Text.ElideRight
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                  width: Math.max(0, Math.min(implicitWidth,
                    subtitle.available - actorGroup.width - durationGroup.width))
                }

                Row {
                  id: actorGroup
                  visible: subtitle.parts.actor !== ""
                  spacing: Style.space(5)
                  Text {
                    visible: branchText.visible
                    text: "·"
                    color: root.dim
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.caption
                  }
                  Text {
                    id: actorText
                    text: subtitle.parts.actor
                    color: root.dim
                    elide: Text.ElideRight
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.caption
                    width: Math.max(0, Math.min(implicitWidth,
                      subtitle.available - durationGroup.width))
                  }
                }

                Row {
                  id: durationGroup
                  visible: subtitle.parts.duration !== ""
                  spacing: Style.space(5)
                  Text {
                    visible: branchText.visible || actorGroup.visible
                    text: "·"
                    color: root.dim
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.caption
                  }
                  // No elide and no width cap: a truncated duration is a wrong
                  // duration, and it is the cheapest thing on the line.
                  Text {
                    text: subtitle.parts.duration
                    color: root.dim
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.caption
                  }
                }
              }

              MouseArea {
                id: runMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onEntered: { root.cursorActive = true; root.selectedIndex = runRow.index }
                onClicked: if (root.engine) root.engine.openRun(runRow.modelData.url)
              }

              PanelToolTip {
                visible: runMouse.containsMouse && runRow.modelData.message !== ""
                text: runRow.modelData.message
                fontFamily: root.fontFamily
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
                ? ("Connected as " + root.auth.login + " · " + (root.auth.sourceLabel || ""))
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

          Text {
            visible: root.repos.length > 1
            width: parent.width
            textFormat: Text.PlainText
            text: "Drag the handle to reorder, or J / K to move the selected row"
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            bottomPadding: Style.space(2)
          }

          // Reorderable list. Dragging moves a row; J and K do the same from
          // the keyboard, and both go through Model.moveItem so they cannot
          // disagree about the result.
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

                readonly property bool cursored: root.cursorActive
                  && root.selectedIndex === index && root.dragIndex < 0

                Rectangle {
                  anchors.fill: parent
                  radius: Style.cornerRadius
                  color: root.dragIndex === index || repoRow.cursored
                    ? Style.hoverFill : "transparent"
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
              Keys.onEscapePressed: keyCatcher.forceActiveFocus()
            }

            // The realtime verdict. Colour carries severity; the glyph carries
            // the same information again for anyone who cannot rely on it.
            //
            // Every glyph here is one already proven to render in this bar —
            // check, cross, spinner, warning. Reaching for a nicer-looking
            // codepoint risks a missing one, and a tofu box in a validation
            // message is worse than a plain tick.
            Row {
              width: parent.width
              spacing: Style.space(6)
              visible: root.addMessage !== ""

              Text {
                text: root.addState === "ok" ? "\u{f00c}"
                  : root.addState === "checking" ? "\u{f021}"
                  : root.addState === "duplicate" ? "\u{f071}"
                  : "\u{f00d}"
                color: root.addState === "ok" ? root.statusColor("passing")
                  : (root.addState === "checking" || root.addState === "duplicate") ? root.dim
                  : root.statusColor("failing")
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption

                // Spin an intermediate property and bind `rotation` through a
                // conditional, so a check or a cross is never left sitting at
                // whatever angle the spinner stopped on.
                property real spinAngle: 0
                rotation: root.addState === "checking" ? spinAngle : 0

                NumberAnimation on spinAngle {
                  running: root.addState === "checking" && root.animate
                  loops: Animation.Infinite
                  from: 0; to: 360; duration: 1100
                }
              }

              Text {
                width: parent.width - Style.space(20)
                textFormat: Text.PlainText
                elide: Text.ElideRight
                text: root.addMessage
                color: root.addState === "ok" ? root.foreground : root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
            }

            // Inert until the repository has actually been confirmed to exist.
            // `enabled` alone stops the click, but a permanent border still
            // reads as a live control, so the box only appears once pressing it
            // would do something.
            Button {
              width: parent.width
              text: "Add project"
              bordered: root.addState === "ok"
              enabled: root.addState === "ok"
              opacity: root.addState === "ok" ? 1.0 : 0.45
              foreground: root.addState === "ok" ? root.foreground : root.dim
              fontFamily: root.fontFamily
              onClicked: root.commitAdd()
              Behavior on opacity { enabled: root.animate; NumberAnimation { duration: 140 } }
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
            Keys.onEscapePressed: keyCatcher.forceActiveFocus()
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
      return (owner === "" ? "" : owner + " · ") + when
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
