package processing.mode.rust;

import processing.app.Base;
import processing.app.Sketch;
import processing.app.SketchCode;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * RustBuild – drives the cargo build pipeline for a RustMode sketch.
 *
 * <p>Pipeline (mirrors CppBuild):
 * <ol>
 *   <li>Collect sketch tabs → concatenated source string.</li>
 *   <li>Parse with Tree-sitter via {@link RustAnalyzer}.</li>
 *   <li>Transform to a valid Rust binary via {@link RustEmitter}.</li>
 *   <li>Initialise / update the persistent cargo project in the build cache.</li>
 *   <li>Write the emitted source to {@code src/main.rs}.</li>
 *   <li>Run {@code cargo build [--release]}.</li>
 *   <li>Rewrite any rustc error messages back to sketch line numbers and
 *       strip fully-qualified crate paths before surfacing them in the IDE.</li>
 * </ol>
 *
 * <p>The cargo project is kept in
 * {@code ~/.processing/rust-build-cache/<sketch-name>/} so that
 * {@code target/} is preserved between runs, giving warm incremental builds.
 */
public class RustBuild {

  private static final java.util.logging.Logger LOG =
    java.util.logging.Logger.getLogger(RustBuild.class.getName());

  // ── Constants ───────────────────────────────────────────────────────────────

  /** Version stamp written to {@code .rustmode-version} in the cache project.
   *  Bump this to force a full project re-scaffold when the template changes. */
  private static final String RUSTMODE_VERSION = "0.1.0";

  /** Matches rustc error lines, e.g.
   *  {@code error[E0308]: --> src/main.rs:42:5} or the short form
   *  {@code src/main.rs:42:5: error: …}
   */
  private static final Pattern ERR_LINE_PATTERN =
    Pattern.compile("(-->\\s+)?src[/\\\\]main\\.rs:(\\d+)(?::(\\d+))?");

  /** Matches the processing:: prefix that sketches should never see. */
  private static final Pattern CRATE_PREFIX_PATTERN =
    Pattern.compile("\\bprocessing::"); // e.g. processing::App

  // ── Fields ──────────────────────────────────────────────────────────────────

  private final Sketch sketch;
  private final RustMode mode;
  private final boolean release;

  /** Number of lines injected before the user's code in main.rs.
   *  Used to map rustc line numbers back to sketch lines. */
  private int headerLineCount = 0;

  /** Per-tab start lines in the concatenated source (1-based). */
  private int[] tabStartLines;

  /** {first main.rs line, first sketch line, line count} per chunk; see RustEmitter. */
  private List<int[]> lineMap = List.of();

  // cargo while it is running, so Stop can end a build that is still going
  private volatile Process cargoProc;
  private volatile boolean cancelled;

  /** Stops a build in progress. Safe from any thread, any number of times. */
  public void cancel() {
    cancelled = true;
    Process p = cargoProc;
    if (p != null) {
      // cargo starts rustc as a child; take those down too
      p.descendants().forEach(ProcessHandle::destroyForcibly);
      p.destroyForcibly();
    }
  }

  /** Shared cargo target dir: dependencies are compiled once for all sketches. */
  private static File sharedTarget() {
    return new File(Compat.sketchbookFolder(null), "rust-build-cache/.shared-target");
  }

  // ── Construction ────────────────────────────────────────────────────────────

  public RustBuild(Sketch sketch, RustMode mode, boolean release) {
    this.sketch  = sketch;
    this.mode    = mode;
    this.release = release;
  }

  // ── Public entry-point ──────────────────────────────────────────────────────

  /**
   * Run the full build pipeline.
   *
   * @return path to the compiled binary, or {@code null} on failure.
   */
  public File build() throws IOException, InterruptedException, java.io.UnsupportedEncodingException {
    // 1 ── Concatenate sketch tabs ─────────────────────────────────────────────
    String sketchSource = collectTabs();

    // 2 ── Parse + analyse ────────────────────────────────────────────────────
    RustAnalyzer analyzer = new RustAnalyzer(sketchSource, sketch.getName());
    RustAnalysis  analysis = analyzer.analyse();

    // 3 ── Emit main.rs ───────────────────────────────────────────────────────
    RustEmitter emitter = new RustEmitter(analysis);
    String      mainRs  = emitter.emit();
    headerLineCount = emitter.getHeaderLineCount();
    lineMap         = emitter.getLineMap();

    // 4 ── Ensure persistent cargo project exists ──────────────────────────────
    File cacheProject = ensureCacheProject();

    // 5 ── Write src/main.rs ──────────────────────────────────────────────────
    File mainRsFile = new File(cacheProject, "src/main.rs");
    writeFile(mainRsFile, mainRs);

    // 6 ── cargo build ────────────────────────────────────────────────────────
    return runCargo(cacheProject);
  }

