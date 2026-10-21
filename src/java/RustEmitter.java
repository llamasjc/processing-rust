package processing.mode.rust;

import ch.usi.si.seart.treesitter.Language;
import ch.usi.si.seart.treesitter.Node;
import ch.usi.si.seart.treesitter.Parser;
import ch.usi.si.seart.treesitter.Tree;

import java.io.UnsupportedEncodingException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * RustEmitter – converts a {@link RustAnalysis} into a complete, compilable
 * {@code src/main.rs} file.
 *
 * <h2>Variable rewriting</h2>
 * Uses a Tree-sitter walk of each lifecycle function body, not regex.
 * This makes the rewrite scope-aware:
 * <ul>
 *   <li>Inner {@code let} bindings that shadow a sketch variable are tracked
 *       per-block and never rewritten.</li>
 *   <li>Closure parameters are tracked the same way.</li>
 *   <li>Identifiers in field-access, pattern, and function-definition positions
 *       are left alone.</li>
 *   <li>Processing global identifiers ({@code mouseX}, {@code width}, …) that
 *       appear bare are rewritten to free-function calls ({@code mouseX()}).</li>
 * </ul>
 */
public class RustEmitter {

  // Bare identifiers that are Processing global-state readers → rewritten to fn calls.
  private static final Set<String> GLOBAL_FNS = Set.of(
    "mouseX", "mouseY", "pmouseX", "pmouseY",
    "mouse_x", "mouse_y", "pmouse_x", "pmouse_y",
    "width", "height",
    "frameCount", "frame_count",
    "millis", "focused",
    "key", "keyCode", "key_code",
    "mousePressed", "keyPressed",
    "mouse_pressed", "key_pressed"
  );

  /**
   * Names in GLOBAL_FNS whose call form would collide with a user-defined
   * lifecycle function (e.g. user writes {@code fn mousePressed()} or
   * {@code fn mouse_pressed()}, both of which end up in the generated output
   * as Rust functions).  For these we emit a fully-qualified call so the
   * function-resolution path bypasses the user's function.
   *
   * <p>The lib.rs aliases ({@code pub fn mousePressed()}) are not in scope
   * here because the sketch's lifecycle functions receive snake_case names;
   * the camelCase aliases {@code mousePressed()} / {@code keyPressed()} in
   * lib.rs could still shadow after {@code use processing::*}.  Using the
   * fully-qualified path is unambiguous in all cases.
   */
  /**
   * Is the next thing after this name an opening parenthesis? Inside a macro
   * (println!, format!) the parser sees loose tokens, not calls, so
   * {@code mouse_x()} written there looks like a bare {@code mouse_x} and
   * would be turned into {@code mouse_x()()}.
   */
  private static boolean followedByParen(Node node, char[] src) {
    int i = node.getEndByte();
    while (i < src.length && Character.isWhitespace(src[i])) i++;
    return i < src.length && src[i] == '(';
  }

  private static final Map<String, String> GLOBAL_FN_REMAP = Map.of(
    "mousePressed",  "processing::mouse_pressed_global",
    "mouse_pressed", "processing::mouse_pressed_global",
    "keyPressed",    "processing::key_pressed_global",
    "key_pressed",   "processing::key_pressed_global"
  );

  private final RustAnalysis analysis;
  private int headerLineCount = 0;

  public RustEmitter(RustAnalysis analysis) { this.analysis = analysis; }

  public int getHeaderLineCount() { return headerLineCount; }

  // The generated file is not the sketch with a header on top: globals move
  // into a struct, helper items come first, functions are reordered. So each
  // chunk copied from the sketch records where it landed:
  //   { first line in main.rs, first line in the sketch, number of lines }
  // (all 1-based). RustBuild uses it to point errors at the real sketch line.
  private final java.util.List<int[]> lineMap = new java.util.ArrayList<>();

  public java.util.List<int[]> getLineMap() { return lineMap; }

