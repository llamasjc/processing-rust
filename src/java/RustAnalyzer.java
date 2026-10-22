package processing.mode.rust;

import ch.usi.si.seart.treesitter.Language;
import ch.usi.si.seart.treesitter.Node;
import ch.usi.si.seart.treesitter.Parser;
import ch.usi.si.seart.treesitter.Tree;

import java.io.UnsupportedEncodingException;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * RustAnalyzer – walks the Tree-sitter parse tree of the concatenated sketch
 * source and produces a {@link RustAnalysis} value object that {@link RustEmitter}
 * can turn into a compilable Rust binary.
 *
 * <h2>What it discovers</h2>
 * <ul>
 *   <li><b>Sketch-level bindings</b> – top-level {@code let} / {@code let mut}
 *       statements that become fields of {@code SketchState}.</li>
 *   <li><b>Lifecycle functions</b> – {@code setup()}, {@code draw()}, and the
 *       optional event callbacks.  Their presence determines which builder calls
 *       the emitter generates.</li>
 *   <li><b>Pass-through items</b> – everything else (user structs, helpers,
 *       {@code use}, {@code const}, {@code static}, {@code type}) emitted verbatim.</li>
 * </ul>
 *
 * <h2>Tree-sitter binding</h2>
 * Uses the <a href="https://github.com/seart-group/java-tree-sitter">seart
 * java-tree-sitter</a> binding ({@code ch.usi.si.seart.treesitter.*}), 1.9.1.
 * The Rust grammar native library must be on {@code java.library.path};
 * {@code Parser.getFor(Language.RUST)} gives a parser for the grammar
 * compiled into lib/<platform>/libjava-tree-sitter by native/build.sh.
 */
public class RustAnalyzer {

  // ── Known lifecycle function names (snake_case canonical forms) ─────────────
  private static final Set<String> LIFECYCLE_FNS = Set.of(
    "setup", "draw",
    "mouse_pressed", "mouse_released", "mouse_clicked", "mouse_moved", "mouse_dragged",
    "mouse_wheel",
    "key_pressed",  "key_released",  "key_typed",
    "focus_gained", "focus_lost", "mouse_entered", "mouse_exited", "window_moved", "window_resized"
  );

  // camelCase → snake_case normalisation for lifecycle callbacks.
  // Users may write either convention; the emitter always receives snake_case.
  private static final java.util.Map<String, String> CAMEL_TO_SNAKE =
    java.util.Map.ofEntries(
      java.util.Map.entry("focusGained", "focus_gained"), java.util.Map.entry("focusLost", "focus_lost"),
      java.util.Map.entry("mouseEntered", "mouse_entered"), java.util.Map.entry("mouseExited", "mouse_exited"),
      java.util.Map.entry("windowMoved", "window_moved"), java.util.Map.entry("windowResized", "window_resized"),
      java.util.Map.entry("mousePressed", "mouse_pressed"),
      java.util.Map.entry("mouseReleased", "mouse_released"),
      java.util.Map.entry("mouseClicked", "mouse_clicked"),
      java.util.Map.entry("mouseMoved", "mouse_moved"),
      java.util.Map.entry("mouseDragged", "mouse_dragged"),
      java.util.Map.entry("mouseWheel", "mouse_wheel"),
      java.util.Map.entry("keyPressed", "key_pressed"),
      java.util.Map.entry("keyReleased", "key_released"),
      java.util.Map.entry("keyTyped", "key_typed")
    );

  // ── Fields ──────────────────────────────────────────────────────────────────

  private final String source;
  private final String sketchName;

  private final Map<String, SketchBinding> bindings       = new LinkedHashMap<>();
  private final Map<String, String>        lifecycleFns   = new LinkedHashMap<>();
  private final List<String>               passthroughItems = new ArrayList<>();
  // 1-based sketch line each item starts on (see RustAnalysis.itemLines)
  private final Map<String, Integer>       itemLines = new java.util.HashMap<>();
  /** 1-based {row, col} pairs for each ERROR / MISSING node found. */
  private final List<int[]>                parseErrorLocations = new ArrayList<>();

  // ── Construction ────────────────────────────────────────────────────────────

  public RustAnalyzer(String source) {
    this(source, "sketch");
  }

  public RustAnalyzer(String source, String sketchName) {
    this.source     = source;
    this.sketchName = sketchName;
  }

  // ── Public API ──────────────────────────────────────────────────────────────

