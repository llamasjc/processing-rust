package processing.mode.rust;

import processing.app.Base;
import processing.app.Formatter;
import processing.app.Preferences;
import processing.app.SketchCode;
import processing.app.syntax.JEditTextArea;
import processing.app.syntax.PdeTextArea;
import processing.app.syntax.PdeTextAreaDefaults;
import processing.app.syntax.SyntaxDocument;
import processing.app.ui.Editor;
import processing.app.ui.EditorException;
import processing.app.ui.EditorState;
import processing.app.ui.EditorToolbar;

import javax.swing.JMenu;
import javax.swing.JMenuItem;
import javax.swing.KeyStroke;
import javax.swing.SwingUtilities;
import java.awt.event.InputEvent;
import java.awt.event.KeyEvent;
import java.io.BufferedReader;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.IdentityHashMap;
import java.util.Map;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * The sketch editor for RustMode: Run builds the sketch with cargo and starts
 * it, Stop ends whichever of the two is going on.
 *
 * <p>There is at most one run per editor. Starting a new one stops the old
 * one first, including a build that is still compiling.
 */
public class RustEditor extends Editor {

  private static final Logger LOG = Logger.getLogger(RustEditor.class.getName());

  /** One press of Run: the build, then the sketch process. */
  private static final class Run {
    volatile RustBuild build;
    volatile Process   proc;
    volatile Thread    thread;
    volatile boolean   stopped;
  }

  private volatile Run current;

  // Sketch processes that are still alive, across all editors. Emptied by a
  // shutdown hook so quitting the IDE never leaves sketch windows behind.
  private static final Set<Process> LIVE = ConcurrentHashMap.newKeySet();
  static {
    try {
      Runtime.getRuntime().addShutdownHook(new Thread(() -> {
        for (Process p : LIVE) p.destroyForcibly();
      }, "rustmode-stop-sketches"));
    } catch (Throwable ignored) {}   // already shutting down
  }

  // One token marker per tab: each tab has its own document, and a marker
  // holds the colors for exactly one text.
  private final Map<SyntaxDocument, TsRustTokenMarker> markers = new IdentityHashMap<>();
  private boolean ready;   // false while Editor's constructor is still running

  protected RustEditor(Base base, String path, EditorState state, RustMode mode)
      throws EditorException {
    super(base, path, state, mode);
    ready = true;
    try {
      attachTokenMarker();
    } catch (Throwable t) {
      LOG.log(Level.WARNING, "couldn't set up highlighting", t);
    }
  }

  // ── Syntax colors ─────────────────────────────────────────────────────────────

  /**
   * Editor calls this whenever another tab is shown (and while a sketch is
   * loading). The tab's document may be brand new, so this is the place to
   * give it a marker.
   */
  @Override
  public void setCode(SketchCode code) {
    super.setCode(code);
    if (ready) {
      try {
        attachTokenMarker();
      } catch (Throwable t) {
        LOG.log(Level.WARNING, "couldn't set up highlighting for this tab", t);
      }
    }
  }

  /** Makes sure the document now in the text area has a Rust token marker. */
  private void attachTokenMarker() {
    // Without the native library, keep Processing's keyword-based colors.
    if (!RustMode.isNativeLoaded()) return;
    // ...and the same if the query file didn't make it into the jar
    if (TsRustTokenMarker.class.getResource("/queries/rust-highlights.scm") == null) return;
    SyntaxDocument doc = getTextArea().getDocument();
    if (doc == null || markers.containsKey(doc)) return;
    // a query that doesn't load would mean no colors at all: keep the default ones
    if (!TsRustTokenMarker.available()) return;

    TsRustTokenMarker marker = new TsRustTokenMarker();
    // highlights are rebuilt off the event thread; repaint once they're ready
    marker.setRepaintHook(() -> SwingUtilities.invokeLater(() -> {
      if (getTextArea().getDocument() == doc) getTextArea().getPainter().repaint();
    }));
    doc.setTokenMarker(marker);
    doc.addDocumentListener(new javax.swing.event.DocumentListener() {
      public void insertUpdate(javax.swing.event.DocumentEvent e)  { marker.invalidate(doc); }
      public void removeUpdate(javax.swing.event.DocumentEvent e)  { marker.invalidate(doc); }
      public void changedUpdate(javax.swing.event.DocumentEvent e) { marker.invalidate(doc); }
    });
    markers.put(doc, marker);
    marker.invalidate(doc);

    // drop the markers of tabs that no longer exist
    if (markers.size() > sketch.getCodeCount()) {
      java.util.Set<SyntaxDocument> open = Collections.newSetFromMap(new IdentityHashMap<>());
      for (SketchCode c : sketch.getCode()) {
        Object d = c.getDocument();
        if (d instanceof SyntaxDocument) open.add((SyntaxDocument) d);
      }
      markers.entrySet().removeIf(e -> {
        if (open.contains(e.getKey())) return false;
        e.getValue().dispose();
        return true;
      });
    }
  }