  // ── Step 1 – Collect tabs ───────────────────────────────────────────────────

  /**
   * Concatenate all sketch tabs into a single source string, tracking the
   * start line of each tab so we can map errors back to the right file.
   */
  private String collectTabs() {
    SketchCode[] codes = sketch.getCode();
    tabStartLines = new int[codes.length];
    StringBuilder sb = new StringBuilder();
    int lineCount = 1;

    for (int i = 0; i < codes.length; i++) {
      tabStartLines[i] = lineCount;
      String src = codes[i].getProgram();
      if (src == null) src = "";
      // "\r\n" and lone "\r" line ends would throw the line numbers off
      src = src.replace("\r\n", "\n").replace('\r', '\n');
      sb.append(src);
      if (!src.endsWith("\n")) sb.append('\n');
      // Count lines in this tab
      for (int c = 0; c < src.length(); c++) {
        if (src.charAt(c) == '\n') lineCount++;
      }
    }
    return sb.toString();
  }

  // ── Step 4 – Cache project ──────────────────────────────────────────────────

  /**
   * Return (creating if necessary) the persistent cargo project directory.
   *
   * <p>Layout:
   * <pre>
   * ~/.processing/rust-build-cache/&lt;sketch-name&gt;/
   *   .rustmode-version        ← recreated if stale
   *   Cargo.toml
   *   Cargo.lock               ← preserved (avoids re-resolution)
   *   processing-api/          ← copied from RustMode on creation / update
   *   src/
   *     main.rs                ← overwritten every build
   *   target/                  ← preserved (incremental builds)
   * </pre>
   */
  private File ensureCacheProject() throws IOException {
    File cacheRoot = new File(Compat.sketchbookFolder(null),
                              "rust-build-cache/" + sanitiseName(sketch.getName()));
    cacheRoot.mkdirs();

    File versionStamp = new File(cacheRoot, ".rustmode-version");
    boolean needsScaffold = !versionStamp.exists()
      || !readFile(versionStamp).trim().equals(RUSTMODE_VERSION);

    // Also re-copy the processing crate if rebuild-engine.sh updated it since
    // we last scaffolded.  The script touches rust-build-cache/.lib-stamp.
    if (!needsScaffold) {
      File libStamp = new File(cacheRoot.getParentFile(), ".lib-stamp");
      if (libStamp.exists() && libStamp.lastModified() > versionStamp.lastModified()) {
        needsScaffold = true;
      }
    }

    // The engine itself changed (a mode update, or work on src/lib.rs): every
    // sketch must pick it up, not keep compiling against its old copy.
    if (!needsScaffold) {
      for (String f : engineFiles(mode.getFolder())) {
        File ours   = new File(mode.getFolder(), f);
        File theirs = new File(cacheRoot, "processing/" + f);
        if (!theirs.exists() || ours.length() != theirs.length()
            || ours.lastModified() > theirs.lastModified()) {
          needsScaffold = true;
        }
      }
    }

    if (needsScaffold) {
      scaffoldProject(cacheRoot);
      writeFile(versionStamp, RUSTMODE_VERSION);
    }

    // Always ensure src/ exists (target/ is cargo's responsibility).
    new File(cacheRoot, "src").mkdirs();

    return cacheRoot;
  }