  /** Parse and analyse the sketch source, returning an immutable analysis object. */
  public RustAnalysis analyse() throws UnsupportedEncodingException {
    if (!RustMode.isNativeLoaded())
      throw new UnsupportedEncodingException("Tree-sitter did not load, see the console. "
        + "Run native/build.sh and rebuild the jar.");
    Parser parser;
    try {
      parser = Parser.getFor(Language.RUST);
    } catch (UnsatisfiedLinkError | ch.usi.si.seart.treesitter.error.ABIVersionError e) {
        throw new UnsupportedEncodingException(
          "Tree-sitter Rust grammar not linked into libjava-tree-sitter. "
          + "Run native/build.sh rust and rebuild the jar.");
      }
    try (parser) {
      try (Tree tree = parser.parse(source)) {
        Node root = tree.getRootNode();
        walkTopLevel(root);
      }
    }
    // Always run regex fallback: catches any lifecycle functions that
    // tree-sitter missed due to a grammar/runtime version mismatch.
    // regexFallback() skips functions already found by tree-sitter.
    regexFallback();
    return new RustAnalysis(
      List.copyOf(bindings.values()),
      Map.copyOf(lifecycleFns),
      List.copyOf(passthroughItems),
      sketchName,
      List.copyOf(parseErrorLocations),
      Map.copyOf(itemLines)
    );
  }

  // ── Walk ─────────────────────────────────────────────────────────────────────

  /**
   * Recursively collect ERROR and MISSING nodes, recording their 1-based
   * {row, col} so the linter can place gutter markers at real positions.
   */
  private void checkParseErrors(Node node) {
    String type = node.getType();
    if ("ERROR".equals(type) || "MISSING".equals(type)) {
      // getStartPoint() returns a two-element int[] {row, col}, 0-based.
      parseErrorLocations.add(new int[]{ node.getStartPoint().getRow() + 1, node.getStartPoint().getColumn() + 1 }); // 1-based
      // Still recurse: a single ERROR node can contain nested errors.
    }
    for (int i = 0; i < node.getChildCount(); i++) {
      checkParseErrors(node.getChild(i));
    }
  }

  /** Iterate over the direct children of the source_file (root) node. */
  private void walkTopLevel(Node root) {
    checkParseErrors(root);
    int count = root.getChildCount();
    // Accumulate attribute_item nodes; attach them to the following item.
    List<String> pendingAttrs = new ArrayList<>();
    int attrsRow = -1;   // row of the first buffered attribute, or -1

    for (int i = 0; i < count; i++) {
      Node child = root.getChild(i);
      String type = child.getType();
      switch (type) {
        case "line_comment", "block_comment" -> {
          // Comments between items: clear the attr buffer (they break attr-to-item grouping)
          // but don't emit them as passthrough; they're already in the function slices.
          // Top-level standalone comments are silently dropped — they had no semantic role.
        }
        case "attribute_item" -> {
          // Buffer attributes — they'll be prepended to the next item's text.
          String text = sliceSource(child);
          if (!text.isBlank()) {
            if (pendingAttrs.isEmpty()) attrsRow = child.getStartPoint().getRow();
            pendingAttrs.add(text);
          }
        }
        case "let_declaration" -> {
          pendingAttrs.clear(); // attributes above top-level lets are unusual; drop them
          attrsRow = -1;
          handleTopLevelLet(child);
        }
        case "function_item" -> {
          handleFunction(child, pendingAttrs,
            (pendingAttrs.isEmpty() || attrsRow < 0 ? child.getStartPoint().getRow() : attrsRow) + 1);
          pendingAttrs.clear();
          attrsRow = -1;
        }
        default -> {
          // Anything else (use_declaration, struct_item, impl_item, const_item,
          // static_item, type_item, …) is passed through verbatim.
          String text = sliceSource(child);
          if (!text.isBlank()) {
            // Prepend any buffered attributes.
            String full = pendingAttrs.isEmpty()
              ? text
              : String.join("\n", pendingAttrs) + "\n" + text;
            passthroughItems.add(full);
            itemLines.putIfAbsent(full,
              (pendingAttrs.isEmpty() || attrsRow < 0 ? child.getStartPoint().getRow() : attrsRow) + 1);
          }
          pendingAttrs.clear();
          attrsRow = -1;
        }
      }
    }
  }

  // ── Top-level let ────────────────────────────────────────────────────────────

  /**
   * Parse a top-level {@code let [mut] name [: Type] = init;} into a
   * {@link SketchBinding}.
   *
   * Tree-sitter Rust grammar shape for let_declaration:
   * <pre>
   *   (let_declaration
   *     ["mut"] @mutable_specifier
   *     (identifier) @name
   *     [":" (type)] @type_annotation
   *     ["=" (expression)] @value)
   * </pre>
   */
  private void handleTopLevelLet(Node node) {
    boolean mutable = false;
    String  name    = null;
    String  typeAnn = null;
    String  init    = null;

    // Track whether the previous named child was "="  so we can grab
    // the following expression as the initialiser.
    boolean nextIsInit = false;

    for (int i = 0; i < node.getChildCount(); i++) {
      Node c    = node.getChild(i);
      String t  = c.getType();

      if ("mutable_specifier".equals(t)) {
        mutable = true;
      } else if ("identifier".equals(t) && name == null) {
        // The first identifier is the binding name.
        name = sliceSource(c);
      } else if ("=".equals(t)) {
        nextIsInit = true;
      } else if (nextIsInit && !";".equals(t)) {
        // Grab the whole initialiser expression as a source slice.
        init = sliceSource(c).trim();
        nextIsInit = false;
      } else if (":".equals(t)) {
        // Type annotation follows; pick it up on the next non-whitespace child.
        // We rely on the fact that the next named node after ":" is the type.
        if (i + 1 < node.getChildCount()) {
          Node typeNode = node.getChild(i + 1);
          String tt = typeNode.getType();
          if (!tt.equals("=") && !tt.equals(";")) {
            typeAnn = sliceSource(typeNode).trim();
          }
        }
      }
    }

    if (name != null) {
      bindings.put(name, new SketchBinding(name, mutable, typeAnn, init));
      itemLines.put("let:" + name, node.getStartPoint().getRow() + 1);
    }
  }

