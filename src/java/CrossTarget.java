package processing.mode.rust;

import java.io.File;
import java.util.concurrent.TimeUnit;

/**
 * Exporting for another platform, as C++ Mode does it: Rust compiles for the
 * other platform's target, and that platform's C linker links it (MinGW for
 * Windows, the aarch64 GCC for Linux ARM, osxcross for macOS). The CPU
 * renderer only; libprocessing also needs the other platform's system
 * libraries, so it's exported on that platform.
 */
final class CrossTarget {
  final String triple;
  final String linker;
  /** how to get the linker, shown in the dialog when it's missing */
  final String install;

  private CrossTarget(String triple, String linker, String install) {
    this.triple = triple; this.linker = linker; this.install = install;
  }

  private static boolean linux() { return System.getProperty("os.name", "").toLowerCase().contains("linux"); }
  private static boolean mac() { return System.getProperty("os.name", "").toLowerCase().contains("mac"); }

  private static String pkg(String arch, String deb, String fedora) {
    if (new File("/usr/bin/pacman").exists()) return "pacman -S " + arch;
    if (new File("/usr/bin/apt-get").exists()) return "apt install " + deb;
    if (new File("/usr/bin/dnf").exists()) return "dnf install " + fedora;
    return arch;
  }

  /** How to build for t from this computer, or null when it can't. */
  static CrossTarget to(ExportDialog.Target t) {
    if (linux() || mac()) {
      switch (t) {
        case WINDOWS_X64:
          return new CrossTarget("x86_64-pc-windows-gnu", "x86_64-w64-mingw32-gcc",
            mac() ? "brew install mingw-w64" : pkg("mingw-w64-gcc", "gcc-mingw-w64-x86-64", "mingw64-gcc"));
        case MACOS_X64:
          return new CrossTarget("x86_64-apple-darwin", mac() ? "cc" : "o64-clang", "osxcross (github.com/tpoechtrager/osxcross)");
        case MACOS_ARM64:
          return new CrossTarget("aarch64-apple-darwin", mac() ? "cc" : "oa64-clang", "osxcross (github.com/tpoechtrager/osxcross)");
        default:
      }
    }
    if (linux()) {
      if (t == ExportDialog.Target.LINUX_ARM64)
        return new CrossTarget("aarch64-unknown-linux-gnu", "aarch64-linux-gnu-gcc",
          pkg("aarch64-linux-gnu-gcc", "gcc-aarch64-linux-gnu", "gcc-aarch64-linux-gnu"));
      if (t == ExportDialog.Target.LINUX_X64)
        return new CrossTarget("x86_64-unknown-linux-gnu", "x86_64-linux-gnu-gcc",
          pkg("x86_64-linux-gnu-gcc", "gcc-x86-64-linux-gnu", "gcc-x86_64-linux-gnu"));
    }
    return null;
  }

  boolean linkerInstalled() {
    try {
      Process p = new ProcessBuilder(linker, "--version").redirectErrorStream(true).start();
      p.getInputStream().transferTo(java.io.OutputStream.nullOutputStream());
      return p.waitFor(20, TimeUnit.SECONDS) && p.exitValue() == 0;
    } catch (Exception e) {
      return false;
    }
  }

  /** Why t can't be exported from here with this renderer, or null if it can. */
  static String why(ExportDialog.Target t, boolean libprocessing) {
    if (t == ExportDialog.Target.here()) return null;
    if (libprocessing) return "libprocessing: export on that platform";
    CrossTarget c = to(t);
    if (c == null) return "export on that platform";
    if (!c.linkerInstalled()) return "needs " + c.install;
    return null;
  }

  /** Adds Rust's standard library for the target (once, quick). */
  void addRustTarget(String toolchain) throws java.io.IOException, InterruptedException {
    java.util.List<String> cmd = new java.util.ArrayList<>(java.util.List.of(RustSetup.tool("rustup"), "target", "add", triple));
    if (toolchain != null) { cmd.add("--toolchain"); cmd.add(toolchain); }
    Process p = new ProcessBuilder(cmd).redirectErrorStream(true).start();
    p.getInputStream().transferTo(java.io.OutputStream.nullOutputStream());
    if (p.waitFor() != 0) throw new java.io.IOException("rustup target add " + triple + " failed");
  }

  /** cargo's names for the linker and C compiler settings for this target. */
  String linkerVar() { return "CARGO_TARGET_" + triple.toUpperCase().replace('-', '_') + "_LINKER"; }
  String ccVar() { return "CC_" + triple.replace('-', '_'); }
}
