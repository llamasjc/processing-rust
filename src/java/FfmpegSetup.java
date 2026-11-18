package processing.mode.rust;

import java.io.BufferedReader;
import java.io.File;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;
import java.util.regex.Pattern;
import javax.swing.JOptionPane;
import javax.swing.SwingUtilities;

/**
 * Movie and Capture play and grab video through ffmpeg. When a sketch uses
 * them and ffmpeg isn't installed, this offers to install it, the way C++
 * Mode's InstallWizard does: one dialog, then the system's own package
 * manager (Homebrew on macOS, winget on Windows, pacman, apt, dnf or zypper
 * through pkexec on Linux). Asked once per session.
 */
final class FfmpegSetup {
  private FfmpegSetup() {}

  private static volatile boolean asked;
  private static final Pattern USES_VIDEO = Pattern.compile("\\b(Movie|Capture)\\b");

  private static String os() { return System.getProperty("os.name", "").toLowerCase(); }
  private static boolean windows() { return os().contains("win"); }
  private static boolean mac() { return os().contains("mac"); }

  /** Folders ffmpeg is often in but that a desktop-started IDE may not have on PATH. */
  static List<File> extraDirs() {
    List<File> out = new ArrayList<>();
    String home = System.getProperty("user.home");
    if (mac()) { out.add(new File("/opt/homebrew/bin")); out.add(new File("/usr/local/bin")); }
    if (windows()) {
      String local = System.getenv("LOCALAPPDATA");
      if (local != null) out.add(new File(local, "Microsoft\\WinGet\\Links"));
      out.add(new File("C:\\ffmpeg\\bin"));
    }
    if (!windows()) out.add(new File(home, ".local/bin"));
    return out;
  }

  /** The folder ffmpeg was found in, if it's one PATH may be missing. */
  static File dir() {
    String exe = windows() ? "ffmpeg.exe" : "ffmpeg";
    for (File d : extraDirs()) if (new File(d, exe).isFile()) return d;
    return null;
  }

  /** Put ffmpeg's folder on the sketch's PATH. */
  static void prepare(Map<String, String> env) {
    File d = dir();
    if (d == null) return;
    String key = windows() && env.containsKey("Path") ? "Path" : "PATH";
    String path = env.getOrDefault(key, "");
    if (!path.contains(d.getAbsolutePath())) env.put(key, d.getAbsolutePath() + File.pathSeparator + path);
  }

  static boolean installed() {
    if (runs("ffmpeg", "-version")) return true;
    File d = dir();
    return d != null && runs(new File(d, windows() ? "ffmpeg.exe" : "ffmpeg").getAbsolutePath(), "-version");
  }

  /** Does any tab in the sketch folder use Movie or Capture? */
  static boolean sketchUsesVideo(File sketchFolder, String extension) {
    if (sketchFolder == null) return false;
    File[] files = sketchFolder.listFiles((d, n) -> n.endsWith(extension));
    if (files == null) return false;
    for (File f : files) {
      try {
        if (USES_VIDEO.matcher(Files.readString(f.toPath())).find()) return true;
      } catch (IOException ignored) {}
    }
    return false;
  }

  /**
   * Before a run: if the sketch uses video and ffmpeg is missing, offer to
   * install it. Never blocks the run; the sketch says so itself if ffmpeg is
   * still missing. Call off the Swing thread.
   */
  static void ensure(File sketchFolder, String extension, Consumer<String> status) {
    if (asked || !sketchUsesVideo(sketchFolder, extension) || installed()) return;
    asked = true;
    String[] cmd = installCommand();
    String how = cmd == null ? null : String.join(" ", cmd);
    final int[] answer = { JOptionPane.NO_OPTION };
    try {
      SwingUtilities.invokeAndWait(() -> {
        if (how == null) {
          JOptionPane.showMessageDialog(null,
            "This sketch uses Movie or Capture, which need ffmpeg.\n\n"
            + (mac() ? "Install Homebrew (https://brew.sh), then run: brew install ffmpeg"
                     : "Install ffmpeg from https://ffmpeg.org/download.html and put it on your PATH."),
            "ffmpeg", JOptionPane.INFORMATION_MESSAGE);
        } else {
          answer[0] = JOptionPane.showConfirmDialog(null,
            "This sketch uses Movie or Capture, which need ffmpeg.\n\nInstall it now with:\n  " + how
            + (needsPassword() ? "\n\n(asks for your password)" : ""),
            "ffmpeg", JOptionPane.YES_NO_OPTION, JOptionPane.QUESTION_MESSAGE);
        }
      });
    } catch (Exception e) {
      return;
    }
    if (how == null || answer[0] != JOptionPane.YES_OPTION) return;
    status.accept("Installing ffmpeg (a minute or two)...");
    try {
      List<String> full = new ArrayList<>();
      if (needsPassword()) full.add("pkexec");
      for (String s : cmd) full.add(s);
      Process p = new ProcessBuilder(full).redirectErrorStream(true).start();
      try (BufferedReader r = new BufferedReader(new InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8))) {
        String line;
        while ((line = r.readLine()) != null) status.accept(line);
      }
      p.waitFor();
    } catch (Exception e) {
      status.accept("Couldn't install ffmpeg: " + e.getMessage());
      return;
    }
    status.accept(installed() ? "ffmpeg is installed."
      : "ffmpeg didn't install. Try in a terminal: " + (needsPassword() ? "sudo " : "") + how);
  }

  private static boolean needsPassword() { return !mac() && !windows(); }

  /** The package manager command, or null if there isn't one we know. */
  static String[] installCommand() {
    if (mac()) {
      for (String b : new String[] { "/opt/homebrew/bin/brew", "/usr/local/bin/brew" })
        if (new File(b).isFile()) return new String[] { b, "install", "ffmpeg" };
      return null;
    }
    if (windows()) {
      if (runs("winget", "--version"))
        return new String[] { "winget", "install", "-e", "--id", "Gyan.FFmpeg",
          "--accept-source-agreements", "--accept-package-agreements" };
      return null;
    }
    if (new File("/usr/bin/pacman").exists()) return new String[] { "pacman", "-S", "--noconfirm", "--needed", "ffmpeg" };
    if (new File("/usr/bin/apt-get").exists()) return new String[] { "apt-get", "install", "-y", "ffmpeg" };
    if (new File("/usr/bin/dnf").exists()) return new String[] { "dnf", "install", "-y", "ffmpeg-free" };
    if (new File("/usr/bin/zypper").exists()) return new String[] { "zypper", "install", "-y", "ffmpeg" };
    return null;
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
}