  /** Call just before appending {@code text}, which starts at sketch line {@code sketchLine}. */
  private void mapChunk(StringBuilder sb, String text, Integer sketchLine) {
    if (sketchLine == null || sketchLine < 1) return;
    String t = text.endsWith("\n") ? text.substring(0, text.length() - 1) : text;
    lineMap.add(new int[] { countLines(sb.toString()) + 1, sketchLine, countLines(t) + 1 });
  }

  // ── Top-level emit ────────────────────────────────────────────────────────────

  public String emit() throws UnsupportedEncodingException {
    StringBuilder sb = new StringBuilder();

    line(sb, "#![allow(unused_imports, unused_variables, dead_code)]");
    line(sb, "use processing::*;");
    line(sb, "");

    if (!analysis.passthroughItems().isEmpty()) {
      line(sb, "// ── user-defined items ───────────────────────────────────────────────────────");
      for (String item : analysis.passthroughItems()) {
        mapChunk(sb, item, analysis.itemLines().get(item));
        sb.append(item);
        if (!item.endsWith("\n")) sb.append('\n');
        line(sb, "");
      }
    }

    emitSketchState(sb);

    // headerLineCount = everything ABOVE the first lifecycle function.
    // rustc line numbers are absolute; we subtract this to get sketch-relative lines.
    headerLineCount = countLines(sb.toString());

    emitLifecycleFns(sb);
    emitMain(sb);
    String generated = sb.toString();
    String rewritten = OverloadRewriter.rewrite(generated);
    // the map is only good if the overload rewrite left every line where it was
    if (countLines(rewritten) != countLines(generated)) lineMap.clear();
    return rewritten;
  }

  // ── SketchState ───────────────────────────────────────────────────────────────

  private void emitSketchState(StringBuilder sb) {
    line(sb, "// ── SketchState ──────────────────────────────────────────────────────────────");
    line(sb, "struct SketchState {");
    for (SketchBinding b : analysis.bindings()) {
      String t = b.typeAnn() != null ? b.typeAnn() : inferType(b.init());
      if (t != null) {
        line(sb, "    " + b.name() + ": " + t + ",");
      } else {
        line(sb, "    // TODO: give it a type, like: let " + b.name() + ": YourType = ...;");
        line(sb, "    " + b.name() + ": (),");
      }
    }
    line(sb, "}");
    line(sb, "");
    line(sb, "impl SketchState {");
    line(sb, "    fn new() -> Self {");
    for (SketchBinding b : analysis.bindings()) {
      String kw   = b.mutable() ? "let mut " : "let ";
      // Always emit the type when known so `Default::default()` can be inferred.
      String fieldType = b.typeAnn() != null ? b.typeAnn() : inferType(b.init());
      String tp   = fieldType != null ? ": " + fieldType : "";
      String init = b.init() != null ? b.init() : "Default::default()";
      mapChunk(sb, "x", analysis.itemLines().get("let:" + b.name()));
      line(sb, "        " + kw + b.name() + tp + " = " + init + ";");
    }
    line(sb, "        Self {");
    for (SketchBinding b : analysis.bindings()) line(sb, "            " + b.name() + ",");
    line(sb, "        }");
    line(sb, "    }");
    line(sb, "}");
    line(sb, "");
  }

  // ── Lifecycle fns ─────────────────────────────────────────────────────────────

  private void emitLifecycleFns(StringBuilder sb) throws UnsupportedEncodingException {
    line(sb, "// ── sketch lifecycle ─────────────────────────────────────────────────────────");

    // If the user didn't write setup(), synthesise a no-op so main() compiles.
    if (!analysis.hasSetup()) {
      line(sb, "fn setup(__sketch: &mut SketchState) {}");
      line(sb, "");
    }
    // draw() is always required by the App builder; synthesise a no-op if missing.
    if (!analysis.hasDraw()) {
      line(sb, "fn draw(__sketch: &mut SketchState) {}");
      line(sb, "");
    }

    for (Map.Entry<String, String> e : analysis.lifecycleFns().entrySet()) {
      String out = transformFn(e.getKey(), e.getValue());
      // only when the rewrite kept the function's line structure
      if (countLines(out.strip()) == countLines(e.getValue().strip())) {
        mapChunk(sb, out, analysis.itemLines().get(e.getValue()));
      }
      sb.append(out);
      if (!out.endsWith("\n")) sb.append('\n');
      line(sb, "");
    }
  }