  /** Create or replace the non-source scaffolding. */
  private void scaffoldProject(File root) throws IOException {
    // ── sketch Cargo.toml ─────────────────────────────────────────────────────
    // The processing crate lives at RustMode/src/lib.rs; we copy it into a
    // processing/ subdir of the cache project so the path dep is relative.
    String sketchBinName = sanitiseName(sketch.getName());
    String cargoToml =
      "[package]\n" +
      "name = \"" + sketchBinName + "\"\n" +
      "version = \"0.1.0\"\n" +
      "edition = \"2021\"\n" +
      "\n" +
      "[dependencies]\n" +
      "processing = { path = \"processing\" }\n" +
      "\n" +
      "[features]\n" +
      "default = []\n" +
      "# Build for browser: cargo build --target wasm32-unknown-unknown\\\n" +
      "#   --features wasm --no-default-features\n" +
      "wasm = [\"processing/wasm\"]\n" +
      "\n" +
      "[profile.dev]\n" +
      "opt-level = 1\n" +
      "\n" +
      "[profile.release]\n" +
      "opt-level = 3\n" +
      "lto = true\n" +
      "codegen-units = 1\n";

    writeFile(new File(root, "Cargo.toml"), cargoToml);

    // ── processing/ ───────────────────────────────────────────────────────────
    // The crate source lives in RustMode/src/lib.rs and RustMode/Cargo.toml.
    // All backend stubs are inlined as `mod` blocks in lib.rs — no crates/ dir.
    File modeRoot = mode.getFolder();
    File destCrate = new File(root, "processing");
    destCrate.mkdirs();
    new File(destCrate, "src").mkdirs();

    Files.copy(new File(modeRoot, "Cargo.toml").toPath(),
               new File(destCrate, "Cargo.toml").toPath(),
               StandardCopyOption.REPLACE_EXISTING);
    // lib.rs plus its module files (data.rs and friends).
    for (String f : engineFiles(modeRoot)) {
      if (!f.endsWith(".rs")) continue;
      Files.copy(new File(modeRoot, f).toPath(), new File(destCrate, f).toPath(),
                 StandardCopyOption.REPLACE_EXISTING);
    }
  }

  /** Cargo.toml and every .rs file directly in the mode's src/ folder. */
  static java.util.List<String> engineFiles(File modeRoot) {
    java.util.List<String> out = new java.util.ArrayList<>();
    out.add("Cargo.toml");
    out.add("src/lib.rs");
    File[] rs = new File(modeRoot, "src").listFiles((d, n) -> n.endsWith(".rs") && !n.equals("lib.rs"));
    if (rs != null) {
      java.util.Arrays.sort(rs);
      for (File f : rs) out.add("src/" + f.getName());
    }
    return out;
  }

  // ── Step 6 – cargo build ────────────────────────────────────────────────────

  private File runCargo(File projectDir) throws IOException, InterruptedException {
    // ── Pre-warm: extract bundled dep rlibs on first run ─────────────────────────
    extractPrebuiltDeps();

    // ── Pre-flight: ensure cargo is on PATH ───────────────────────────────────
    if (!isCargoAvailable()) {
      throw new IOException(
        "cargo not found — install Rust from https://rustup.rs and restart Processing.");
    }

    List<String> cmd = new ArrayList<>();
    cmd.add("cargo");
    cmd.add("build");
    if (release) cmd.add("--release");
    cmd.add("--message-format=short");

    ProcessBuilder pb = new ProcessBuilder(cmd);
    pb.directory(projectDir);
    pb.redirectErrorStream(true);  // merge stderr into stdout

    // Propagate CARGO_HOME / RUSTUP_HOME from the environment; add sccache if
    // available so that the processing crate and minifb are cached across sketches.
    pb.environment().put("RUSTFLAGS",
      System.getenv().getOrDefault("RUSTFLAGS", "") + " -C incremental=true");

    // Point all sketches at a shared target dir so deps are compiled once.
    pb.environment().put("CARGO_TARGET_DIR", sharedTarget().getAbsolutePath());

    if (cancelled) throw new InterruptedException("build cancelled");
    Process proc = pb.start();
    cargoProc = proc;
    if (cancelled) cancel();   // Stop arrived while cargo was starting
    try { proc.getOutputStream().close(); } catch (IOException ignored) {}

    // Stream output, rewriting error lines in real time.
    BufferedReader reader = new BufferedReader(
      new InputStreamReader(proc.getInputStream(), StandardCharsets.UTF_8));

    // Collect structured diagnostics for gutter markers; first error for status bar.
    List<RustProblem> diagnostics = new ArrayList<>();
    String firstError = null;
    boolean firstErrorLocated = false;
    String line;
    while ((line = reader.readLine()) != null) {
      String rewritten = rewriteLocations(line);
      // Forward all output to the IDE console.
      mode.statusMessage(rewritten);

      // --message-format=short gives one line per problem:
      //   Sketch.pde:12:5: error[E0308]: mismatched types: ...
      RustProblem dp = parseRewrittenDiagnostic(rewritten);
      if (dp != null) diagnostics.add(dp);
      boolean located = dp != null && dp.getSeverity() == RustProblem.ERROR;
      if (located && !firstErrorLocated) {
        firstError = rewritten.trim();      // an error with a line beats one without
        firstErrorLocated = true;
      } else if (firstError == null
                 && (rewritten.startsWith("error") || rewritten.contains(": error"))) {
        firstError = rewritten.trim();
      }
    }

    int exitCode;
    try {
      exitCode = proc.waitFor();
    } catch (InterruptedException e) {
      cancel();
      throw e;
    } finally {
      cargoProc = null;
    }
    if (cancelled) throw new InterruptedException("build cancelled");
    if (exitCode != 0) {
      String msg = firstError != null
        ? firstError
        : "cargo build failed (exit " + exitCode + ") — see console for details";
      throw new CargoException(msg, diagnostics);
    }

    // Find the binary in the shared target dir.
    String profile   = release ? "release" : "debug";
    String sketchBin = sanitiseName(sketch.getName());
    String binaryName = isWindows() ? sketchBin + ".exe" : sketchBin;
    File binary = new File(sharedTarget(), profile + "/" + binaryName);

    if (!binary.exists()) {
      throw new IOException("Build succeeded but binary not found: " + binary);
    }
    return binary;
  }

