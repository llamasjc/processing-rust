package processing.mode.rust;

import ch.usi.si.seart.treesitter.Language;
import ch.usi.si.seart.treesitter.Node;
import ch.usi.si.seart.treesitter.Query;
import ch.usi.si.seart.treesitter.QueryCapture;
import ch.usi.si.seart.treesitter.QueryCursor;
import ch.usi.si.seart.treesitter.QueryMatch;

import processing.app.syntax.SyntaxDocument;
import processing.app.syntax.Token;
import processing.app.syntax.TokenMarker;

import javax.swing.text.Segment;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Async tree-sitter token marker for Rust.
 *
 * Architecture mirrors CppMode's TsTokenMarker:
 *   • A background scheduler debounces source changes (150 ms).
 *   • {@link #rebuildHighlights()} parses the tab and runs
 *     the highlights query, building a {@code byte[]} parallel to the source.
 *   • {@link #markTokensImpl} walks that byte[] on the EDT per rendered line.
 */
public class TsRustTokenMarker extends TokenMarker {

    // ── State ─────────────────────────────────────────────────────────────
    private static final byte[] EMPTY        = new byte[0];
    private static final int[]  EMPTY_LINES  = new int[0];

    /** Parallel to the source: one token-type byte per source character. */
    private final AtomicReference<byte[]> highlights  = new AtomicReference<>(EMPTY);
    /**
     * Line-start byte offsets, precomputed alongside {@link #highlights}.
     * {@code lineOffsets[i]} is the index of the first byte of line {@code i}
     * in the source (0-based). Replaced atomically with {@link #highlights}
     * (both fields are written under {@code this}, read without locks in
     * {@link #markTokensImpl} — the worst case is a stale read which just
     * shows the previous highlight, not a crash).
     */
    private volatile int[]      lineOffsets  = EMPTY_LINES;
    private volatile String     highlightedSource = "";
    private volatile SyntaxDocument  syntaxDoc;

    private volatile Runnable repaintHook;

    // ── Background scheduler ──────────────────────────────────────────────
    // One thread for every editor and tab. Parsing is quick, and a single
    // thread means the shared query below is never used from two at once.
    private static final ScheduledExecutorService scheduler = newScheduler();
    private volatile ScheduledFuture<?> pending;
    private volatile boolean disposed;
    private static final long DEBOUNCE_MS = 150;

    // ── Cached query ──────────────────────────────────────────────────────
    // Shared by all markers and never closed: closing it when one editor
    // window shuts would pull native memory out from under a parse still
    // running for another.
    private static volatile Query tsQuery;
    private static volatile boolean queryFailed;

    // java-tree-sitter 1.9.1 does not evaluate query predicates, so a pattern
    // like ((identifier) @function.builtin (#match? @function.builtin "^(fill|rect)$"))
    // matches EVERY identifier. We read the #match? regexes out of the .scm
    // ourselves and check captured text against them. Capture name -> regexes.
    private static volatile Map<String, List<Pattern>> matchPredicates = Map.of();
    private static final Pattern MATCH_PREDICATE =
        Pattern.compile("\\(#match\\?\\s+@([\\w.]+)\\s+\"((?:[^\"\\\\]|\\\\.)*)\"\\s*\\)");

    public TsRustTokenMarker() {}

    private static ScheduledExecutorService newScheduler() {
        ScheduledThreadPoolExecutor ex =
            new ScheduledThreadPoolExecutor(1, r -> {
                Thread t = new Thread(r, "rust-ts-highlighter");
                t.setDaemon(true);
                return t;
            });
        ex.setRemoveOnCancelPolicy(true);
        return ex;
    }

    // ── TokenMarker overrides ─────────────────────────────────────────────

    @Override
    public void addColoring(String keyword, String type) {
        // no-op: tree-sitter drives all coloring
    }

    @Override
    public byte markTokensImpl(byte token, Segment line, int lineIndex) {
        byte[] hl      = highlights.get();
        int[]  offsets = lineOffsets;   // read once; consistent snapshot with hl

        if (hl.length == 0 || offsets.length == 0) {
            addToken(line.count, Token.NULL);
            return Token.NULL;
        }

        // O(1) line-start lookup using the precomputed offset table.
        int lineStart = (lineIndex < offsets.length) ? offsets[lineIndex] : hl.length;

        // Walk the highlight array for each character on this line.
        int  pos     = lineStart;
        int  start   = 0;
        byte lastTok = Token.NULL;

        for (int col = 0; col < line.count; col++, pos++) {
            byte tok = (pos < hl.length) ? hl[pos] : Token.NULL;
            if (tok != lastTok) {
                if (col > start) addToken(col - start, lastTok);
                start   = col;
                lastTok = tok;
            }
        }
        addToken(line.count - start, lastTok);
        return Token.NULL;
    }

    // ── Repaint ───────────────────────────────────────────────────────────

    /**
     * Called (on the highlighter thread) after new highlights are ready. The
     * document doesn't change when colors do, so without this the text area
     * wouldn't repaint until something else made it.
     */
    public void setRepaintHook(Runnable hook) {
        this.repaintHook = hook;
    }

    // ── Invalidation / rebuild ────────────────────────────────────────────

    public synchronized void invalidate(SyntaxDocument doc) {
        if (disposed) return;
        this.syntaxDoc = doc;
        if (pending != null) pending.cancel(false);
        try {
            pending = scheduler.schedule(this::rebuildSafely, DEBOUNCE_MS, TimeUnit.MILLISECONDS);
        } catch (java.util.concurrent.RejectedExecutionException ignored) {}
    }

    // An exception escaping a scheduled task is swallowed without a trace, and
    // an Error (a native library that failed to load, say) would do the same
    // on every keystroke. Report the first one, then stay quiet.
    private static volatile boolean reportedFailure;

    private void rebuildSafely() {
        try {
            rebuildHighlights();
        } catch (Throwable t) {
            if (!reportedFailure) {
                reportedFailure = true;
                System.err.println("[RustMode] syntax highlighting stopped working: " + t);
            }
        }
    }

    private void rebuildHighlights() {
        SyntaxDocument doc = syntaxDoc;
        if (doc == null || disposed) return;
        // without the native library there is nothing to parse with
        if (!RustMode.isNativeLoaded()) return;

        String src;
        try {
            src = doc.getText(0, doc.getLength());
        } catch (Exception e) {
            return;
        }

        Query q = ensureQuery();
        if (q == null) return;

        ch.usi.si.seart.treesitter.Parser parser = parser();
        if (parser == null) return;
        try (ch.usi.si.seart.treesitter.Tree tree = parser.parse(src)) {
            Node root = tree.getRootNode();

            byte[] hl   = new byte[src.length()];
            byte[] rank = new byte[src.length()];   // priority of what's painted at each char
            Map<String, List<Pattern>> preds = matchPredicates;
            // every identifier is offered to every name-list pattern, so the
            // same few names get checked over and over: remember the answers
            Map<String, Boolean> verdicts = new HashMap<>();

            try (QueryCursor cursor = root.walk(q)) {
                for (QueryMatch match : cursor) {
                    for (java.util.Map.Entry<ch.usi.si.seart.treesitter.Capture,
                            java.util.Collection<Node>> entry
                            : match.getCaptures().entrySet()) {
                        String captureName = entry.getKey().getName();
                        byte   tokType     = highlightNameToToken(captureName);
                        if (tokType == Token.NULL) continue;
                        byte   r           = captureRank(captureName);
                        List<Pattern> regexes = preds.get(captureName);
                        for (Node node : entry.getValue()) {
                            // offsets are Java char indexes (the parser reads UTF-16)
                            int start = Math.max(0, Math.min(node.getStartByte(), src.length()));
                            int end   = Math.max(start, Math.min(node.getEndByte(), src.length()));
                            if (regexes != null) {
                                String text = src.substring(start, end);
                                boolean ok = verdicts.computeIfAbsent(captureName + '\0' + text,
                                    k -> matchesAny(regexes, text));
                                if (!ok) continue;
                            }
                            for (int b = start; b < end; b++) {
                                if (r >= rank[b]) { hl[b] = tokType; rank[b] = r; }
                            }
                        }
                    }
                }
            }

            // Precompute line-start offsets so markTokensImpl is O(1) per line.
            int[] offsets = buildLineOffsets(src);

            if (disposed) return;
            highlightedSource = src;
            lineOffsets = offsets;   // write lineOffsets before highlights so
            highlights.set(hl);      // markTokensImpl never sees a stale pair

            Runnable hook = repaintHook;
            if (hook != null) hook.run();
        }
    }

    // One parser, only ever touched on the highlighter thread.
    private static ch.usi.si.seart.treesitter.Parser tsParser;
    private static boolean parserFailed;

    private static ch.usi.si.seart.treesitter.Parser parser() {
        if (tsParser == null && !parserFailed) {
            try {
                tsParser = ch.usi.si.seart.treesitter.Parser.getFor(Language.RUST);
            } catch (Throwable t) {
                parserFailed = true;
                System.err.println("[RustMode] no Rust parser, so no syntax colors: " + t);
            }
        }
        return tsParser;
    }

    // ── Line offset table ─────────────────────────────────────────────────

    /** Package-private hook for tests. */
    static int[] testBuildLineOffsets(String src) { return buildLineOffsets(src); }

    /**
     * Returns an array where {@code result[i]} is the index of the first
     * character of line {@code i} in {@code src}.  Line 0 always starts at 0.
     * Built once per rebuild; makes {@link #markTokensImpl} O(1) per line
     * instead of O(lines-before-current-line).
     */
    private static int[] buildLineOffsets(String src) {
        if (src.isEmpty()) return new int[]{0};
        // Count newlines first so we can allocate the exact array.
        int lineCount = 1;
        for (int i = 0; i < src.length(); i++) {
            if (src.charAt(i) == '\n') lineCount++;
        }
        int[] offsets = new int[lineCount];
        int   line    = 0;
        offsets[0]    = 0;
        for (int i = 0; i < src.length(); i++) {
            if (src.charAt(i) == '\n' && line + 1 < lineCount) {
                offsets[++line] = i + 1;
            }
        }
        return offsets;
    }

    // ── Query loading ─────────────────────────────────────────────────────

    private static Query ensureQuery() {
        if (tsQuery != null || queryFailed) return tsQuery;
        synchronized (TsRustTokenMarker.class) {
            if (tsQuery != null || queryFailed) return tsQuery;
            try {
                String scm = toAscii(loadResource("/queries/rust-highlights.scm"));
                matchPredicates = parseMatchPredicates(scm);
                tsQuery = Query.getFor(Language.RUST, scm);
            } catch (Throwable e) {
                // say it once; retrying on every keystroke would fail the same way
                queryFailed = true;
                System.err.println("[RustMode] Failed to load rust-highlights.scm: " + e);
            }
        }
        return tsQuery;
    }

    /**
     * java-tree-sitter 1.9.1 passes the query to tree-sitter with its length
     * in Java chars while the text itself is UTF-8, so each non-ASCII
     * character makes the query that many bytes short and the patterns at the
     * end of the file quietly disappear (or the cut lands mid-pattern and the
     * whole query is rejected). The file is kept ASCII; this makes sure a
     * stray dash in a comment can never bring that back.
     */
    static String toAscii(String scm) {
        StringBuilder sb = new StringBuilder(scm.length());
        for (int i = 0; i < scm.length(); i++) {
            char c = scm.charAt(i);
            sb.append(c < 128 ? c : '?');
        }
        return sb.toString();
    }

    private static String loadResource(String path) throws IOException {
        try (InputStream in = TsRustTokenMarker.class.getResourceAsStream(path)) {
            if (in == null) throw new IOException("Resource not found: " + path);
            return new String(in.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    // ── Predicates and priority ───────────────────────────────────────────

    /** Package-private for tests. All #match? regexes in the query, by capture name. */
    static Map<String, List<Pattern>> parseMatchPredicates(String scm) {
        Map<String, List<Pattern>> out = new HashMap<>();
        Matcher m = MATCH_PREDICATE.matcher(scm);
        while (m.find()) {
            String regex = m.group(2).replace("\\\\", "\\");   // unescape the .scm string
            try {
                out.computeIfAbsent(m.group(1), k -> new ArrayList<>()).add(Pattern.compile(regex));
            } catch (java.util.regex.PatternSyntaxException bad) {
                System.err.println("[RustMode] ignoring a bad #match? pattern in rust-highlights.scm: " + bad.getDescription());
            }
        }
        return out;
    }

    private static boolean matchesAny(List<Pattern> regexes, String text) {
        for (Pattern p : regexes) if (p.matcher(text).find()) return true;
        return false;
    }

    /**
     * Where captures overlap, the higher rank wins: a comment or string is
     * never recolored by something inside it, and a Processing builtin like
     * size() stays a builtin even though the call pattern also matches it.
     */
    private static byte captureRank(String name) {
        return switch (name) {
            case "comment", "string", "escape"                   -> 6;
            case "function.builtin", "variable.builtin"          -> 5;
            case "keyword", "constant.builtin", "type.builtin"   -> 4;
            case "function.macro", "attribute", "label"          -> 3;
            case "type", "constructor", "constant"               -> 3;
            case "function", "function.method"                  -> 2;
            default                                              -> 1;
        };
    }

    // ── Token type mapping ────────────────────────────────────────────────

    private static byte highlightNameToToken(String name) {
        return switch (name) {
            case "keyword"                       -> Token.KEYWORD1;
            case "function.builtin",
                 "variable.builtin"              -> Token.KEYWORD2;   // Processing API
            case "type", "type.builtin",
                 "constructor"                   -> Token.KEYWORD3;
            case "function", "function.method",
                 "function.macro"                -> Token.FUNCTION1;
            case "comment"                       -> Token.COMMENT1;
            case "string", "escape"              -> Token.LITERAL1;
            case "constant.builtin", "constant"  -> Token.LITERAL2;   // numbers, true/false, CONSTANTS
            case "attribute", "label"            -> Token.LABEL;
            case "operator"                      -> Token.OPERATOR;
            default                              -> Token.NULL;
        };
    }

    // ── Cleanup ───────────────────────────────────────────────────────────

    public synchronized void dispose() {
        disposed = true;
        if (pending != null) pending.cancel(false);
        repaintHook = null;
        syntaxDoc = null;
        highlights.set(EMPTY);
        lineOffsets = EMPTY_LINES;
    }
}
