package processing.mode.rust;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import javax.swing.JOptionPane;
import javax.swing.SwingUtilities;

/**
 * Gets a computer ready to build sketches, the way C++ Mode's InstallWizard
 * does: find what's missing, say it in one dialog, and install it when the
 * user says yes. What a sketch needs:
 *
 * <ul>
 * <li>Rust, through rustup. Installed into the user's home, no admin rights.
 *     Found in ~/.cargo/bin even when Processing was started from a desktop
 *     menu that doesn't have it on PATH.</li>
 * <li>A linker. On Windows Rust Mode uses the windows-gnu toolchain, which
 *     brings its own, so Microsoft's Build Tools are never needed. macOS
 *     needs the Command Line Tools (Apple's own installer is started). Linux
 *     needs a C compiler, and for libprocessing the X11, ALSA and udev
 *     development files (installed with the package manager, through pkexec).</li>
 * </ul>
 *
 * Checked once per session before the first build, and again when a build
 * fails with an error that means something is missing.
 */
final class RustSetup {
  private RustSetup() {}

  private static volatile boolean checked;

  static boolean windows() { return System.getProperty("os.name", "").toLowerCase().contains("win"); }
  static boolean mac() { return System.getProperty("os.name", "").toLowerCase().contains("mac"); }

  /** Where rustup puts cargo, rustc and rustup itself. */
  static File cargoBin() {
    String home = System.getenv("CARGO_HOME");
    if (home != null && !home.isEmpty()) return new File(home, "bin");
    return new File(System.getProperty("user.home"), ".cargo" + File.separator + "bin");
  }

  /** The Rust version the engine is pinned to (rust-toolchain.toml). */
  static String pinnedVersion(File modeFolder) {
    try {
      String t = Files.readString(new File(modeFolder, "rust-toolchain.toml").toPath());
      java.util.regex.Matcher m = java.util.regex.Pattern.compile("channel\\s*=\\s*\"([^\"]+)\"").matcher(t);
      if (m.find()) return m.group(1);
    } catch (IOException ignored) {}
    return "stable";
  }

  /**
   * The toolchain to build with. On Windows, the windows-gnu one: it links
   * with the MinGW linker it ships, so there's nothing else to install.
   */
  static String toolchain(String version) {
    return windows() ? version + "-x86_64-pc-windows-gnu" : version;
  }

  /** PATH with ~/.cargo/bin in front, and the toolchain Rust Mode builds with. */
  static void prepare(Map<String, String> env, File modeFolder) {
    String path = env.getOrDefault(windows() ? "Path" : "PATH", env.getOrDefault("PATH", ""));
    String bin = cargoBin().getAbsolutePath();
    if (!path.contains(bin)) path = bin + File.pathSeparator + path;
    env.put("PATH", path);
    if (windows()) env.put("Path", path);
    // Windows: the windows-gnu toolchain, so no Microsoft linker is needed
    if (windows() && modeFolder != null && !env.containsKey("RUSTUP_TOOLCHAIN"))
      env.put("RUSTUP_TOOLCHAIN", toolchain(pinnedVersion(modeFolder)));
  }

  /** A program by name, looked up in ~/.cargo/bin first. */
  static String tool(String name) {
    File f = new File(cargoBin(), windows() ? name + ".exe" : name);
    return f.isFile() ? f.getAbsolutePath() : name;
  }

  private static boolean runs(String... cmd) {
    try {
      Process p = new ProcessBuilder(cmd).redirectErrorStream(true).start();
      p.getInputStream().transferTo(java.io.OutputStream.nullOutputStream());
      return p.waitFor(30, TimeUnit.SECONDS) && p.exitValue() == 0;
    } catch (Exception e) {
      return false;
    }
  }

  private static boolean exists(String... paths) {
    for (String p : paths) if (new File(p).exists()) return true;
    return false;
  }

  /** What's missing for building, in words, with what installs it. */
  static List<String> missing(File modeFolder, boolean libprocessing) {
    List<String> out = new ArrayList<>();
    if (!runs(tool("rustup"), "--version") && !runs(tool("cargo"), "--version")) out.add("rust");
    if (mac() && !runs("xcode-select", "-p")) out.add("xcode");
    if (!mac() && !windows()) {
      if (!runs("cc", "--version") && !runs("gcc", "--version")) out.add("cc");
      if (libprocessing && !runs("pkg-config", "--exists", "x11", "alsa", "libudev")) out.add("devlibs");
    }
    return out;
  }

  /**
   * Before the first build: checks, and offers to install what's missing.
   * Returns false if the user said no or the install didn't work (the build
   * then fails with its own error). Call off the Swing thread.
   */
  static boolean ensure(RustMode mode, boolean libprocessing) {
    if (checked) return true;
    // Windows with Rust already there: add the windows-gnu toolchain if it isn't
    if (windows() && runs(tool("rustup"), "--version")) {
      String tc = toolchain(pinnedVersion(mode.getFolder()));
      if (!runs(tool("rustup"), "run", tc, "rustc", "-V")) {
        mode.statusMessage("Getting the Rust toolchain that needs no Visual Studio (once, a minute or two)...");
        runs(tool("rustup"), "toolchain", "install", tc, "--profile", "minimal");
      }
    }
    List<String> m = missing(mode.getFolder(), libprocessing);
    if (m.isEmpty()) { checked = true; return true; }
    boolean ok = offer(mode, m);
    if (ok) checked = true;
    return ok;
  }