  // ── Diagnostic parsing ──────────────────────────────────────────────────────

  /** A sketch location as it reads after rewriting: {@code Sketch.pde:12:5} */
  private static final Pattern REWRITTEN_LOCATION =
    Pattern.compile("(\\S+\\.(?:pde|prs|rs)):(\\d+):(\\d+)");

  /**
   * A {@link RustProblem} from a rewritten cargo output line, or {@code null}
   * if the line doesn't point into the sketch.
   */
  private RustProblem parseRewrittenDiagnostic(String line) {
    Matcher m = REWRITTEN_LOCATION.matcher(line);
    if (!m.find()) return null;

    String fileName = m.group(1);
    int    lineNum  = Integer.parseInt(m.group(2));  // 1-based
    int    col      = Integer.parseInt(m.group(3));  // 1-based

    SketchCode[] codes = sketch.getCode();
    int tabIndex = -1;
    for (int i = 0; i < codes.length; i++) {
      if (codes[i].getFileName().equals(fileName)) { tabIndex = i; break; }
    }
    if (tabIndex < 0) return null;   // generated code or the engine, not a tab

    boolean isErr = line.contains("error") && !line.contains(": warning");
    return new RustProblem(tabIndex, lineNum - 1, col - 1,
                           line.trim(),
                           isErr ? RustProblem.ERROR : RustProblem.WARNING,
                           -1, -1);
  }

  // ── Error rewriting ─────────────────────────────────────────────────────────

  /** Where a line of the generated main.rs came from: {tab index, 1-based line in that tab}, or null. */
  private int[] sketchPosition(int mainRsLine) {
    if (tabStartLines == null || tabStartLines.length == 0) return null;
    for (int[] chunk : lineMap) {
      if (mainRsLine >= chunk[0] && mainRsLine < chunk[0] + chunk[2]) {
        int sketchLine = chunk[1] + (mainRsLine - chunk[0]);
        int tab = 0;
        for (int i = tabStartLines.length - 1; i >= 0; i--) {
          if (sketchLine >= tabStartLines[i]) { tab = i; break; }
        }
        return new int[] { tab, sketchLine - tabStartLines[tab] + 1 };
      }
    }
    return null;
  }

  /**
   * Rewrites a line of cargo or sketch output so that every
   * {@code src/main.rs:LINE[:COL]} names the sketch tab and the line in that
   * tab, and {@code processing::} prefixes are gone. Locations in generated
   * glue (nothing the person wrote) are left as they are.
   */
  public String rewriteLocations(String line) {
    if (line == null) return null;
    line = CRATE_PREFIX_PATTERN.matcher(line).replaceAll("");
    if (line.indexOf("main.rs") < 0) return line;

    Matcher m = ERR_LINE_PATTERN.matcher(line);
    StringBuilder sb = new StringBuilder();
    while (m.find()) {
      String repl = m.group();
      try {
        int[] pos = sketchPosition(Integer.parseInt(m.group(2)));
        if (pos != null && pos[0] < sketch.getCodeCount()) {
          repl = (m.group(1) != null ? m.group(1) : "")
            + sketch.getCode(pos[0]).getFileName() + ":" + pos[1]
            + (m.group(3) != null ? ":" + m.group(3) : "");
        }
      } catch (NumberFormatException ignored) {}
      m.appendReplacement(sb, Matcher.quoteReplacement(repl));
    }
    m.appendTail(sb);
    return sb.toString();
  }