  public RustMode getRustMode() {
    return (RustMode) getMode();
  }

  // ── Text area ─────────────────────────────────────────────────────────────────
  //
  // Editor's default is a bare JEditTextArea with no gutter, so no line
  // numbers. Same text area as Java mode instead, with Rust typing help on
  // top. Called from Editor's constructor, before our own fields are set.

  @Override
  protected JEditTextArea createTextArea() {
    return new PdeTextArea(new PdeTextAreaDefaults(getMode()), new RustInputHandler(this), this);
  }

  // ── Run ───────────────────────────────────────────────────────────────────────

  /** Run (present = false) or Present (present = true). Call on the event thread. */
  public void startRun(boolean present) {
    try {
      // Copies every tab's live editor text into the sketch (and clears the
      // console if that preference is on), so Run uses what is on screen,
      // saved or not, in all tabs.
      prepareRun();
      stopCurrent();
      statusNotice("Building with cargo...");

      Run run = new Run();
      java.awt.Rectangle display = screenBounds();
      java.awt.Rectangle screen = present ? display : null;
      Thread t = new Thread(() -> doRun(run, screen, display), "RustMode-run");
      t.setDaemon(true);
      run.thread = t;
      current = run;
      try { getToolbar().activateRun(); } catch (Throwable ignored) {}
      t.start();
    } catch (Throwable t) {
      // whatever went wrong, the Run button has to keep working
      LOG.log(Level.WARNING, "could not start the sketch", t);
      current = null;
      deactivateRun();
      statusError("Could not start the sketch: " + t);
    }
  }

  /** Runs on the run's own thread. */
  private void doRun(Run run, java.awt.Rectangle presentOn, java.awt.Rectangle display) {
    try {
      // Quick checks with tree-sitter before cargo gets involved.
      String source = String.join("\n",
        Arrays.stream(sketch.getCode()).map(SketchCode::getProgram).toArray(String[]::new));
      RustAnalysis analysis = new RustAnalyzer(source, sketch.getName()).analyse();
      List<RustLinter.LintProblem> lint = new RustLinter(sketch, analysis).lint();
      if (run.stopped) return;
      if (!lint.isEmpty()) {
        RustLinter.LintProblem first = lint.get(0);
        for (RustLinter.LintProblem p : lint) {
          System.err.println(tabName(p.tabIndex()) + ":" + p.line() + ": " + p.message());
        }
        showProblem(run, first.tabIndex(), first.line() - 1,
          tabName(first.tabIndex()) + ":" + first.line() + ": " + first.message());
        return;
      }

      RustBuild build = new RustBuild(sketch, getRustMode(), Preferences.getBoolean("build.release"));
      run.build = build;
      if (run.stopped) return;
      File binary = build.build();
      if (run.stopped) return;

      statusNotice("Running " + sketch.getName() + "...");
      int exit = runBinary(run, binary, presentOn, display);
      if (run.stopped) return;
      if (exit == 0) {
        statusNotice("Done.");
      } else {
        statusError("The sketch stopped with an error (exit status " + exit + "). See the console.");
      }

    } catch (InterruptedException e) {
      // Stop was pressed, or a new Run replaced this one
    } catch (CargoException e) {
      if (run.stopped) return;
      String msg = e.getMessage() != null ? e.getMessage() : "cargo build failed";
      RustProblem where = e.diagnostics().stream()
        .filter(d -> d.getSeverity() == RustProblem.ERROR).findFirst().orElse(null);
      if (where != null) showProblem(run, where.getTabIndex(), where.getLineNumber(), msg);
      else statusError(msg);
    } catch (Throwable e) {
      if (run.stopped) return;
      LOG.log(Level.WARNING, "run failed", e);
      statusError(e.getMessage() != null ? e.getMessage() : e.toString());
    } finally {
      SwingUtilities.invokeLater(() -> {
        if (current == run) {
          current = null;
          deactivateRun();
        }
      });
    }
  }

