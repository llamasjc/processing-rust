package processing.mode.rust;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Properties;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import javax.swing.JOptionPane;
import javax.swing.SwingUtilities;

/**
 * The precompiled libprocessing engine for platforms whose copy isn't in
 * RustMode.zip (it has to stay under GitHub's 2 GB per file). The release has
 * it as RustMode-libprocessing-&lt;platform&gt;.zip; when libprocessing is picked
 * and the engine isn't there, this offers to download it instead of
 * compiling it, which takes a long time on small machines.
 */
final class EngineDownload {
  private EngineDownload() {}

  private static volatile boolean declined;

  /** The release this copy of Rust Mode came from: url and prettyVersion in mode.properties. */
  static String zipUrl(File modeFolder, String platform) {
    Properties p = new Properties();
    try (InputStream in = Files.newInputStream(new File(modeFolder, "mode.properties").toPath())) {
      p.load(in);
    } catch (IOException e) {
      return null;
    }
    String url = p.getProperty("url", "").trim();
    String version = p.getProperty("prettyVersion", "").trim();
    if (!url.startsWith("https://github.com/") || version.isEmpty()) return null;
    if (url.endsWith("/")) url = url.substring(0, url.length() - 1);
    return url + "/releases/download/v" + version + "/RustMode-libprocessing-" + platform + ".zip";
  }

  /**
   * Asks once per session. Returns true when the engine was downloaded and
   * unpacked into cache/&lt;platform&gt;/libprocessing. Call off the Swing thread.
   */
  static boolean offer(RustMode mode, String platform) {
    if (declined) return false;
    String url = zipUrl(mode.getFolder(), platform);
    if (url == null) return false;
    final int[] answer = { JOptionPane.NO_OPTION };
    try {
      SwingUtilities.invokeAndWait(() -> answer[0] = JOptionPane.showOptionDialog(null,
        "The libprocessing engine for this computer (" + platform + ") isn't installed.\n\n"
        + "Download it now (about 450 MB, once), or build it here instead,\n"
        + "which can take a long time on a small machine.",
        "Rust Mode", JOptionPane.YES_NO_OPTION, JOptionPane.QUESTION_MESSAGE, null,
        new String[] { "Download", "Build it here" }, "Download"));
    } catch (Exception e) {
      return false;
    }
    if (answer[0] != JOptionPane.YES_OPTION) {
      declined = true;
      return false;
    }
    try {
      download(mode, url);
      return new File(mode.getFolder(), "cache/" + platform + "/libprocessing/cache.json").isFile();
    } catch (Exception e) {
      mode.statusMessage("Couldn't download the libprocessing engine (" + e.getMessage()
        + "). Building it here instead.");
      declined = true;
      return false;
    }
  }

  private static void download(RustMode mode, String url) throws IOException, InterruptedException {
    mode.statusMessage("Downloading the libprocessing engine...");
    Path tmp = Files.createTempFile("rustmode-engine", ".zip");
    try {
      Http.download(url, tmp.toFile(), done -> mode.statusMessage(
        "Downloading the libprocessing engine: " + (done >> 20) + " MB"));
      mode.statusMessage("Unpacking the libprocessing engine...");
      unzip(tmp, mode.getFolder().toPath());
      mode.statusMessage("libprocessing engine installed.");
    } finally {
      Files.deleteIfExists(tmp);
    }
  }

  /** The zip's entries start with RustMode/; they go into the mode folder itself. */
  private static void unzip(Path zip, Path modeFolder) throws IOException {
    Path root = modeFolder.toAbsolutePath().normalize();
    // cache.json last: it's what marks the cache as usable, so a download
    // that breaks halfway leaves nothing the build would pick up
    Path meta = null;
    byte[] metaBytes = null;
    try (ZipInputStream z = new ZipInputStream(Files.newInputStream(zip))) {
      ZipEntry e;
      while ((e = z.getNextEntry()) != null) {
        String name = e.getName().replace('\\', '/');
        int slash = name.indexOf('/');
        if (slash < 0) continue;
        name = name.substring(slash + 1);
        if (name.isEmpty()) continue;
        Path dest = root.resolve(name).normalize();
        if (!dest.startsWith(root)) throw new IOException("bad entry " + e.getName());
        if (e.isDirectory()) { Files.createDirectories(dest); continue; }
        Files.createDirectories(dest.getParent());
        if (dest.getFileName().toString().equals("cache.json")) {
          meta = dest;
          metaBytes = z.readAllBytes();
          continue;
        }
        Files.copy(z, dest, StandardCopyOption.REPLACE_EXISTING);
      }
    }
    if (meta != null) Files.write(meta, metaBytes);
  }
}