  // ── main ─────────────────────────────────────────────────────────────────────

  private void emitMain(StringBuilder sb) {
    line(sb, "// ── entry point ──────────────────────────────────────────────────────────────");
    line(sb, "fn main() {");
    line(sb, "    processing::App::new(SketchState::new(), setup, draw)");
    // Sketch name is baked in at emit-time; the Cargo project name is always "sketch"
    // so the window title comes from here.
    line(sb, "        .title(\"" + analysis.sketchName().replace("\\", "\\\\").replace("\"", "\\\"") + "\")");
    if (analysis.hasMousePressed())  line(sb, "        .mouse_pressed(mouse_pressed)");
    if (analysis.hasMouseReleased()) line(sb, "        .mouse_released(mouse_released)");
    if (analysis.hasMouseClicked())  line(sb, "        .mouse_clicked(mouse_clicked)");
    if (analysis.hasMouseMoved())    line(sb, "        .mouse_moved(mouse_moved)");
    if (analysis.hasMouseDragged())  line(sb, "        .mouse_dragged(mouse_dragged)");
    if (analysis.hasMouseWheel())    line(sb, "        .mouse_wheel(mouse_wheel)");
    if (analysis.hasKeyPressed())    line(sb, "        .key_pressed(key_pressed)");
    if (analysis.hasKeyReleased())   line(sb, "        .key_released(key_released)");
    if (analysis.hasKeyTyped())      line(sb, "        .key_typed(key_typed)");
    line(sb, "        .run();");
    line(sb, "}");
  }

  // ── Tree-sitter body transform ────────────────────────────────────────────────

  /**
   * Inject the state parameter into the signature, then walk the body with
   * Tree-sitter to rewrite sketch variable references and bare global names.
   *
   * <p>Most lifecycle functions take no parameters in the sketch and become
   * {@code fn name(__sketch: &mut SketchState)}.
   *
   * <p>{@code mouse_wheel} is the exception: the user writes
   * {@code fn mouse_wheel(delta: f32)} and we inject the state param before it,
   * giving {@code fn mouse_wheel(__sketch: &mut SketchState, delta: f32)}.
   * The {@link App} builder expects {@code fn(&mut S, f32)}.
   */
  private String transformFn(String fnName, String src)
      throws UnsupportedEncodingException {

    if ("mouse_wheel".equals(fnName)) {
      // Match fn mouse_wheel( <anything> ) and prepend the state param.
      // The user-written param (typically "delta: f32") is preserved after the comma.
      java.util.regex.Matcher mwMatcher = java.util.regex.Pattern
          .compile("\\bfn\\s+mouse_wheel\\s*\\(([^)]*)\\)")
          .matcher(src);
      if (mwMatcher.find()) {
        String existing = mwMatcher.group(1).trim();
        // Normalise: if the user wrote a param name, use it; otherwise synthesise "_delta".
        String paramName = "delta";
        if (!existing.isEmpty()) {
          // e.g. "delta: f32" → first token is the name
          String[] parts = existing.split("[:\\s]+");
          if (parts.length > 0 && !parts[0].isEmpty()) paramName = parts[0];
        } else {
          // User wrote fn mouse_wheel() — add the param for them.
          existing = paramName + ": f32";
        }
        String params = "__sketch: &mut SketchState, " + existing;
        src = mwMatcher.replaceFirst("fn mouse_wheel(" + params + ")");

        // Inject processing::__set_wheel_delta(<paramName>); at the top of the block
        // so mouse_wheel_delta() works from draw() or anywhere else in the frame.
        // We replace the first "{" that opens the function body.
        int braceIdx = src.indexOf('{', src.indexOf("fn mouse_wheel("));
        if (braceIdx >= 0) {
          src = src.substring(0, braceIdx + 1)
              + "\n    processing::__set_wheel_delta(" + paramName + ");"
              + src.substring(braceIdx + 1);
        }
      }
    } else {
      // All other lifecycle functions: replace empty param list with the state param.
      src = src.replaceFirst(
        "\\bfn\\s+" + java.util.regex.Pattern.quote(fnName) + "\\s*\\(\\s*\\)",
        "fn " + fnName + "(__sketch: &mut SketchState)"
      );
    }

    char[] srcChars = src.toCharArray();

    try (Parser parser = Parser.getFor(Language.RUST)) {
      try (Tree tree = parser.parse(src)) {
        Node root = tree.getRootNode();

        // Find the function_item then its block body.
        Node body = findBlock(root);
        if (body == null) return src;

        List<Rewrite> rewrites = new ArrayList<>();
        walkBlock(body, null, srcChars, rewrites, new HashSet<>());

        // Apply back-to-front so offsets stay valid (char indexes, not bytes).
        rewrites.sort((a, b) -> Integer.compare(b.start, a.start));
        for (Rewrite rw : rewrites) {
          src = src.substring(0, rw.start()) + rw.replacement() + src.substring(rw.end());
        }
      }
    }
    return src;
  }