  private String tabName(int tab) {
    return tab >= 0 && tab < sketch.getCodeCount() ? sketch.getCode(tab).getFileName() : "sketch";
  }

  /** Shows an error and goes to its tab and line (0-based). Safe from any thread. */
  private void showProblem(Run run, int tab, int line, String message) {
    SwingUtilities.invokeLater(() -> {
      if (run.stopped) return;
      try {
        if (tab >= 0 && tab < sketch.getCodeCount()) {
          if (sketch.getCurrentCodeIndex() != tab) sketch.setCurrentCode(tab);
          JEditTextArea ta = getTextArea();
          int ln = Math.min(Math.max(line, 0), ta.getLineCount() - 1);
          if (ln >= 0) {
            int start = ta.getLineStartOffset(ln);
            int stop  = Math.max(start, ta.getLineStopOffset(ln) - 1);
            ta.select(start, stop);
          }
        }
      } catch (Throwable t) {
        LOG.log(Level.FINE, "couldn't highlight the error line", t);
      }
      statusError(message);
    });
  }

  /** Starts the sketch and waits for it. Returns its exit status. */
  private int runBinary(Run run, File binary, java.awt.Rectangle presentOn, java.awt.Rectangle display)
      throws IOException, InterruptedException {
    ProcessBuilder pb = new ProcessBuilder(binary.getAbsolutePath());
    File dir = sketch.getFolder();
    if (dir != null && dir.isDirectory()) pb.directory(dir);
    // The sketch's stdin stays open and unused: if the IDE dies without a
    // chance to clean up, the pipe closes and the sketch closes with it.
    pb.environment().put("PROCESSING_WATCH_STDIN", "1");
    FfmpegSetup.prepare(pb.environment());
    // the GLFW 3.4 the mode ships, for libprocessing (see RustBuild.glfwDir)
    File glfw = RustBuild.glfwDir(getRustMode().getFolder());
    if (glfw != null) {
      String var = isWindowsOS() ? "PATH" : System.getProperty("os.name", "").toLowerCase().contains("mac") ? "DYLD_LIBRARY_PATH" : "LD_LIBRARY_PATH";
      String old = pb.environment().get(var);
      pb.environment().put(var, glfw.getAbsolutePath() + (old == null || old.isEmpty() ? "" : File.pathSeparator + old));
    }
    // the sketch was built for this renderer (RustBuild adds the feature)
    if (Renderer.libprocessing()) pb.environment().put("PROCESSING_RENDERER", "libprocessing");
    else pb.environment().remove("PROCESSING_RENDERER");
    // hour(), minute(), day()... in local time
    pb.environment().put("PROCESSING_UTC_OFFSET",
      String.valueOf(java.util.TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 1000));
    // the screen this editor is on: full_screen(), display_width() and display_height() need it
    if (display != null && display.width > 0 && display.height > 0) {
      pb.environment().put("PROCESSING_DISPLAY",
        display.width + "," + display.height + "," + display.x + "," + display.y);
    } else {
      pb.environment().remove("PROCESSING_DISPLAY");
    }
    if (presentOn != null && presentOn.width > 0 && presentOn.height > 0) {
      pb.environment().put("PROCESSING_PRESENT",
        presentOn.width + "," + presentOn.height + "," + presentOn.x + "," + presentOn.y);
    } else {
      pb.environment().remove("PROCESSING_PRESENT");
    }

    Process p = pb.start();
    run.proc = p;
    LIVE.add(p);
    if (run.stopped) kill(p);   // Stop arrived while the sketch was starting

    // println!() to the console in black, panics and eprintln!() in red.
    // Separate threads so a full pipe on one stream can't block the other.
    // A panic says "panicked at src/main.rs:26:21"; show the sketch tab and line.
    RustBuild build = run.build;
    Thread out = pump(p.getInputStream(), System.out, "RustMode-stdout", null);
    Thread err = pump(p.getErrorStream(), System.err, "RustMode-stderr", build);
    try {
      int exit = p.waitFor();
      out.join(1000);
      err.join(1000);
      return exit;
    } catch (InterruptedException e) {
      kill(p);
      throw e;
    } finally {
      if (!p.isAlive()) LIVE.remove(p);
    }
  }