  // ── Utilities ───────────────────────────────────────────────────────────────

  /**
   * Extract the platform-specific prebuilt dep rlibs (bundled in the mode dir)
   * into the shared Cargo target so the first sketch build skips dep compilation.
   * Runs once per mode version; a stamp file prevents repeat extraction.
   */
  private void extractPrebuiltDeps() {
    try {
      String os   = System.getProperty("os.name",  "").toLowerCase();
      String arch = System.getProperty("os.arch",  "").toLowerCase();
      String platform;
      if (os.contains("win"))       platform = "windows-x86-64";
      else if (os.contains("mac"))  platform = arch.contains("aarch64") ? "macos-arm64" : "macos-x86-64";
      else                          platform = arch.contains("aarch64") ? "linux-arm64"  : "linux-x86-64";

      File modeDir = mode.getFolder();
      File bundle  = new File(modeDir, "prebuilt-deps/" + platform + ".tar.gz");
      if (!bundle.exists()) return;  // not bundled for this platform

      File sharedDeps = new File(sharedTarget(), "debug/deps");

      File stamp = new File(Compat.sketchbookFolder(null),
        "rust-build-cache/.prebuilt-stamp-" + platform);

      // Re-extract if bundle is newer than stamp
      if (stamp.exists() && stamp.lastModified() >= bundle.lastModified()) return;

      sharedDeps.mkdirs();
      LOG.info("RustMode: extracting prebuilt deps for " + platform + "...");

      ProcessBuilder pb = new ProcessBuilder(
        "tar", "xzf", bundle.getAbsolutePath(),
        "--strip-components=1",
        "-C", sharedDeps.getAbsolutePath());
      pb.redirectErrorStream(true);
      Process p = pb.start();
      p.waitFor();

      stamp.getParentFile().mkdirs();
      stamp.createNewFile();
      LOG.info("RustMode: prebuilt deps extracted to " + sharedDeps);
    } catch (Exception e) {
      LOG.log(java.util.logging.Level.WARNING, "RustMode: could not extract prebuilt deps", e);
    }
  }

  /** Returns true if {@code cargo} is reachable on the current PATH. */
  private static boolean isCargoAvailable() {
    try {
      Process p = new ProcessBuilder("cargo", "--version")
        .redirectErrorStream(true)
        .start();
      p.getInputStream().transferTo(java.io.OutputStream.nullOutputStream());
      return p.waitFor() == 0;
    } catch (IOException | InterruptedException e) {
      return false;
    }
  }

  private static String sanitiseName(String name) {
    return name.replaceAll("[^A-Za-z0-9_\\-]", "_");
  }

  private static boolean isWindows() {
    return System.getProperty("os.name", "").toLowerCase().startsWith("win");
  }

  private static void writeFile(File f, String content) throws IOException {
    f.getParentFile().mkdirs();
    try (PrintWriter pw = new PrintWriter(new FileWriter(f, StandardCharsets.UTF_8))) {
      pw.print(content);
    }
  }

  private static String readFile(File f) throws IOException {
    return new String(Files.readAllBytes(f.toPath()), StandardCharsets.UTF_8);
  }

  /**
   * Recursively copy {@code src} directory to {@code dest}, preserving
   * structure.  Existing files are overwritten.
   */
  private static void copyDirectory(File src, File dest) throws IOException {
    if (!src.exists()) return;
    dest.mkdirs();
    File[] children = src.listFiles();
    if (children == null) return;
    for (File child : children) {
      File target = new File(dest, child.getName());
      if (child.isDirectory()) {
        copyDirectory(child, target);
      } else {
        Files.copy(child.toPath(), target.toPath(),
          StandardCopyOption.REPLACE_EXISTING);
      }
    }
  }
}