  /** Find the block node inside the first function_item under root. */
  private static Node findBlock(Node root) {
    for (int i = 0; i < root.getChildCount(); i++) {
      Node child = root.getChild(i);
      if ("function_item".equals(child.getType())) {
        for (int j = 0; j < child.getChildCount(); j++) {
          if ("block".equals(child.getChild(j).getType())) return child.getChild(j);
        }
      }
    }
    return null;
  }

  // ── Recursive body walker ─────────────────────────────────────────────────────

  /**
   * Walk {@code node}, collecting {@link Rewrite} entries.
   * {@code shadowed} is the set of locally-bound names visible at this point.
   */
  private void walkBlock(Node node, Node parent, char[] src,
                         List<Rewrite> rewrites, Set<String> shadowed) {
    String type = node.getType();

    switch (type) {
      case "let_declaration" -> {
        // `let x1 = r1;`: only what comes before `=` is the pattern. The value
        // is walked first, with the old names, so `let x = x + 1` still reads
        // the sketch variable; then the new names hide sketch variables.
        Node pattern = null;
        boolean afterEq = false;
        for (int i = 0; i < node.getChildCount(); i++) {
          Node c = node.getChild(i);
          String ct = c.getType();
          if ("=".equals(ct)) { afterEq = true; continue; }
          if (!afterEq && pattern == null && !"let".equals(ct) && !"mutable_specifier".equals(ct)
              && !":".equals(ct) && !";".equals(ct)) {
            pattern = c;
            continue;
          }
          // The value (and `else` block): a bare name directly here is a use, not a binding.
          if (afterEq) walkBlock(c, null, src, rewrites, shadowed);
        }
        if (pattern != null) collectPatternIdents(pattern, src, shadowed);
      }

      case "identifier" -> {
        String name = slice(node, src);
        if (GLOBAL_FNS.contains(name) && !isCallTarget(node, parent) && !isPattern(node, parent)
            && !followedByParen(node, src)) {
          // Use fully-qualified form for names that may collide with a user
          // lifecycle function (e.g. mousePressed, keyPressed, mouse_pressed, key_pressed).
          String callee = GLOBAL_FN_REMAP.getOrDefault(name, name);
          rewrites.add(new Rewrite(node.getStartByte(), node.getEndByte(), callee + "()"));
        } else if (isSketchVar(name) && !shadowed.contains(name)
                   && !isFieldAccess(node, parent) && !isPattern(node, parent) && !isCallTarget(node, parent)) {
          rewrites.add(new Rewrite(node.getStartByte(), node.getEndByte(), "__sketch." + name));
        }
      }

      case "block" -> {
        // New nested scope — snapshot and restore shadowed.
        Set<String> inner = new HashSet<>(shadowed);
        for (int i = 0; i < node.getChildCount(); i++)
          walkBlock(node.getChild(i), node, src, rewrites, inner);
      }

      case "closure_expression" -> {
        Set<String> inner = new HashSet<>(shadowed);
        collectClosureParams(node, src, inner);
        for (int i = 0; i < node.getChildCount(); i++)
          walkBlock(node.getChild(i), node, src, rewrites, inner);
      }

      case "for_expression" -> {
        // Shadow all loop-pattern variables inside the for body.
        Set<String> inner = new HashSet<>(shadowed);
        // The pattern is the first child of for_expression (before `in`).
        // Collect all identifiers from it recursively.
        if (node.getChildCount() > 0) {
          collectPatternIdents(node.getChild(0), src, inner);
        }
        // Walk all children; the pattern itself is in pattern-position (handled by isPattern).
        for (int i = 0; i < node.getChildCount(); i++)
          walkBlock(node.getChild(i), node, src, rewrites, inner);
      }

      default -> {
        for (int i = 0; i < node.getChildCount(); i++)
          walkBlock(node.getChild(i), node, src, rewrites, shadowed);
      }
    }
  }