  /** After a failed build: does the output mean something isn't installed? */
  static boolean looksMissing(String output) {
    String o = output.toLowerCase();
    return o.contains("linker `cc` not found") || o.contains("linker 'cc' not found")
        || o.contains("cannot find -lx11") || o.contains("cannot find -lasound") || o.contains("cannot find -ludev")
        || o.contains("xcrun: error") || o.contains("invalid active developer path")
        || o.contains("link.exe") && o.contains("not found")
        || o.contains("is not installed") && o.contains("toolchain");
  }

  static void recheck(RustMode mode, boolean libprocessing) {
    checked = false;
    ensure(mode, libprocessing);
  }

  private static String describe(List<String> m) {
    List<String> words = new ArrayList<>();
    if (m.contains("rust")) words.add("Rust (installed with rustup into your home folder, about 300 MB)");
    if (m.contains("xcode")) words.add("Apple's Command Line Tools (the linker)");
    if (m.contains("cc")) words.add("a C compiler (the linker)");
    if (m.contains("devlibs")) words.add("the X11, ALSA and udev development files (for libprocessing)");
    return " - " + String.join("\n - ", words);
  }

  private static boolean offer(RustMode mode, List<String> m) {
    final int[] answer = { JOptionPane.NO_OPTION };
    try {
      SwingUtilities.invokeAndWait(() -> answer[0] = JOptionPane.showConfirmDialog(null,
        "To build sketches, Rust Mode needs:\n\n" + describe(m) + "\n\nInstall now?",
        "Rust Mode setup", JOptionPane.YES_NO_OPTION, JOptionPane.QUESTION_MESSAGE));
    } catch (Exception e) {
      return false;
    }
    if (answer[0] != JOptionPane.YES_OPTION) return false;
    try {
      if (m.contains("rust")) installRust(mode);
      if (m.contains("xcode")) {
        // Apple's own installer; it runs on its own and we wait for it
        runs("xcode-select", "--install");
        mode.statusMessage("Finish installing the Command Line Tools in Apple's window, then press Run again.");
        return false;
      }
      if (m.contains("cc") || m.contains("devlibs")) installLinuxPackages(mode);
    } catch (Exception e) {
      mode.statusMessage("Setup didn't finish: " + e.getMessage());
      return false;
    }
    return missing(mode.getFolder(), m.contains("devlibs")).isEmpty();
  }

  private static void installRust(RustMode mode) throws IOException, InterruptedException {
    String version = pinnedVersion(mode.getFolder());
    mode.statusMessage("Installing Rust " + version + " (a minute or two)...");
    List<String> cmd;
    if (windows()) {
      File exe = File.createTempFile("rustup-init", ".exe");
      try (InputStream in = URI.create("https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-gnu/rustup-init.exe").toURL().openStream()) {
        Files.copy(in, exe.toPath(), StandardCopyOption.REPLACE_EXISTING);
      }
      cmd = List.of(exe.getAbsolutePath(), "-y", "--profile", "minimal",
        "--default-host", "x86_64-pc-windows-gnu", "--default-toolchain", version);
    } else {
      cmd = List.of("sh", "-c", "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain " + version);
    }
    Process p = new ProcessBuilder(cmd).redirectErrorStream(true).start();
    try (var r = new java.io.BufferedReader(new java.io.InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8))) {
      String line;
      while ((line = r.readLine()) != null) mode.statusMessage(line);
    }
    if (p.waitFor() != 0) throw new IOException("the Rust installer didn't finish (see the console)");
  }

  /** The package manager's install command for the linker and dev files, as C++ Mode does it. */
  static String[] linuxInstall() {
    if (exists("/usr/bin/pacman")) return new String[] { "pacman", "-S", "--noconfirm", "--needed", "base-devel", "libx11", "alsa-lib", "systemd-libs", "pkgconf" };
    if (exists("/usr/bin/apt-get")) return new String[] { "apt-get", "install", "-y", "build-essential", "pkg-config", "libx11-dev", "libasound2-dev", "libudev-dev" };
    if (exists("/usr/bin/dnf")) return new String[] { "dnf", "install", "-y", "gcc", "pkgconf-pkg-config", "libX11-devel", "alsa-lib-devel", "systemd-devel" };
    if (exists("/usr/bin/zypper")) return new String[] { "zypper", "install", "-y", "gcc", "pkg-config", "libX11-devel", "alsa-devel", "libudev-devel" };
    return null;
  }

  private static void installLinuxPackages(RustMode mode) throws IOException, InterruptedException {
    String[] pkg = linuxInstall();
    if (pkg == null) throw new IOException("unknown package manager: install a C compiler and the X11, ALSA and udev development files");
    List<String> cmd = new ArrayList<>();
    cmd.add("pkexec");
    for (String s : pkg) cmd.add(s);
    mode.statusMessage("Installing " + String.join(" ", pkg) + " (asks for your password)...");
    Process p = new ProcessBuilder(cmd).redirectErrorStream(true).start();
    try (var r = new java.io.BufferedReader(new java.io.InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8))) {
      String line;
      while ((line = r.readLine()) != null) mode.statusMessage(line);
    }
    if (p.waitFor() != 0) {
      throw new IOException("couldn't install them; run: sudo " + String.join(" ", pkg));
    }
  }

  /**
   * macOS marks everything that came out of a downloaded zip as quarantined,
   * and then refuses to start the programs in it. The mode's own files are
   * cleared once, at startup.
   */
  static void clearQuarantine(File folder) {
    if (!mac() || folder == null) return;
    new Thread(() -> runs("xattr", "-dr", "com.apple.quarantine", folder.getAbsolutePath()), "rustmode-xattr").start();
  }
}
