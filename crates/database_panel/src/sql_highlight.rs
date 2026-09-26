//! A small, dependency-free SQL syntax highlighter.
//!
//! This workspace's vendored editor's real tree-sitter backend
//! (`gpui_component::highlighter`) is a documented no-op here — the
//! `tree-sitter` feature is declared but never wired to an actual
//! `tree-sitter`/grammar dependency (see that crate's `Cargo.toml`), so
//! `EditorState::new(..).language("sql")` alone highlights nothing. Rather
//! than pulling in a new, unvetted `tree-sitter-sql` dependency to fix a
//! crate-wide stub, this plugs into the editor's separate, parser-
//! independent seam instead (`InputHighlighter`/`set_highlighter_factory`,
//! `crates/gpui_base/src/input/editor/highlighting.rs`) with a manual
//! keyword/string/number/comment scanner — real color, no grammar crate.

use std::ops::Range;

use gpui::{Context, HighlightStyle, Window, hsla};
use gpui_component::input::{EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope};

const KEYWORD_COLOR: fn() -> gpui::Hsla = || hsla(286.0 / 360.0, 0.60, 0.72, 1.0);
const STRING_COLOR: fn() -> gpui::Hsla = || hsla(95.0 / 360.0, 0.45, 0.62, 1.0);
const NUMBER_COLOR: fn() -> gpui::Hsla = || hsla(29.0 / 360.0, 0.70, 0.62, 1.0);
const COMMENT_COLOR: fn() -> gpui::Hsla = || hsla(220.0 / 360.0, 0.12, 0.55, 1.0);

const KEYWORDS: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create",
    "table", "alter", "drop", "add", "column", "join", "inner", "left", "right", "outer", "full",
    "on", "and", "or", "not", "null", "is", "as", "order", "by", "group", "having", "limit",
    "offset", "distinct", "union", "all", "in", "exists", "between", "like", "case", "when",
    "then", "else", "end", "default", "primary", "key", "foreign", "references", "index",
    "unique", "view", "if", "with", "asc", "desc", "count", "sum", "avg", "min", "max", "cast",
    "returning", "begin", "commit", "rollback", "transaction", "grant", "revoke", "database",
    "schema", "constraint", "check", "cascade", "restrict", "true", "false",
];

/// Classifies `text` into style runs covering it end to end — `styles()`'
/// contract requires a *full* partition, so a plain identifier/whitespace
/// run still gets an entry (`HighlightStyle::default()`), just merged with
/// its neighbors when they share the same style.
fn classify(text: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut runs: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut i = 0;

    let push = |start: usize, end: usize, style: HighlightStyle, runs: &mut Vec<(Range<usize>, HighlightStyle)>| {
        if start == end {
            return;
        }
        if let Some(last) = runs.last_mut() {
            if last.1 == style && last.0.end == start {
                last.0.end = end;
                return;
            }
        }
        runs.push((start..end, style));
    };

    while i < len {
        let c = bytes[i] as char;

        // Line comment: `-- ...` to end of line.
        if c == '-' && i + 1 < len && bytes[i + 1] as char == '-' {
            let start = i;
            while i < len && bytes[i] as char != '\n' {
                i += 1;
            }
            push(start, i, style(COMMENT_COLOR()), &mut runs);
            continue;
        }

        // Block comment: `/* ... */`.
        if c == '/' && i + 1 < len && bytes[i + 1] as char == '*' {
            let start = i;
            i += 2;
            while i + 1 < len && !(bytes[i] as char == '*' && bytes[i + 1] as char == '/') {
                i += 1;
            }
            i = (i + 2).min(len);
            push(start, i, style(COMMENT_COLOR()), &mut runs);
            continue;
        }

        // Quoted string/identifier: `'...'` or `"..."`, with a doubled quote
        // as the escape for a literal quote inside.
        if c == '\'' || c == '"' {
            let quote = c;
            let start = i;
            i += 1;
            while i < len {
                if bytes[i] as char == quote {
                    if i + 1 < len && bytes[i + 1] as char == quote {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            push(start, i, style(STRING_COLOR()), &mut runs);
            continue;
        }

        // Number: a digit run, optionally with one decimal point.
        if c.is_ascii_digit() {
            let start = i;
            while i < len && (bytes[i] as char).is_ascii_digit() {
                i += 1;
            }
            if i < len && bytes[i] as char == '.' && i + 1 < len && (bytes[i + 1] as char).is_ascii_digit() {
                i += 1;
                while i < len && (bytes[i] as char).is_ascii_digit() {
                    i += 1;
                }
            }
            push(start, i, style(NUMBER_COLOR()), &mut runs);
            continue;
        }

        // Word: an identifier or keyword.
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < len && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] as char == '_') {
                i += 1;
            }
            let word = &text[start..i];
            let is_keyword = KEYWORDS.iter().any(|keyword| keyword.eq_ignore_ascii_case(word));
            let word_style = if is_keyword { style(KEYWORD_COLOR()) } else { HighlightStyle::default() };
            push(start, i, word_style, &mut runs);
            continue;
        }

        // Anything else (whitespace, punctuation) — one byte of default
        // style, merged into the previous default run if adjacent.
        push(i, i + 1, HighlightStyle::default(), &mut runs);
        i += 1;
    }

    runs
}

fn style(color: gpui::Hsla) -> HighlightStyle {
    HighlightStyle { color: Some(color), ..Default::default() }
}

/// The `InputHighlighter` this module installs on every workbench editor.
/// Re-tokenizes the whole buffer on every edit — fine at the size a SQL
/// statement/script realistically reaches in this workbench, not something
/// meant for large-file editing.
pub(crate) struct SqlHighlighter {
    runs: Vec<(Range<usize>, HighlightStyle)>,
}

impl SqlHighlighter {
    pub(crate) fn new() -> Self {
        Self { runs: Vec::new() }
    }
}

impl InputHighlighter for SqlHighlighter {
    fn language(&self) -> gpui::SharedString {
        "sql".into()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.runs = classify(&text.to_string());
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.runs
            .iter()
            .filter(|(run, _)| run.end > range.start && run.start < range.end)
            .cloned()
            .collect()
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}