  // ── Context predicates ────────────────────────────────────────────────────────

  private static boolean isFieldAccess(Node node, Node parent) {
    if (parent == null) return false;
    // In `cfg.set(..)` or `ball.x` the name on the left is the receiver and
    // still a sketch variable; only a name to the right of the dot is a field.
    if ("field_expression".equals(parent.getType()))
      return parent.getChildCount() == 0 || parent.getChild(0).getStartByte() != node.getStartByte();
    return "scoped_identifier".equals(parent.getType());
  }

  private static boolean isPattern(Node node, Node parent) {
    if (parent == null) return false;
    return switch (parent.getType()) {
      case "let_declaration", "parameter", "tuple_pattern",
           "struct_pattern", "identifier_pattern",
           "captured_pattern", "match_arm",
           "ref_pattern", "mut_pattern",
           // `for <pat> in` — the bound variable is in pattern position
           "for_expression",
           // `if let` / `while let` patterns
           "let_condition",
           // fn parameter lists and generic params
           "function_parameters" -> true;
      default -> false;
    };
  }

  private static boolean isCallTarget(Node node, Node parent) {
    if (parent == null) return false;
    if ("call_expression".equals(parent.getType()))
      return parent.getChildCount() > 0 && parent.getChild(0).getStartByte() == node.getStartByte();
    return "function_item".equals(parent.getType());
  }

  // ── Helpers ───────────────────────────────────────────────────────────────────

  private static String firstIdentifier(Node node, char[] src) {
    for (int i = 0; i < node.getChildCount(); i++) {
      Node c = node.getChild(i);
      if ("identifier".equals(c.getType())) return slice(c, src);
    }
    return null;
  }

  /**
   * Recursively collect all identifier names from a pattern node
   * (identifier, tuple_pattern, struct_pattern, etc.) into {@code out}.
   * Used to shadow loop variables for {@code for} expressions.
   */
  private static void collectPatternIdents(Node node, char[] src, Set<String> out) {
    String t = node.getType();
    if ("identifier".equals(t)) {
      out.add(slice(node, src));
    } else {
      // Recurse into complex patterns (tuple, struct, ref, …)
      for (int i = 0; i < node.getChildCount(); i++)
        collectPatternIdents(node.getChild(i), src, out);
    }
  }

