package processing.mode.rust;

import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * Rust Mode's sketch-to-crate step without the IDE, for the web editor's
 * compile server (wrust):
 *
 *   java -cp RustMode.jar:java-tree-sitter.jar processing.mode.rust.RustCli sketch.rs main.rs map.json
 *
 * Writes the main.rs the IDE would build, and map.json: the line map
 * ({main.rs line, sketch line, count} chunks) that points errors at the
 * sketch. Exit 0 on success; anything wrong goes to stderr with exit 1.
 */
public final class RustCli {
  private RustCli() {}

  public static void main(String[] args) throws Exception {
    if (args.length < 2) {
      System.err.println("usage: RustCli sketch.rs main.rs [map.json]");
      System.exit(2);
    }
    loadNative();
    String src = Files.readString(Path.of(args[0]), StandardCharsets.UTF_8)
      .replace("\r\n", "\n").replace('\r', '\n');
    if (!src.endsWith("\n")) src += "\n";
    try {
      RustAnalysis analysis = new RustAnalyzer(src, "sketch").analyse();
      RustEmitter emitter = new RustEmitter(analysis);
      String mainRs = emitter.emit();
      Files.writeString(Path.of(args[1]), mainRs, StandardCharsets.UTF_8);
      if (args.length > 2) {
        StringBuilder sb = new StringBuilder("[");
        for (int[] c : emitter.getLineMap()) {
          if (sb.length() > 1) sb.append(',');
          sb.append('[').append(c[0]).append(',').append(c[1]).append(',').append(c[2]).append(']');
        }
        Files.writeString(Path.of(args[2]), sb.append(']').toString(), StandardCharsets.UTF_8);
      }
    } catch (Exception e) {
      System.err.println(e.getMessage() != null ? e.getMessage() : e.toString());
      System.exit(1);
    }
  }

  /** The tree-sitter library packed in the jar (as RustMode does in the IDE). */
  private static void loadNative() throws Exception {
    String os = System.getProperty("os.name", "").toLowerCase();
    boolean arm = System.getProperty("os.arch", "").toLowerCase().matches(".*(aarch64|arm64).*");
    String plat, lib;
    if (os.contains("win")) { plat = "windows-x86-64"; lib = "java-tree-sitter.dll"; }
    else if (os.contains("mac")) { plat = arm ? "macos-arm64" : "macos-x86-64"; lib = "libjava-tree-sitter.dylib"; }
    else { plat = arm ? "linux-arm64" : "linux-x86-64"; lib = "libjava-tree-sitter.so"; }
    try (InputStream in = RustCli.class.getClassLoader().getResourceAsStream("lib/" + plat + "/" + lib)) {
      if (in == null) throw new IllegalStateException("lib/" + plat + "/" + lib + " is not in the jar");
      Path tmp = Path.of(System.getProperty("java.io.tmpdir"))
        .resolve("rustcli-" + System.getProperty("user.name", "user") + "-" + lib);
      Files.copy(in, tmp, StandardCopyOption.REPLACE_EXISTING);
      System.load(tmp.toAbsolutePath().toString());
    }
  }
}
