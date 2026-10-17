# archiveview

archiveview plots EPICS PVs against time. History comes from an
[EPICS Archiver Appliance](https://github.com/archiver-appliance/epicsarchiverap)
and live values come from Channel Access or PV Access, joined into one line
per PV. It is written in Rust with [egui](https://github.com/emilk/egui),
in the style of [bobview](https://github.com/gilesknap/bobview).

It covers what most people use the Phoebus Data Browser for, with a much
smaller interface. The plot fills the window, a slim bar holds search and
the time range, and everything else is direct manipulation: drag to pan,
scroll to zoom, hover to read values. See
[Scope, and how it differs from the Data Browser](explanations/scope.md).

<div class="compass">
  <div class="axis axis-top-left">Learning: acquiring skills</div>
  <div class="axis axis-top-right">Working: applying skills</div>
  <div class="axis axis-left-top">Practical: doing</div>
  <div class="axis axis-left-bottom">Theoretical: knowing</div>
  <a class="grid-card tutorials" href="tutorials/quickstart.html">
    <span class="card-title">📚 Tutorials</span>
    <span class="orientation">learning-oriented</span>
    <p>New to archiveview? Install it, start the mock archiver and a
    simulated beamline, and plot your first PVs.</p>
  </a>
  <a class="grid-card how-to" href="how-to/install.html">
    <span class="card-title">🧭 How-to guides</span>
    <span class="orientation">goal-oriented</span>
    <p>Practical steps: connect to your archiver and IOCs, find PVs, save
    views, take snapshots, embed the plot.</p>
  </a>
  <a class="grid-card explanations" href="explanations/scope.html">
    <span class="card-title">💡 Explanations</span>
    <span class="orientation">understanding-oriented</span>
    <p>Background and design: what archiveview sets out to do and how
    archive and live data become one plot.</p>
  </a>
  <a class="grid-card reference" href="reference/cli.html">
    <span class="card-title">📖 Reference</span>
    <span class="orientation">information-oriented</span>
    <p>Command-line options, gestures and keys, config files, the archiver
    API, the mock archiver and the Rust API.</p>
  </a>
</div>

The [Rust API docs](reference/api.md) for the workspace's crates are
published alongside this book.

The docs follow the [Diátaxis](https://diataxis.fr) framework: four kinds of
page for four needs, learning, doing, understanding and looking things up.