  private static void collectClosureParams(Node closure, char[] src, Set<String> out) {
    for (int i = 0; i < closure.getChildCount(); i++) {
      Node c = closure.getChild(i);
      if ("closure_parameters".equals(c.getType())) {
        for (int j = 0; j < c.getChildCount(); j++) {
          Node p = c.getChild(j);
          if ("identifier".equals(p.getType())) out.add(slice(p, src));
        }
      }
    }
  }

  private boolean isSketchVar(String name) {
    for (SketchBinding b : analysis.bindings()) if (b.name().equals(name)) return true;
    return false;
  }

  private static String slice(Node n, char[] src) {
    int s = Math.min(n.getStartByte(), src.length);
    int e = Math.min(n.getEndByte(),   src.length);
    return new String(src, s, e - s);
  }

  private static void line(StringBuilder sb, String s) { sb.append(s).append('\n'); }

  private static int countLines(String s) {
    int n = 0; for (int i = 0; i < s.length(); i++) if (s.charAt(i) == '\n') n++; return n;
  }

  private static final Map<String, String> KNOWN_RESULT = Map.ofEntries(
    Map.entry("loadJSONObject", "Json"), Map.entry("loadJSONArray", "Json"),
    Map.entry("parseJSONObject", "Json"), Map.entry("load_json", "Json"),
    Map.entry("load_json_object", "Json"), Map.entry("load_json_array", "Json"),
    Map.entry("parse_json_object", "Json"), Map.entry("parse_json_array", "Json"),
    Map.entry("parseJSONArray", "Json"),
    Map.entry("loadTable", "Table"), Map.entry("load_table", "Table"),
    Map.entry("loadStrings", "Vec<String>"), Map.entry("load_strings", "Vec<String>"),
    Map.entry("load_text", "String"), Map.entry("loadBytes", "Vec<u8>"),
    Map.entry("load_bytes", "Vec<u8>"),
    Map.entry("load_image", "u64"), Map.entry("loadImage", "u64"),
    Map.entry("load_shape", "u64"), Map.entry("loadShape", "u64"),
    Map.entry("load_font", "u64"),
    Map.entry("create_font", "u64"), Map.entry("createFont", "u64"),
    Map.entry("create_graphics", "u64"), Map.entry("createGraphics", "u64"),
    Map.entry("create_image", "u64"), Map.entry("createImage", "u64"),
    Map.entry("get_region", "u64"), Map.entry("image_copy", "u64"), Map.entry("image_crop", "u64"),
    Map.entry("color", "Color"), Map.entry("color_rgb", "Color"), Map.entry("color_rgba", "Color"),
    Map.entry("color_ga", "Color"), Map.entry("hex_color", "Color"), Map.entry("lerp_color", "Color"),
    Map.entry("create_shape", "PShape"), Map.entry("createShape", "PShape"),
    Map.entry("create_shape_group", "PShape"), Map.entry("create_shape_rect", "PShape"),
    Map.entry("create_shape_ellipse", "PShape"), Map.entry("create_shape_line", "PShape"),
    Map.entry("create_shape_triangle", "PShape"), Map.entry("create_shape_quad", "PShape"),
    Map.entry("wrap_text", "Vec<String>"),
    Map.entry("load_shader", "PShader"), Map.entry("loadShader", "PShader"), Map.entry("load_shader_vf", "PShader"),
    Map.entry("shader_from_source", "PShader"),
    Map.entry("load_xml", "Option<Xml>"), Map.entry("loadXML", "Option<Xml>"),
    Map.entry("parse_xml", "Result<Xml, String>"), Map.entry("parseXML", "Option<Xml>"),
    Map.entry("create_writer", "PrintWriter"), Map.entry("createWriter", "PrintWriter"),
    Map.entry("create_reader", "Reader"), Map.entry("createReader", "Reader"),
    Map.entry("request_image", "u64"), Map.entry("requestImage", "u64"),
    Map.entry("random", "f32"), Map.entry("noise", "f32"), Map.entry("millis", "u64"));

