package processing.mode.rust;

import processing.app.syntax.JEditTextArea;
import processing.app.syntax.PdeInputHandler;
import processing.app.ui.Editor;

import javax.swing.text.BadLocationException;
import javax.swing.text.Document;
import java.awt.event.KeyEvent;

/**
 * Typing help for Rust on top of Processing's own input handler (which
 * already does auto-indent, Tab and the rest):
 *
 * <ul>
 *   <li>{@code {} closes itself when there is nothing but space or a closing
 *       bracket after the caret.</li>
 *   <li>Typing {@code }} right before a {@code }} steps over it instead of
 *       adding a second one.</li>
 *   <li>Enter between {@code {} and {@code }} opens the block: the caret
 *       lands on an indented line of its own and the closing brace moves
 *       below it.</li>
 * </ul>
 *
 * Anything that goes wrong falls back to the default handling, so a bug
 * here can never make a key stop working.
 */
public class RustInputHandler extends PdeInputHandler {

  private static final int INDENT = 4;   // Rust convention

  public RustInputHandler(Editor editor) {
    super(editor);
  }

  @Override
  public boolean handleTyped(KeyEvent e) {
    try {
      if (!e.isControlDown() && !e.isMetaDown() && !e.isAltDown()) {
        char c = e.getKeyChar();
        if (c == '{' && openBrace()) { e.consume(); return true; }
        if (c == '}' && stepOverBrace()) { e.consume(); return true; }
      }
    } catch (RuntimeException | BadLocationException ignored) {
      // fall through to the default
    }
    return super.handleTyped(e);
  }

  @Override
  public boolean handlePressed(KeyEvent e) {
    try {
      if (e.getKeyCode() == KeyEvent.VK_ENTER && e.getModifiersEx() == 0 && splitBraces()) {
        e.consume();
        return true;
      }
    } catch (RuntimeException | BadLocationException ignored) {
      // fall through to the default
    }
    return super.handlePressed(e);
  }

  private JEditTextArea area() { return editor.getTextArea(); }

  /** Is there a selection? Then typing replaces it; leave that to the default. */
  private boolean hasSelection() {
    JEditTextArea ta = area();
    return ta.getSelectionStart() != ta.getSelectionStop();
  }

  private boolean openBrace() throws BadLocationException {
    JEditTextArea ta = area();
    if (hasSelection() || !ta.isEditable()) return false;
    Document doc = ta.getDocument();
    int pos = ta.getCaretPosition();
    if (pos < doc.getLength()) {
      char next = doc.getText(pos, 1).charAt(0);
      // only pair up when nothing follows that the brace could be meant to wrap
      if (!(Character.isWhitespace(next) || next == ')' || next == ']' || next == '}' || next == ',' || next == ';')) {
        return false;
      }
    }
    doc.insertString(pos, "{}", null);
    ta.setCaretPosition(pos + 1);
    return true;
  }

  private boolean stepOverBrace() throws BadLocationException {
    JEditTextArea ta = area();
    if (hasSelection()) return false;
    Document doc = ta.getDocument();
    int pos = ta.getCaretPosition();
    if (pos < doc.getLength() && doc.getText(pos, 1).charAt(0) == '}') {
      ta.setCaretPosition(pos + 1);
      return true;
    }
    return false;
  }

  private boolean splitBraces() throws BadLocationException {
    JEditTextArea ta = area();
    if (hasSelection() || !ta.isEditable()) return false;
    Document doc = ta.getDocument();
    int pos = ta.getCaretPosition();
    if (pos < 1 || pos >= doc.getLength()) return false;
    if (doc.getText(pos - 1, 1).charAt(0) != '{' || doc.getText(pos, 1).charAt(0) != '}') return false;

    // indent of the line the caret is on
    int lineStart = pos - 1;
    while (lineStart > 0 && doc.getText(lineStart - 1, 1).charAt(0) != '\n') lineStart--;
    StringBuilder indent = new StringBuilder();
    for (int i = lineStart; i < pos; i++) {
      char c = doc.getText(i, 1).charAt(0);
      if (c == ' ' || c == '\t') indent.append(c); else break;
    }
    String inner = indent + " ".repeat(INDENT);
    doc.insertString(pos, "\n" + inner + "\n" + indent, null);
    ta.setCaretPosition(pos + 1 + inner.length());
    return true;
  }
}
