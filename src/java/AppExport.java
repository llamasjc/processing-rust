package processing.mode.rust;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.stream.Stream;

/**
 * File &gt; Export Application, laid out like Processing's own export: a
 * folder named for this platform (linux-amd64, macos-aarch64,
 * windows-amd64...) inside the sketch folder, holding a launcher named after
 * the sketch, the program in lib/, and a copy of the sketch's data folder.
 * The launcher starts the program from its own folder, so loadImage("x.png")
 * and the data folder work wherever the export is moved.
 */
final class AppExport {
  private AppExport() {}

  static boolean windows() {
    return System.getProperty("os.name", "").toLowerCase().contains("win");
  }

  /**
   * Processing's name for this platform, like the Java mode's export
   * folders: linux-amd64, linux-aarch64, macos-x86_64, macos-aarch64,
   * windows-amd64. Worked out here, since Processing's own helper isn't in
   * every Processing 4 release.
   */
  static String variant(String fallback) {
    String os = System.getProperty("os.name", "").toLowerCase();
    String arch = System.getProperty("os.arch", "").toLowerCase();
    boolean arm64 = arch.equals("aarch64") || arch.equals("arm64");
    boolean x64 = arch.equals("amd64") || arch.equals("x86_64");
    if (os.contains("mac")) return "macos-" + (arm64 ? "aarch64" : "x86_64");
    String name = os.contains("win") ? "windows" : os.contains("linux") ? "linux" : null;
    if (name == null || !(arm64 || x64)) return fallback;
    return name + "-" + (arm64 ? "aarch64" : "amd64");
  }

  /** Shows a folder in the file manager; quietly does nothing where it can't. */
  static void show(File folder) {
    try {
      if (java.awt.Desktop.isDesktopSupported()) java.awt.Desktop.getDesktop().open(folder);
    } catch (Throwable ignored) {}
  }

  /**
   * Writes the export and returns its folder.
   * @param program   the executable to ship (copied into lib/)
   * @param args      what the launcher passes it, relative to the export folder
   *                  (e.g. "lib/sketch.lua"); may be empty
   * @param renderer  "libprocessing" to start on that renderer, else null
   */
  static File write(File sketchFolder, String sketchName, File dataFolder, String variant,
                    File program, String[] args, String renderer) throws IOException {
    File dest = new File(sketchFolder, variant);
    deleteTree(dest.toPath());
    File lib = new File(dest, "lib");
    if (!lib.mkdirs()) throw new IOException("can't make " + lib);

    File prog = new File(lib, program.getName());
    Files.copy(program.toPath(), prog.toPath(), StandardCopyOption.REPLACE_EXISTING);
    prog.setExecutable(true, false);

    if (dataFolder != null && dataFolder.isDirectory()) copyTree(dataFolder.toPath(), new File(dest, "data").toPath());

    String safe = sketchName.replaceAll("[^A-Za-z0-9_.-]", "_");
    if (windows()) {
      StringBuilder b = new StringBuilder("@echo off\r\ncd /d \"%~dp0\"\r\n");
      if (renderer != null) b.append("set PROCESSING_RENDERER=").append(renderer).append("\r\n");
      // hour(), minute()... in local time, as when run from the IDE
      b.append("for /f %%o in ('powershell -NoProfile -Command \"[int][TimeZoneInfo]::Local.GetUtcOffset([DateTime]::Now).TotalSeconds\"') do set PROCESSING_UTC_OFFSET=%%o\r\n");
      b.append("\"%~dp0lib\\").append(prog.getName()).append("\"");
      for (String a : args) b.append(" \"%~dp0").append(a.replace('/', '\\')).append("\"");
      b.append(" %*\r\n");
      Files.writeString(new File(dest, safe + ".bat").toPath(), b.toString(), StandardCharsets.UTF_8);
    } else {
      StringBuilder b = new StringBuilder("#!/bin/sh\n");
      b.append("# Starts ").append(sketchName).append(" from this folder, so its data/ files are found.\n");
      b.append("APPDIR=$(cd \"$(dirname \"$0\")\" && pwd -P)\n");
      b.append("cd \"$APPDIR\" || exit 1\n");
      if (renderer != null) b.append("export PROCESSING_RENDERER=").append(renderer).append("\n");
      // hour(), minute()... in local time, as when run from the IDE
      b.append("z=$(date +%z 2>/dev/null)\n");
      b.append("case \"$z\" in [+-][0-9][0-9][0-9][0-9])\n");
      b.append("  h=${z#?}; m=${h#??}; h=${h%??}; h=${h#0}; m=${m#0}\n");
      b.append("  o=$(( ${h:-0} * 3600 + ${m:-0} * 60 )); [ \"${z%????}\" = \"-\" ] && o=$(( -o ))\n");
      b.append("  export PROCESSING_UTC_OFFSET=$o;;\n");
      b.append("esac\n");
      b.append("exec \"$APPDIR/lib/").append(prog.getName()).append("\"");
      for (String a : args) b.append(" \"$APPDIR/").append(a).append("\"");
      b.append(" \"$@\"\n");
      File launcher = new File(dest, safe);
      Files.writeString(launcher.toPath(), b.toString(), StandardCharsets.UTF_8);
      launcher.setExecutable(true, false);
    }
    return dest;
  }

  /**
   * File &gt; Export as PDEZ: the sketch folder zipped next to it, as
   * <sketch>.pdez, the way Processing's Java mode makes one (entries start
   * with the sketch's folder name, so opening the file in Processing installs
   * the sketch). The platform folders Export Application makes are left out:
   * they're programs, big, and only run on one platform.
   */
  static File pdez(File sketchFolder) throws IOException {
    Path folder = sketchFolder.toPath();
    Path target = new File(sketchFolder.getPath() + ".pdez").toPath();
    Files.deleteIfExists(target);
    java.util.Set<String> skip = java.util.Set.of("linux-amd64", "linux-aarch64", "linux-arm",
        "macos-x86_64", "macos-aarch64", "windows-amd64", "windows-aarch64");
    try (java.util.zip.ZipOutputStream zs = new java.util.zip.ZipOutputStream(Files.newOutputStream(target));
         Stream<Path> all = Files.walk(folder)) {
      for (Path p : (Iterable<Path>) all::iterator) {
        if (Files.isDirectory(p)) continue;
        Path rel = folder.relativize(p);
        if (rel.getNameCount() > 1 && skip.contains(rel.getName(0).toString())) continue;
        // forward slashes on every platform, like any zip
        String name = sketchFolder.getName() + "/" + rel.toString().replace(File.separatorChar, '/');
        zs.putNextEntry(new java.util.zip.ZipEntry(name));
        Files.copy(p, zs);
        zs.closeEntry();
      }
    }
    return target.toFile();
  }

  static void copyTree(Path from, Path to) throws IOException {
    try (Stream<Path> all = Files.walk(from)) {
      for (Path p : (Iterable<Path>) all::iterator) {
        Path t = to.resolve(from.relativize(p).toString());
        if (Files.isDirectory(p)) Files.createDirectories(t);
        else Files.copy(p, t, StandardCopyOption.REPLACE_EXISTING);
      }
    }
  }

  static void deleteTree(Path p) throws IOException {
    if (!Files.exists(p)) return;
    try (Stream<Path> all = Files.walk(p)) {
      for (Path q : (Iterable<Path>) all.sorted(java.util.Comparator.reverseOrder())::iterator) Files.delete(q);
    }
  }
}
