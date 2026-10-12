package processing.mode.rust;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Lets sketches call engine functions the Processing way.
 *
 * <p>Rust has no overloading, so the engine exposes {@code fill}, {@code fill_rgb},
 * {@code fill_rgba} and so on. This pass picks the right one from the argument
 * count, so {@code fill(255, 120, 0)} becomes {@code fill_rgb(255.0, 120.0, 0.0)}.
 * It also turns bare integer literals into floats for engine calls, since Rust
 * won't pass {@code 255} to an {@code f32} parameter.
 *
 * <p>Works on text, not the parse tree, and never adds or removes newlines, so
 * error line numbers stay correct. Strings, chars and comments are skipped.
 */
final class OverloadRewriter {

  /** name -> (argument count -> engine function). */
  private static final Map<String, Map<Integer, String>> OVERLOADS = Map.ofEntries(
    Map.entry("fill",        Map.of(2, "fill_ga",       3, "fill_rgb",       4, "fill_rgba")),
    Map.entry("stroke",      Map.of(2, "stroke_ga",     3, "stroke_rgb",     4, "stroke_rgba")),
    Map.entry("background",  Map.of(2, "background_ga", 3, "background_rgb", 4, "background_rgba")),
    Map.entry("tint",        Map.of(2, "tint_ga",       3, "tint_rgb",       4, "tint_rgba")),
    Map.entry("rect",        Map.of(5, "rect_rounded", 8, "rect_radii")),
    Map.entry("filter",      Map.of(2, "filter_param")),
    Map.entry("arc",         Map.of(7, "arc_mode")),
    Map.entry("image",       Map.of(5, "image_sized")),
    Map.entry("graphics_image", Map.of(5, "graphics_image_sized")),
    Map.entry("text_font",   Map.of(2, "text_font_size")),
    Map.entry("shape",       Map.of(5, "shape_sized")),
    Map.entry("scale",       Map.of(2, "scale_xy")),
    Map.entry("translate",   Map.of(3, "translate_3d")),
    Map.entry("random",      Map.of(2, "random_range")),
    Map.entry("noise",       Map.of(2, "noise2", 3, "noise3")),
    Map.entry("begin_shape", Map.of(1, "begin_shape_kind")),
    Map.entry("end_shape",   Map.of(1, "end_shape_mode")),
    Map.entry("color",       Map.of(2, "color_ga", 3, "color_rgb", 4, "color_rgba")),
    Map.entry("color_mode",  Map.of(2, "color_mode_range", 4, "color_mode3", 5, "color_mode_max")),
    Map.entry("colorMode",   Map.of(2, "color_mode_range", 4, "color_mode3", 5, "color_mode_max")),
    Map.entry("nf",          Map.of(3, "nf_float")),
    Map.entry("nfc",         Map.of(2, "nfc_float")),
    Map.entry("text",        Map.of(5, "text_box")),
    Map.entry("box",         Map.of(1, "box_cube", 3, "box_3d")),
    Map.entry("size",        Map.of(3, "size_renderer")),
    Map.entry("sphere_detail", Map.of(2, "sphere_detail_uv")),
    Map.entry("camera",      Map.of(9, "camera_look")),
    Map.entry("perspective", Map.of(0, "perspective_default")),
    Map.entry("text_align",  Map.of(1, "text_align_h")),
    Map.entry("textAlign",   Map.of(1, "text_align_h"))
  );

  /** Method names rewritten like the free functions, for PShape. */
  private static final Set<String> METHOD_OVERLOADS = Set.of("fill", "stroke", "begin_shape", "end_shape", "scale");

  /** Float functions whose last argument is a constant (PIE, CHORD...), not a number to convert. */
  private static final Set<String> LAST_ARG_AS_IS = Set.of("arc_mode");

  /** Float functions whose first argument is an image or layer, not a number to convert. */
  private static final Set<String> FIRST_ARG_AS_IS =
    Set.of("image", "image_sized", "graphics_image", "graphics_image_sized",
           "create_font", "text_font_size", "filter_param",
           "shape", "shape_sized",
           "color_mode_range", "color_mode3", "color_mode_max", "nf_float", "nfc_float");

