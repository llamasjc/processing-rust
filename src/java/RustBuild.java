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
  private static final String RUSTMODE_VERSION = "0.2.0";

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
  /** Where this build's binaries go: libprocessing builds have their own. */
  private static File targetDir() {
    return Renderer.libprocessing()
      ? new File(sharedTarget().getParentFile(), ".shared-target-libprocessing")
      : sharedTarget();
  }

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

    // The local libprocessing fork can appear (or move) after the scaffold:
    // keep the sketch's Cargo.toml pointing at it.
    if (!needsScaffold) {
      File toml = new File(cacheRoot, "Cargo.toml");
      String have = toml.exists() ? readFile(toml) : "";
      String patch = localForkPatch();
      int at = have.indexOf(FORK_MARK);
      int end = at < 0 ? -1 : have.indexOf(FORK_END, at);
      String current = at < 0 || end < 0 ? "" : have.substring(at, end + FORK_END.length());
      if (!current.equals(patch)) {
        String base = at < 0 || end < 0 ? have : have.substring(0, at) + have.substring(end + FORK_END.length());
        int prof = base.indexOf("[profile.dev]");
        String next = prof < 0 ? base + patch : base.substring(0, prof) + patch + base.substring(prof);
        writeFile(toml, next);
      }
    }

    // Always ensure src/ exists (target/ is cargo's responsibility).
    new File(cacheRoot, "src").mkdirs();

    return cacheRoot;
  }

  private static final String FORK_MARK = "# local libprocessing (begin)\n";
  private static final String FORK_END  = "# local libprocessing (end)\n";

  /**
   * Your own libprocessing checkout, when there is one: LIBPROCESSING_DIR, the
   * folder named in the rust.libprocessing preference, ~/lp-fork, or
   * sketchbook/libprocessing. null when none of them has the crates.
   */
  static File localFork() {
    java.util.List<String> tries = new java.util.ArrayList<>();
    String env = System.getenv("LIBPROCESSING_DIR");
    if (env != null && !env.isBlank()) tries.add(env);
    String pref = java.util.prefs.Preferences.userRoot().node("processing/rustmode").get("rust.libprocessing", "");
    if (!pref.isBlank()) tries.add(pref);
    tries.add(new File(System.getProperty("user.home"), "lp-fork").getPath());
    tries.add(new File(Compat.sketchbookFolder(null), "libprocessing").getPath());
    for (String t : tries) {
      File d = new File(t);
      if (new File(d, "crates/processing_render/Cargo.toml").isFile()) return d;
    }
    return null;
  }

  /** The [patch] that swaps the pinned libprocessing for your checkout ("" when there is none). */
  static String localForkPatch() {
    File d = localFork();
    if (d == null) return "";
    StringBuilder sb = new StringBuilder(FORK_MARK);
    sb.append("[patch.\"https://github.com/processing/libprocessing\"]\n");
    for (String c : new String[] { "processing_core", "processing_render", "processing_input", "processing_glfw" }) {
      String path = new File(d, "crates/" + c).getAbsolutePath().replace("\\", "/");
      sb.append(c).append(" = { path = \"").append(path).append("\" }\n");
    }
    sb.append(FORK_END).append("\n");
    return sb.toString();
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
      "# The libprocessing renderer (Bevy + wgpu), picked in the toolbar\n" +
      "libprocessing = [\"processing/libprocessing\"]\n" +
      "\n" +
      "# libprocessing's Bevy fork stands in for upstream Bevy (as in libprocessing itself)\n" +
      "[patch.\"https://github.com/bevyengine/bevy\"]\n" +
      "bevy = { git = \"https://github.com/processing/bevy\", branch = \"main\" }\n" +
      "\n" +
      localForkPatch() +
      "[profile.dev]\n" +
      "opt-level = 1\n" +
      "\n" +
      "[profile.release]\n" +
      "opt-level = 3\n" +
      "lto = true\n" +
      "codegen-units = 1\n";

    writeFile(new File(root, "Cargo.toml"), cargoToml);
    // Start from the engine's own Cargo.lock: the same dependency versions
    // the mode was tested with, and no
    // index lookup on the first build.
    File lock = new File(root, "Cargo.lock");
    File modeLock = new File(mode.getFolder(), "Cargo.lock");
    // (re)made with the scaffold, so a mode update brings its tested versions
    if (modeLock.isFile()) {
      try {
        Files.copy(modeLock.toPath(), lock.toPath(), StandardCopyOption.REPLACE_EXISTING);
      } catch (IOException e) {
        // cargo will make its own
      }
    }


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
    // build.rs checks what the libprocessing checkout supports (textures,
    // attrib(), window events); without it those are off in IDE builds
    if (new File(modeRoot, "build.rs").isFile()) out.add("build.rs");
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
    // the first build of the session: Rust, a linker and (for libprocessing)
    // the dev files are there, or get installed (see RustSetup)
    RustSetup.ensure(mode, Renderer.libprocessing());
    // Movie or Capture without ffmpeg: offer to install it (see FfmpegSetup)
    FfmpegSetup.ensure(sketch.getFolder(), ".pde", mode::statusMessage);

    // ── Fast path: one rustc call against the precompiled engine ─────────────
    if (!release) {
      File fast = cachedBuild(projectDir);
      if (fast != null) return fast;
    }

    // ── Pre-flight: ensure cargo is on PATH ───────────────────────────────────
    if (!isCargoAvailable()) {
      throw new IOException(
        "cargo not found — install Rust from https://rustup.rs and restart Processing.");
    }

    List<String> cmd = new ArrayList<>();
    cmd.add(RustSetup.tool("cargo"));
    cmd.add("build");
    if (release) cmd.add("--release");
    cmd.add("--message-format=short");
    boolean lp = Renderer.libprocessing();
    if (lp) {
      cmd.add("--features");
      cmd.add("libprocessing");
      File fork = localFork();
      mode.statusMessage("Building for libprocessing" + (fork != null ? " from " + fork : " (pinned upstream)")
        + ". The first build compiles Bevy and takes several minutes.");
      if (fork == null && !new File(mode.getFolder(), "cache/" + platformDir() + "/libprocessing").isDirectory())
        mode.statusMessage("RustMode-lite.zip is CPU only. RustMode.zip has libprocessing precompiled:"
          + " unzip it over this one and restart Processing, and this wait goes away.");
    }

    ProcessBuilder pb = new ProcessBuilder(cmd);
    pb.directory(projectDir);
    pb.redirectErrorStream(true);  // merge stderr into stdout
    RustSetup.prepare(pb.environment(), mode.getFolder());

    // Propagate CARGO_HOME / RUSTUP_HOME from the environment; add sccache if
    // available so that the processing crate and minifb are cached across sketches.
    pb.environment().put("RUSTFLAGS",
      System.getenv().getOrDefault("RUSTFLAGS", "") + " -C incremental=true");

    // Point all sketches at a shared target dir so deps are compiled once.
    // (libprocessing builds get their own, so switching back and forth doesn't rebuild everything)
    pb.environment().put("CARGO_TARGET_DIR", targetDir().getAbsolutePath());
    // the engine's build.rs checks this same checkout for what it supports
    // (empty: the pinned upstream commit)
    File fork = localFork();
    pb.environment().put("LIBPROCESSING_DIR", fork == null ? "" : fork.getAbsolutePath());

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
      if (rewritten.contains("failed to find glfw") || rewritten.contains("glfw3.pc")) missingGlfw = true;
      if (RustSetup.looksMissing(rewritten)) missingTools = true;

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
    if (exitCode != 0 && missingTools) {
      // something to build with isn't installed: offer it, as before the first build
      RustSetup.recheck(mode, Renderer.libprocessing());
      throw new CargoException("Something Rust Mode needs to build isn't installed yet (see above). Press Run again once it is.", diagnostics);
    }
    if (exitCode != 0 && missingGlfw) {
      throw new CargoException(glfwHelp(), diagnostics);
    }
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
    File binary = new File(targetDir(), profile + "/" + binaryName);

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

  // ── The precompiled engine (cache/<platform>/<renderer>/) ──────────────────
  //
  // scripts/build-cache.py builds the engine and every crate it uses once per
  // platform; a sketch is then a single rustc call against those libraries,
  // about a second instead of minutes. It needs the exact Rust version the
  // cache was made with (rustup gets it the first time) and the engine
  // sources it was made from. Anything off and the normal cargo build runs.

  static String platformDir() {
    String os = System.getProperty("os.name", "").toLowerCase();
    String arch = System.getProperty("os.arch", "").toLowerCase();
    boolean arm = arch.contains("aarch64") || arch.contains("arm");
    if (os.contains("win")) return "windows-x86-64";
    if (os.contains("mac")) return arm ? "macos-arm64" : "macos-x86-64";
    return arm ? "linux-arm64" : "linux-x86-64";
  }

  /** The engine's sources and lockfile, hashed like scripts/build-cache.py does. */
  static String engineHash(File root) {
    try {
      java.security.MessageDigest md = java.security.MessageDigest.getInstance("SHA-256");
      List<String> names = new ArrayList<>(List.of("Cargo.toml", "Cargo.lock"));
      if (new File(root, "build.rs").isFile()) names.add("build.rs");
      List<String> rs = new ArrayList<>();
      File[] fs = new File(root, "src").listFiles((d, n) -> n.endsWith(".rs"));
      if (fs != null) for (File f : fs) rs.add("src/" + f.getName());
      java.util.Collections.sort(rs);
      names.addAll(rs);
      for (String n : names) {
        byte[] data = Files.readAllBytes(new File(root, n).toPath());
        // line endings don't count, so a Windows checkout matches
        java.io.ByteArrayOutputStream clean = new java.io.ByteArrayOutputStream(data.length);
        for (int i = 0; i < data.length; i++) {
          if (data[i] == '\r' && i + 1 < data.length && data[i + 1] == '\n') continue;
          clean.write(data[i]);
        }
        md.update(n.getBytes(StandardCharsets.UTF_8));
        md.update((byte) 0);
        md.update(clean.toByteArray());
        md.update((byte) 0);
      }
      StringBuilder sb = new StringBuilder();
      for (byte b : md.digest()) sb.append(String.format("%02x", b));
      return sb.toString();
    } catch (Exception e) {
      return "";
    }
  }

  private static String jsonString(String json, String key) {
    Matcher m = Pattern.compile("\"" + key + "\"\\s*:\\s*\"([^\"]*)\"").matcher(json);
    return m.find() ? m.group(1) : null;
  }
  private static List<String> jsonStrings(String json, String key) {
    List<String> out = new ArrayList<>();
    Matcher m = Pattern.compile("\"" + key + "\"\\s*:\\s*\\[([^\\]]*)\\]", Pattern.DOTALL).matcher(json);
    if (m.find()) {
      Matcher s = Pattern.compile("\"((?:[^\"\\\\]|\\\\.)*)\"").matcher(m.group(1));
      while (s.find()) out.add(s.group(1).replace("\\\\", "\\"));
    }
    return out;
  }

  /** Runs a short command, returns its output, or null when it can't run. */
  private static String runQuiet(List<String> cmd, java.util.Map<String, String> env) {
    try {
      List<String> c = new ArrayList<>(cmd);
      c.set(0, RustSetup.tool(c.get(0)));
      ProcessBuilder pb = new ProcessBuilder(c).redirectErrorStream(true);
      RustSetup.prepare(pb.environment(), null);
      if (env != null) pb.environment().putAll(env);
      Process p = pb.start();
      String out = new String(p.getInputStream().readAllBytes(), StandardCharsets.UTF_8);
      return p.waitFor() == 0 ? out : null;
    } catch (IOException | InterruptedException e) {
      return null;
    }
  }

  /** Errors that mean the cache doesn't fit this machine, not that the sketch is wrong. */
  private static final Pattern CACHE_MISMATCH = Pattern.compile(
    "E0514|E0460|E0461|E0462|E0463|E0786|incompatible version of rustc|linking with .* failed|could not find native static library");

  /** Newest modification time (seconds) of the checkout crates RustMode uses; same walk as build-cache.py. */
  static long forkStamp(File fork) {
    long newest = 0;
    for (String c : new String[] { "processing_core", "processing_render", "processing_input", "processing_glfw" }) {
      File d = new File(fork, "crates/" + c);
      newest = Math.max(newest, new File(d, "Cargo.toml").lastModified() / 1000);
      java.util.ArrayDeque<File> todo = new java.util.ArrayDeque<>();
      todo.add(new File(d, "src"));
      while (!todo.isEmpty()) {
        File[] kids = todo.pop().listFiles();
        if (kids == null) continue;
        for (File k : kids) {
          if (k.isDirectory()) todo.add(k);
          else newest = Math.max(newest, k.lastModified() / 1000);
        }
      }
    }
    return newest;
  }

  private static String jsonNumber(String json, String key) {
    java.util.regex.Matcher m = Pattern.compile("\"" + Pattern.quote(key) + "\"\\s*:\\s*(\\d+)").matcher(json);
    return m.find() ? m.group(1) : null;
  }

  private boolean missingGlfw;
  private boolean missingTools;

  /**
   * libprocessing needs GLFW 3.4 when a sketch builds without the prebuilt
   * cache (the cache brings its own). Like C++ Mode's installer, say exactly
   * what to run on this system.
   */
  static String glfwHelp() {
    String os = System.getProperty("os.name", "").toLowerCase();
    String cmd;
    if (os.contains("mac")) cmd = "brew install glfw";
    else if (os.contains("win")) cmd = "reinstall Rust Mode (GLFW comes with it on Windows)";
    else if (new File("/usr/bin/pacman").exists()) cmd = "sudo pacman -S glfw";
    else if (new File("/usr/bin/dnf").exists()) cmd = "sudo dnf install glfw-devel";
    else if (new File("/usr/bin/zypper").exists()) cmd = "sudo zypper install glfw-devel";
    else if (new File("/usr/bin/apt").exists())
      cmd = "sudo apt install libglfw3-dev (Ubuntu 24.04 or newer; 22.04 only has GLFW 3.3, so build 3.4 from glfw.org)";
    else cmd = "your package manager";
    return "The libprocessing renderer needs GLFW 3.4, which isn't installed. Install it with " + cmd
      + ", or pick the CPU renderer in the toolbar, which needs nothing.";
  }

  /**
   * The GLFW the libprocessing cache ships (its native folder), or null.
   * Sketches link against it, and it goes on the library path when they run
   * and into exported applications, so nobody has to install GLFW 3.4.
   */
  static File glfwDir(File modeFolder) {
    if (!Renderer.libprocessing()) return null;
    String plat = platformDir();
    for (String r : new String[] { "libprocessing-fork", "libprocessing" }) {
      File d = new File(modeFolder, "cache/" + plat + "/" + r + "/native");
      if (AppExport.glfwLibs(d).length > 0) return d;
    }
    return null;
  }

  private File cachedBuild(File projectDir) throws IOException, InterruptedException {
    String renderer = Renderer.libprocessing() ? "libprocessing" : "cpu";
    // with a libprocessing checkout of your own, only a cache built from that
    // checkout (libprocessing-fork) will do; the pinned one lacks its changes
    File fork = Renderer.libprocessing() ? localFork() : null;
    if (fork != null) renderer = "libprocessing-fork";
    File dir = new File(mode.getFolder(), "cache/" + platformDir() + "/" + renderer);
    File meta = new File(dir, "cache.json");
    if (!meta.isFile()) return null;
    String json = readFile(meta);
    if (fork != null) {
      String from = jsonString(json, "libprocessing");
      String stamp = jsonNumber(json, "fork_newest");
      if (from == null || !new File(from).getCanonicalPath().equals(fork.getCanonicalPath())
          || stamp == null || forkStamp(fork) > Long.parseLong(stamp)) {
        LOG.info("RustMode: libprocessing checkout changed since the cache was built; using cargo");
        return null;
      }
    }
    String version = jsonString(json, "rustc");
    String engine = jsonString(json, "engine");
    String hash = jsonString(json, "engine_hash");
    if (version == null || engine == null || hash == null) return null;
    if (!hash.equals(engineHash(mode.getFolder()))) {
      LOG.info("RustMode: the engine changed since the cache was built; using cargo");
      return null;
    }
    File engineLib = new File(dir, "deps/" + engine);
    if (!engineLib.isFile()) return null;

    // the cache's exact Rust: through rustup (fetched once), or a matching rustc
    java.util.Map<String, String> env = new java.util.HashMap<>();
    String have = runQuiet(List.of("rustup", "toolchain", "list"), null);
    String tc = RustSetup.toolchain(version);
    if (have != null) {
      if (!have.contains(tc)) {
        mode.statusMessage("Getting Rust " + version + " for fast builds (once, a minute or so)...");
        String got = runQuiet(List.of("rustup", "toolchain", "install", tc, "--profile", "minimal"), null);
        if (got == null) { LOG.info("RustMode: couldn't install Rust " + version + "; using cargo"); return null; }
      }
      env.put("RUSTUP_TOOLCHAIN", tc);
    } else {
      String v = runQuiet(List.of("rustc", "-V"), null);
      if (v == null || !v.contains(" " + version + " ")) return null;
    }

    String bin = sanitiseName(sketch.getName());
    File outDir = new File(targetDir(), "fast");
    outDir.mkdirs();
    File binary = new File(outDir, isWindows() ? bin + ".exe" : bin);

    List<String> cmd = new ArrayList<>(List.of(
      "rustc", "--edition", "2021", "--crate-name", bin, "--crate-type", "bin",
      "-C", "opt-level=1", "--error-format=short",
      "-L", "dependency=" + new File(dir, "deps").getAbsolutePath(),
      "-L", "native=" + new File(dir, "native").getAbsolutePath(),
      "--extern", "processing=" + engineLib.getAbsolutePath(),
      "-o", binary.getAbsolutePath()));
    for (String p : jsonStrings(json, "link_search")) {
      if (new File(p).isDirectory()) { cmd.add("-L"); cmd.add("native=" + p); }
    }
    cmd.add("src/main.rs");

    cmd.set(0, RustSetup.tool(cmd.get(0)));
    ProcessBuilder pb = new ProcessBuilder(cmd);
    pb.directory(projectDir);
    pb.redirectErrorStream(true);
    RustSetup.prepare(pb.environment(), null);
    pb.environment().putAll(env);
    if (cancelled) throw new InterruptedException("build cancelled");
    Process proc = pb.start();
    cargoProc = proc;
    if (cancelled) cancel();
    try { proc.getOutputStream().close(); } catch (IOException ignored) {}

    List<RustProblem> diagnostics = new ArrayList<>();
    List<String> lines = new ArrayList<>();
    String firstError = null;
    boolean mismatch = false;
    BufferedReader reader = new BufferedReader(new InputStreamReader(proc.getInputStream(), StandardCharsets.UTF_8));
    String line;
    while ((line = reader.readLine()) != null) {
      if (CACHE_MISMATCH.matcher(line).find()) mismatch = true;
      String rewritten = rewriteLocations(line);
      lines.add(rewritten);
      RustProblem dp = parseRewrittenDiagnostic(rewritten);
      if (dp != null) diagnostics.add(dp);
      if (firstError == null && dp != null && dp.getSeverity() == RustProblem.ERROR) firstError = rewritten.trim();
    }
    int exit;
    try { exit = proc.waitFor(); } finally { cargoProc = null; }
    if (cancelled) throw new InterruptedException("build cancelled");
    if (exit != 0 && mismatch) {
      LOG.info("RustMode: the precompiled engine doesn't fit here; using cargo");
      return null;
    }
    // the sketch's own messages, as cargo would show them
    for (String l : lines) mode.statusMessage(l);
    if (exit != 0) {
      throw new CargoException(firstError != null ? firstError : "rustc failed (exit " + exit + ")", diagnostics);
    }
    return binary.isFile() ? binary : null;
  }

  /** Returns true if {@code cargo} is reachable on the current PATH. */
  private static boolean isCargoAvailable() {
    try {
      Process p = new ProcessBuilder(RustSetup.tool("cargo"), "--version")
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