  /** Index of c outside brackets and parentheses, or -1. */
  private static int topLevel(String s, char c) {
    int depth = 0;
    for (int i = 0; i < s.length(); i++) {
      char ch = s.charAt(i);
      if (ch == '(' || ch == '[' || ch == '{') depth++;
      else if (ch == ')' || ch == ']' || ch == '}') depth--;
      else if (ch == c && depth == 0) return i;
    }
    return -1;
  }

  static String inferType(String init) {
    if (init == null) return null;
    init = init.trim();
    if (init.matches("-?\\d+_i8"))               return "i8";
    if (init.matches("-?\\d+_i16"))              return "i16";
    if (init.matches("-?\\d+(_i32)?"))           return "i32";
    if (init.matches("-?\\d+_i64"))              return "i64";
    if (init.matches("-?\\d+_u8"))               return "u8";
    if (init.matches("-?\\d+_u16"))              return "u16";
    if (init.matches("-?\\d+_u32"))              return "u32";
    if (init.matches("-?\\d+_u64"))              return "u64";
    if (init.matches("-?\\d+_usize"))            return "usize";
    if (init.matches("-?\\d+_isize"))            return "isize";
    // Integer with _f32 / _f64 suffix (e.g. 0_f32)
    if (init.matches("-?\\d+_f32"))              return "f32";
    if (init.matches("-?\\d+_f64"))              return "f64";
    // Decimal literals
    if (init.matches("-?\\d*\\.\\d*(_f32)?"))    return "f32";
    if (init.matches("-?\\d*\\.\\d*_f64"))       return "f64";
    if (init.equals("true") || init.equals("false")) return "bool";
    // String types
    if (init.startsWith("\""))                   return "&'static str";
    if (init.matches("String::from\\(.*\\)")
        || init.matches("format!\\(.*\\)")
        || init.equals("String::new()"))         return "String";
    if (init.startsWith("'") && init.length() == 3)  return "char";
    // Vec
    // 0.0f32, 3u8, 2_i64: a literal with its type written on it
    java.util.regex.Matcher lit = java.util.regex.Pattern
        .compile("-?[\\d_]*\\.?[\\d_]*_?(f32|f64|i8|i16|i32|i64|isize|u8|u16|u32|u64|usize)").matcher(init);
    if (lit.matches() && init.matches("-?\\d.*")) return lit.group(1);
    // Vec::<f32>::new(), vec![0.0; 64], vec![1, 2, 3]: the element type when it can be told
    java.util.regex.Matcher turbo = java.util.regex.Pattern.compile("Vec::<(.+)>::(new|with_capacity)\\(.*\\)").matcher(init);
    if (turbo.matches()) return "Vec<" + turbo.group(1) + ">";
    if (init.startsWith("vec![") && init.endsWith("]")) {
      String inner = init.substring(5, init.length() - 1).trim();
      int semi = topLevel(inner, ';');
      String first = semi >= 0 ? inner.substring(0, semi) : inner.split(",")[0];
      String et = inferType(first.trim());
      if (et != null && !et.contains("_")) return "Vec<" + et + ">";
      return "Vec<_>";
    }
    if (init.equals("Vec::new()"))               return "Vec<_>";
    // Engine calls with a known result type.
    String call = init.replaceAll("\\s+", "");
    int paren = call.indexOf('(');
    String fn = paren > 0 ? call.substring(0, paren) : call;
    String known = KNOWN_RESULT.get(fn);
    if (known != null && paren > 0 && call.endsWith(")")) return known;
    // Type::new(..), Type::Variant, Type::from(..): the type is the path head.
    java.util.regex.Matcher m = java.util.regex.Pattern
        .compile("([A-Z][A-Za-z0-9_]*)::[a-zA-Z_][A-Za-z0-9_]*(\\(.*\\))?").matcher(call);
    if (m.matches() && !m.group(1).equals("Vec") && !m.group(1).equals("String")
        && !m.group(1).equals("Self")) return m.group(1);
    return null;
  }

  private record Rewrite(int start, int end, String replacement) {}
}
