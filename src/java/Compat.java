package processing.mode.rust;

import java.io.File;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;

import processing.app.Base;
import processing.app.Mode;
import processing.app.ui.Editor;

/**
 * Calls whose home moved between Processing releases. Everything goes
 * through reflection, so one RustMode.jar works on 4.3, 4.5.x and newer.
 */
final class Compat {
  private Compat() {}

  /** The sketchbook folder, wherever this Processing keeps it. */
  static File sketchbookFolder(Mode mode) {
    for (String owner : new String[] { "processing.app.Base", "processing.app.Platform",
                                       "processing.app.Preferences" }) {
      try {
        Method m = Class.forName(owner).getMethod("getSketchbookFolder");
        Object f = m.invoke(null);
        if (f instanceof File file) return file;
      } catch (Throwable ignored) { }
    }
    for (String key : new String[] { "sketchbook.path.four", "sketchbook.path" }) {
      try {
        Object p = Class.forName("processing.app.Preferences").getMethod("get", String.class).invoke(null, key);
        if (p instanceof String str && !str.isEmpty()) return new File(str);
      } catch (Throwable ignored) { }
    }
    // modes/RustMode -> the sketchbook is two levels up
    if (mode != null && mode.getFolder() != null) {
      File up = mode.getFolder().getParentFile();
      if (up != null && up.getParentFile() != null) return up.getParentFile();
    }
    return new File(System.getProperty("user.home"), "sketchbook");
  }

  /** Every open editor window. */
  @SuppressWarnings("unchecked")
  static List<Editor> editors(Base base) {
    if (base == null) return List.of();
    try {
      Object r = base.getClass().getMethod("getEditors").invoke(base);
      if (r instanceof Collection<?> c) return new ArrayList<>((Collection<Editor>) c);
    } catch (Throwable ignored) { }
    for (Class<?> k = base.getClass(); k != null; k = k.getSuperclass()) {
      try {
        Field f = k.getDeclaredField("editors");
        f.setAccessible(true);
        Object r = f.get(base);
        if (r instanceof Collection<?> c) return new ArrayList<>((Collection<Editor>) c);
      } catch (Throwable ignored) { }
    }
    // Last resort: the editor windows themselves
    List<Editor> out = new ArrayList<>();
    for (java.awt.Window w : java.awt.Window.getWindows())
      if (w instanceof Editor e && w.isDisplayable()) out.add(e);
    return out;
  }
}