  // ── Functions ─────────────────────────────────────────────────────────────────

  /** Classify a top-level function as lifecycle or pass-through. */
  private void handleFunction(Node node, List<String> attrs, int startLine) {
    String fnName = null;
    Node   nameNode = null;
    for (int i = 0; i < node.getChildCount(); i++) {
      if ("identifier".equals(node.getChild(i).getType())) {
        nameNode = node.getChild(i);
        fnName   = sliceSource(nameNode);
        break;
      }
    }
    if (fnName == null) return;

    // Normalise camelCase lifecycle names → snake_case so the emitter always
    // receives a canonical key (e.g. "mouse_pressed", never "mousePressed").
    String canonical = CAMEL_TO_SNAKE.getOrDefault(fnName, fnName);
    boolean isLifecycle = LIFECYCLE_FNS.contains(canonical);

    String fnText = sliceSource(node);

    // If the user wrote a camelCase lifecycle name, rewrite just the identifier
    // in the source text so the emitter generates the correct Rust fn signature.
    if (!canonical.equals(fnName)) {
      // Replace the first occurrence of the camelCase name after "fn ".
      fnText = fnText.replaceFirst("\\b" + java.util.regex.Pattern.quote(fnName) + "\\b",
                                   canonical);
    }

    if (isLifecycle) {
      String full = attrs.isEmpty()
        ? fnText
        : String.join("\n", attrs) + "\n" + fnText;
      lifecycleFns.put(canonical, full);
      itemLines.put(full, startLine);
    } else {
      String full = attrs.isEmpty()
        ? fnText
        : String.join("\n", attrs) + "\n" + fnText;
      passthroughItems.add(full);
      itemLines.putIfAbsent(full, startLine);
    }
  }

  // ── Regex fallback ────────────────────────────────────────────────────────────

  /**
   * When tree-sitter produces no lifecycle functions (e.g. due to a grammar/
   * runtime version mismatch), scan the raw source with a regex and extract
   * each function body so the linter and emitter still work.
   */
  private void regexFallback() {
    // Match "fn <name> (...) { ... }" at any nesting level.
    // We extract the name and the entire function text up to a balanced closing brace.
    java.util.regex.Pattern fnPat = java.util.regex.Pattern.compile(
      "\\bfn\\s+(\\w+)\\s*\\(", java.util.regex.Pattern.MULTILINE);
    java.util.regex.Matcher m = fnPat.matcher(source);
    while (m.find()) {
      String name      = m.group(1);
      String canonical = CAMEL_TO_SNAKE.getOrDefault(name, name);
      if (LIFECYCLE_FNS.contains(canonical) && !lifecycleFns.containsKey(canonical)) {
        int    start  = m.start();
        String fnText = extractBalanced(start);
        // Normalise fn name in text if user wrote camelCase.
        if (!canonical.equals(name)) {
          fnText = fnText.replaceFirst(
            "\\b" + java.util.regex.Pattern.quote(name) + "\\b", canonical);
        }
        lifecycleFns.put(canonical, fnText);
        int line = 1;
        for (int c = 0; c < start; c++) if (source.charAt(c) == '\n') line++;
        itemLines.put(fnText, line);
      }
    }
  }

  /** Extract from {@code start} through the closing brace that balances the first '{'. */
  private String extractBalanced(int start) {
    int depth = 0;
    boolean inBrace = false;
    for (int i = start; i < source.length(); i++) {
      char c = source.charAt(i);
      if (c == '{') { depth++; inBrace = true; }
      else if (c == '}') {
        depth--;
        if (inBrace && depth == 0) return source.substring(start, i + 1);
      }
    }
    // No balanced brace found — return to end of source.
    return source.substring(start);
  }

  // ── Source slice helper ────────────────────────────────────────────────────────

  /**
   * Return the source substring covered by {@code node}.
   * seart reports offsets as char indexes into the Java String (it parses
   * UTF-16), so this is a plain substring.
   */
  private String sliceSource(Node node) {
    int start = Math.min(node.getStartByte(), source.length());
    int end   = Math.min(node.getEndByte(),   source.length());
    return source.substring(start, end);
  }
}
