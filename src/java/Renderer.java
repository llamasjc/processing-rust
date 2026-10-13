package processing.mode.rust;

import javax.swing.JComboBox;
import java.awt.Dimension;

/**
 * Which renderer sketches run on: the built-in software one, or
 * libprocessing (Bevy + wgpu). Picked in the toolbar, remembered between
 * sessions.
 */
final class Renderer {
  private Renderer() {}

  static final String SOFTWARE = "Software";
  static final String LIBPROCESSING = "libprocessing";

  private static final java.util.prefs.Preferences PREFS =
    java.util.prefs.Preferences.userRoot().node("processing/rustmode");

  static boolean libprocessing() {
    return LIBPROCESSING.equals(PREFS.get("rust.renderer", SOFTWARE));
  }

  static void set(String name) {
    PREFS.put("rust.renderer", LIBPROCESSING.equals(name) ? LIBPROCESSING : SOFTWARE);
  }

  /** The toolbar menu. */
  static JComboBox<String> menu() {
    JComboBox<String> box = new JComboBox<>(new String[] { SOFTWARE, LIBPROCESSING });
    box.setSelectedItem(libprocessing() ? LIBPROCESSING : SOFTWARE);
    box.setToolTipText("Renderer: the built-in software one, or libprocessing (GPU, first build takes a while)");
    box.setFocusable(false);
    box.addActionListener(e -> set((String) box.getSelectedItem()));
    Dimension d = box.getPreferredSize();
    box.setMaximumSize(new Dimension(d.width, d.height));
    return box;
  }
}