  /** Engine functions whose numeric parameters are all f32. */
  private static final Set<String> FLOAT_ARGS = Set.of(
    "fill", "fill_ga", "fill_rgb", "fill_rgba",
    "stroke", "stroke_ga", "stroke_rgb", "stroke_rgba",
    "background", "background_ga", "background_rgb", "background_rgba",
    "ellipse", "circle", "rect", "rect_rounded", "square", "line", "point",
    "triangle", "quad", "arc", "translate", "translate_3d", "rotate",
    "scale", "scale_xy", "stroke_weight", "text", "text_size", "frame_rate",
    "random", "random_range", "noise", "noise2",
    "vertex", "bezier_vertex", "quadratic_vertex", "curve_vertex",
    "arc_mode", "bezier", "curve", "shear_x", "shear_y", "apply_matrix",
    "tint", "tint_ga", "tint_rgb", "tint_rgba", "noise3",
    "image", "image_sized", "graphics_image", "graphics_image_sized",
    "create_font", "text_font_size", "text_leading",
    "rect_radii", "filter_param", "shape", "shape_sized",
    "color", "color_ga", "color_rgb", "color_rgba",
    "color_mode_range", "color_mode3", "color_mode_max",
    "text_box", "sq", "norm", "bezier_point", "bezier_tangent", "curve_point", "curve_tangent"
  );

  private OverloadRewriter() {}

  static String rewrite(String src) {
    StringBuilder out = new StringBuilder(src.length() + 64);
    int i = 0, n = src.length();
    while (i < n) {
      char c = src.charAt(i);
      int skip = skipNonCode(src, i);
      if (skip > i) { out.append(src, i, skip); i = skip; continue; }
      // shape.fill(255, 200, 0) -> shape.fill_rgb(...): PShape has the same
      // overloads as the drawing functions. Arguments take any number type there.
      if (Character.isJavaIdentifierStart(c) && i > 0 && src.charAt(i - 1) == '.'
          && (i < 2 || src.charAt(i - 2) != '.')) {
        int j = i;
        while (j < n && Character.isJavaIdentifierPart(src.charAt(j))) j++;
        String name = src.substring(i, j);
        int k = j;
        while (k < n && (src.charAt(k) == ' ' || src.charAt(k) == '\t')) k++;
        if (METHOD_OVERLOADS.contains(name) && k < n && src.charAt(k) == '(') {
          int close = matchParen(src, k);
          if (close > 0) {
            List<String> args = splitArgs(src.substring(k + 1, close));
            boolean empty = args.size() == 1 && args.get(0).isBlank();
            int count = empty ? 0 : args.size();
            String target = OVERLOADS.getOrDefault(name, Map.of()).getOrDefault(count, name);
            List<String> fixed = new ArrayList<>();
            for (String a : args) fixed.add(rewrite(a));
            out.append(target).append(src, j, k + 1).append(String.join(",", fixed)).append(')');
            i = close + 1;
            continue;
          }
        }
        out.append(name);
        i = j;
        continue;
      }
      if (Character.isJavaIdentifierStart(c) && (i == 0 || !isIdentOrPath(src.charAt(i - 1)))) {
        int j = i;
        while (j < n && Character.isJavaIdentifierPart(src.charAt(j))) j++;
        String name = src.substring(i, j);
        int k = j;
        while (k < n && (src.charAt(k) == ' ' || src.charAt(k) == '\t')) k++;
        boolean isCall = k < n && src.charAt(k) == '('
          && (OVERLOADS.containsKey(name) || FLOAT_ARGS.contains(name))
          && !precededByFn(src, i);
        if (isCall) {
          int close = matchParen(src, k);
          if (close > 0) {
            List<String> args = splitArgs(src.substring(k + 1, close));
            boolean empty = args.size() == 1 && args.get(0).isBlank();
            int count = empty ? 0 : args.size();
            String target = OVERLOADS.getOrDefault(name, Map.of()).getOrDefault(count, name);
            List<String> fixed = new ArrayList<>();
            for (int ai = 0; ai < args.size(); ai++) {
              String r = rewrite(args.get(ai));
              boolean asIs = (LAST_ARG_AS_IS.contains(target) && ai == args.size() - 1)
                || (FIRST_ARG_AS_IS.contains(target) && ai == 0);
              if (FLOAT_ARGS.contains(target) && !"size".equals(target) && !asIs) r = floatLiteral(r);
              fixed.add(r);
            }
            out.append(target).append(src, j, k + 1).append(String.join(",", fixed)).append(')');
            i = close + 1;
            continue;
          }
        }
        out.append(name);
        i = j;
        continue;
      }
      out.append(c);
      i++;
    }
    return out.toString();
  }