  private static Thread pump(InputStream in, PrintStream out, String name, RustBuild map) {
    Thread t = new Thread(() -> {
      try (BufferedReader r = new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8))) {
        String line;
        while ((line = r.readLine()) != null) {
          if (map != null) {
            try { line = map.rewriteLocations(line); } catch (RuntimeException ignored) {}
          }
          out.println(line);
        }
      } catch (IOException ignored) {
      }
    }, name);
    t.setDaemon(true);
    t.start();
    return t;
  }

  private static void kill(Process p) {
    p.destroy();
    try {
      if (!p.waitFor(1, TimeUnit.SECONDS)) p.destroyForcibly();
    } catch (InterruptedException e) {
      p.destroyForcibly();
      Thread.currentThread().interrupt();
    }
    LIVE.remove(p);
  }

  // ── Stop ──────────────────────────────────────────────────────────────────────

  public void handleStop() {
    boolean wasRunning = current != null;
    stopCurrent();
    deactivateRun();
    if (wasRunning) statusNotice("Stopped."); else statusEmpty();
  }

  /** Ends the current run, whether it is still building or already on screen. */
  private void stopCurrent() {
    Run run = current;
    current = null;
    if (run == null) return;
    run.stopped = true;
    RustBuild b = run.build;
    if (b != null) b.cancel();
    Process p = run.proc;
    if (p != null) {
      // off the event thread: waiting up to a second for the sketch to
      // close must not freeze the editor
      Thread k = new Thread(() -> kill(p), "RustMode-stop");
      k.setDaemon(true);
      k.start();
    }
    Thread t = run.thread;
    if (t != null) t.interrupt();
  }

  @Override
  public void internalCloseRunner() {
    stopCurrent();
  }

  /** The Run button goes back to normal. Stopping the sketch is stopCurrent's job. */
  @Override
  public void deactivateRun() {
    try { getToolbar().deactivateRun(); } catch (Throwable ignored) {}
  }

  @Override
  public void dispose() {
    stopCurrent();
    for (TsRustTokenMarker m : markers.values()) m.dispose();
    markers.clear();
    super.dispose();
  }

  /** The screen this editor window is on (present mode covers that one). */
  private java.awt.Rectangle screenBounds() {
    try {
      java.awt.GraphicsConfiguration gc = getGraphicsConfiguration();
      if (gc != null) return gc.getBounds();
    } catch (Throwable ignored) {}
    try {
      java.awt.Dimension d = java.awt.Toolkit.getDefaultToolkit().getScreenSize();
      return new java.awt.Rectangle(0, 0, d.width, d.height);
    } catch (Throwable t) {
      return null;   // no screen info: runs in a normal window
    }
  }

  // ── Cargo output ──────────────────────────────────────────────────────────────

  /** A line of cargo output for the console; errors and warnings in red. */
  public void statusMessage(String msg) {
    if (msg.startsWith("error") || msg.startsWith("warning")) {
      System.err.println(msg);
    } else if (!msg.isBlank()) {
      System.out.println(msg);
    }
  }

  // ── Editor plumbing ───────────────────────────────────────────────────────────

  @Override
  public String getCommentPrefix() {
    return "//";
  }

  @Override
  public EditorToolbar createToolbar() {
    return new EditorToolbar(this) {
      // mode 0 = Run, 1 = Present
      @Override public void handleRun(int mode) { RustEditor.this.startRun(mode == 1); }
      @Override public void handleStop()        { RustEditor.this.handleStop(); }
      // the renderer menu, left of the mode selector
      @Override public void addModeButtons(javax.swing.Box box, javax.swing.JLabel label) {
        box.add(Renderer.menu());
        addGap(box);
      }
    };
  }

  @Override
  public Formatter createFormatter() {
    return null;   // rustfmt could go here later
  }

  @Override
  public JMenu buildFileMenu() {
    // like Processing's: Ctrl+Shift+E
    int menuKey = java.awt.Toolkit.getDefaultToolkit().getMenuShortcutKeyMaskEx();
    JMenuItem app = new JMenuItem("Export Application...");
    app.setAccelerator(KeyStroke.getKeyStroke(KeyEvent.VK_E, menuKey | InputEvent.SHIFT_DOWN_MASK));
    app.addActionListener(e -> handleExportApplication());
    JMenuItem pdez = new JMenuItem("Export as PDEZ...");
    pdez.addActionListener(e -> handleExportPdez());
    return buildFileMenu(new JMenuItem[] { app, pdez });
  }

  /** File > Export as PDEZ: the sketch folder zipped next to it (see AppExport.pdez). */
  private void handleExportPdez() {
    if (sketch.isUntitled() || sketch.isReadOnly()) {
      statusError("Save the sketch first.");
      return;
    }
    if (sketch.isModified()) {
      try { sketch.save(); } catch (Exception ex) { statusError("Couldn't save: " + ex.getMessage()); return; }
    }
    try {
      File pdez = AppExport.pdez(sketch.getFolder());
      statusNotice("Exported " + pdez.getName());
      AppExport.show(pdez.getParentFile());
    } catch (Exception ex) {
      LOG.log(Level.WARNING, "PDEZ export failed", ex);
      statusError("Export failed: " + ex.getMessage());
    }
  }

  /**
   * File > Export Application: an optimized build of the sketch, in a folder
   * named for this platform inside the sketch folder, with its data folder
   * and a launcher named after the sketch (see AppExport). Built for the
   * renderer picked in the toolbar.
   */
  private void handleExportApplication() {
    if (sketch.isUntitled() || sketch.isReadOnly()) {
      statusError("Save the sketch first: the export goes inside its folder.");
      return;
    }
    if (sketch.isModified()) {
      try { sketch.save(); } catch (Exception ex) { statusError("Couldn't save: " + ex.getMessage()); return; }
    }
    // Rust sketches are compiled for the computer they're exported on, so
    // the other platforms are shown but off (like Java mode without a JDK)
    ExportDialog.Target here = ExportDialog.Target.here();
    ExportDialog.Choice choice = ExportDialog.show(this, sketch.getFolder(),
      new String[] { Renderer.SOFTWARE, Renderer.LIBPROCESSING },
      Renderer.libprocessing() ? Renderer.LIBPROCESSING : Renderer.SOFTWARE,
      (t, r) -> CrossTarget.why(t, Renderer.LIBPROCESSING.equals(r)));
    if (choice == null) return;
    boolean lp = Renderer.LIBPROCESSING.equals(choice.renderer);
    statusNotice("Exporting " + sketch.getName() + " (an optimized build, this can take a while)...");
    Thread t = new Thread(() -> {
      // the build reads the renderer from the toolbar setting: use the
      // dialog's for this export, then put the toolbar's back
      boolean toolbarLp = Renderer.libprocessing();
      Renderer.set(lp ? Renderer.LIBPROCESSING : Renderer.SOFTWARE);
      AppExport.icon = choice.icon;
      try {
        choice.outDir.mkdirs();
        List<String> made = new ArrayList<>();
        for (ExportDialog.Target target : choice.targets) {
          statusNotice("Exporting for " + target.label + " (an optimized build, this can take a while)...");
          File binary = new RustBuild(sketch, getRustMode(), true).crossTo(target).build();
          if (binary == null || !binary.isFile()) { statusError("Export failed: the build didn't produce a program"); return; }
          File dest = AppExport.write(choice.outDir, sketch.getName(), new File(sketch.getFolder(), "data"),
            target, binary, new String[0], lp ? "libprocessing" : null,
            lp ? AppExport.glfwLibs(RustBuild.glfwDir(getRustMode().getFolder())) : null);
          made.add(dest.getName());
          if (choice.appImage && (target == ExportDialog.Target.LINUX_X64 || target == ExportDialog.Target.LINUX_ARM64)) {
            String safe = sketch.getName().replaceAll("[^A-Za-z0-9_.-]", "_");
            File img = AppImage.make(getRustMode().getFolder(), dest, sketch.getName(), safe,
              target == ExportDialog.Target.LINUX_X64 ? "x86_64" : "aarch64", choice.icon, this::statusNotice);
            made.add(img.getName());
          }
        }
        statusNotice("Exported to " + choice.outDir.getName() + "/: " + String.join(", ", made));
        AppExport.show(choice.outDir);
      } catch (CargoException e) {
        statusError(e.getMessage() != null ? e.getMessage() : "the build failed");
      } catch (Throwable e) {
        LOG.log(Level.WARNING, "export failed", e);
        statusError("Export failed: " + (e.getMessage() != null ? e.getMessage() : e.toString()));
      } finally {
        AppExport.icon = null;
        Renderer.set(toolbarLp ? Renderer.LIBPROCESSING : Renderer.SOFTWARE);
      }
    }, "rustmode-export");
    t.setDaemon(true);
    t.start();
  }

  @Override
  public JMenu buildSketchMenu() {
    int menuKey = java.awt.Toolkit.getDefaultToolkit().getMenuShortcutKeyMaskEx();
    JMenuItem run = new JMenuItem("Run");
    run.setAccelerator(KeyStroke.getKeyStroke(KeyEvent.VK_R, menuKey));
    run.addActionListener(e -> getToolbar().handleRun(0));
    // fills the screen this window is on, sketch in the middle; Esc closes it
    JMenuItem present = new JMenuItem("Present");
    present.setAccelerator(KeyStroke.getKeyStroke(KeyEvent.VK_R, menuKey | InputEvent.SHIFT_DOWN_MASK));
    present.addActionListener(e -> getToolbar().handleRun(1));
    JMenuItem stop = new JMenuItem("Stop");
    stop.addActionListener(e -> handleStop());
    try {
      // sets Editor's private sketchMenu field, which windowActivated needs
      return buildSketchMenu(new JMenuItem[] { run, present, stop });
    } catch (Throwable t) {
      LOG.log(Level.WARNING, "Sketch menu fallback", t);
      JMenu menu = new JMenu("Sketch");
      menu.add(run);
      menu.add(present);
      menu.add(stop);
      return menu;
    }
  }

  @Override
  public void handleImportLibrary(String jarPath) {
    // Rust sketches don't use Processing libraries
  }

  @Override
  public JMenu buildHelpMenu() {
    JMenu help = new JMenu("Help");
    JMenuItem site = new JMenuItem("Rust Mode on GitHub");
    site.addActionListener(e -> openUrl("https://github.com/llamasjc/processing-rust"));
    help.add(site);
    JMenuItem book = new JMenuItem("The Rust Book");
    book.addActionListener(e -> openUrl("https://doc.rust-lang.org/book/"));
    help.add(book);
    JMenuItem std = new JMenuItem("Rust Standard Library");
    std.addActionListener(e -> openUrl("https://doc.rust-lang.org/std/"));
    help.add(std);
    return help;
  }

  private static void openUrl(String url) {
    Thread t = new Thread(() -> {
      try {
        if (System.getProperty("os.name", "").toLowerCase().contains("linux")) {
          new ProcessBuilder("xdg-open", url).start();
        } else {
          java.awt.Desktop.getDesktop().browse(new java.net.URI(url));
        }
      } catch (Exception ex) {
        System.err.println("[RustMode] couldn't open " + url + ": " + ex);
      }
    }, "rustmode-open-url");
    t.setDaemon(true);
    t.start();
  }

  private static boolean isWindowsOS() { return System.getProperty("os.name", "").toLowerCase().contains("win"); }
}