  /** {@code 255} -> {@code 255.0}, {@code -3} -> {@code -3.0}; keeps surrounding whitespace. */
  private static String floatLiteral(String arg) {
    String t = arg.strip();
    if (!t.matches("-?\\d+")) return arg;
    int lead = arg.indexOf(t);
    return arg.substring(0, lead) + t + ".0" + arg.substring(lead + t.length());
  }

  private static boolean isIdentOrPath(char c) {
    return Character.isJavaIdentifierPart(c) || c == '.' || c == ':';
  }

  private static boolean precededByFn(String s, int i) {
    int p = i - 1;
    while (p >= 0 && Character.isWhitespace(s.charAt(p))) p--;
    return p >= 1 && s.startsWith("fn", p - 1) && (p < 2 || !Character.isJavaIdentifierPart(s.charAt(p - 2)));
  }

  /** Index of the ')' matching the '(' at {@code open}, or -1. */
  private static int matchParen(String s, int open) {
    int depth = 0;
    for (int i = open; i < s.length(); ) {
      int skip = skipNonCode(s, i);
      if (skip > i) { i = skip; continue; }
      char c = s.charAt(i);
      if (c == '(' || c == '[' || c == '{') depth++;
      else if (c == ')' || c == ']' || c == '}') { if (--depth == 0) return i; }
      i++;
    }
    return -1;
  }

  /** Split on top-level commas, keeping each piece's original text. */
  private static List<String> splitArgs(String s) {
    List<String> parts = new ArrayList<>();
    int depth = 0, start = 0;
    for (int i = 0; i < s.length(); ) {
      int skip = skipNonCode(s, i);
      if (skip > i) { i = skip; continue; }
      char c = s.charAt(i);
      if (c == '(' || c == '[' || c == '{') depth++;
      else if (c == ')' || c == ']' || c == '}') depth--;
      else if (c == ',' && depth == 0) { parts.add(s.substring(start, i)); start = i + 1; }
      i++;
    }
    parts.add(s.substring(start));
    // A trailing comma leaves an empty last piece; drop it so fill(1, 2, 3,) still counts 3.
    if (parts.size() > 1 && parts.get(parts.size() - 1).isBlank()) {
      String last = parts.remove(parts.size() - 1);
      parts.set(parts.size() - 1, parts.get(parts.size() - 1) + "," + last);
    }
    return parts;
  }

  /** If a string, char literal or comment starts at {@code i}, return the index after it. */
  private static int skipNonCode(String s, int i) {
    int n = s.length();
    char c = s.charAt(i);
    if (c == '/' && i + 1 < n && s.charAt(i + 1) == '/') {
      int e = s.indexOf('\n', i);
      return e < 0 ? n : e;
    }
    if (c == '/' && i + 1 < n && s.charAt(i + 1) == '*') {
      int e = s.indexOf("*/", i + 2);
      return e < 0 ? n : e + 2;
    }
    if (c == '"') {
      int j = i + 1;
      while (j < n && s.charAt(j) != '"') j += s.charAt(j) == '\\' ? 2 : 1;
      return Math.min(j + 1, n);
    }
    if (c == '\'') {  // char literal like 'a' or '\n'; lifetimes like 'a are left alone
      if (i + 2 < n && s.charAt(i + 1) != '\\' && s.charAt(i + 2) == '\'') return i + 3;
      if (i + 3 < n && s.charAt(i + 1) == '\\') {
        int e = s.indexOf('\'', i + 2);
        if (e > 0 && e - i <= 10) return e + 1;
      }
    }
    return i;
  }
}
