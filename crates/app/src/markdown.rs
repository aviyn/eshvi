//! Markdown in LSP popups, laid out like VS Code's hovers: code blocks as
//! plain code in the editor font, highlighted with the colorscheme's colors
//! (no box, no copy button), sections split by thin rules, and the prose
//! rendered by egui_commonmark.

use std::sync::OnceLock;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Ui};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::{SyntaxDefinition, SyntaxReference, SyntaxSet, SyntaxSetBuilder};
use syntect::util::LinesWithEndings;

use crate::fonts::GridFonts;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// A fenced code block and its language (the fence's info string).
    Code { lang: String, text: String },
    /// A thematic break (`---`).
    Rule,
    Prose(String),
}

/// Split markdown into code blocks, rules and the prose between them.
pub fn split(markdown: &str) -> Vec<Block> {
    fn flush(blocks: &mut Vec<Block>, prose: &mut String) {
        let text = prose.trim();
        if !text.is_empty() {
            blocks.push(Block::Prose(text.to_owned()));
        }
        prose.clear();
    }
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut after_blank = true;
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let indented = line.len() - trimmed.len() > 3;
        if let Some((fence, len)) = (!indented).then(|| fence(trimmed)).flatten() {
            flush(&mut blocks, &mut prose);
            let lang = trimmed[len..].split_whitespace().next().unwrap_or("").to_owned();
            let mut code = Vec::new();
            for line in lines.by_ref() {
                let t = line.trim();
                if t.len() >= len && t.chars().all(|c| c == fence) {
                    break;
                }
                code.push(line);
            }
            blocks.push(Block::Code { lang, text: code.join("\n") });
            after_blank = true;
        } else if !indented && after_blank && is_rule(trimmed) {
            // (After a paragraph line, `---` underlines a heading instead.)
            flush(&mut blocks, &mut prose);
            blocks.push(Block::Rule);
        } else {
            prose.push_str(line);
            prose.push('\n');
            after_blank = trimmed.is_empty();
        }
    }
    flush(&mut blocks, &mut prose);
    // No rules at the edges or twice in a row.
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for b in blocks {
        if b == Block::Rule && out.last().is_none_or(|l| *l == Block::Rule) {
            continue;
        }
        out.push(b);
    }
    if out.last() == Some(&Block::Rule) {
        out.pop();
    }
    out
}

/// A code fence (``` or ~~~, at least 3): its character and length.
fn fence(line: &str) -> Option<(char, usize)> {
    let c = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = line.chars().take_while(|x| *x == c).count();
    (len >= 3 && !(c == '`' && line[len..].contains('`'))).then_some((c, len))
}

fn is_rule(line: &str) -> bool {
    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    chars.len() >= 3 && ['-', '*', '_'].iter().any(|r| chars.iter().all(|c| c == r))
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// What a code fence says for (System)Verilog, which syntect doesn't bundle.
const VERILOG_FENCES: [&str; 6] = ["systemverilog", "verilog", "sv", "svh", "v", "vh"];

/// The SystemVerilog grammar (`assets/syntaxes/`) in a set of its own: merged into
/// the bundled set, which loads in 1 ms, it would cost a 100 ms rebuild the first
/// time any code is drawn; alone, 22 ms the first time this language is.
fn verilog_syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(|| {
        let grammar = include_str!("../../../assets/syntaxes/SystemVerilog.sublime-syntax");
        let mut builder = SyntaxSetBuilder::new();
        builder.add(SyntaxDefinition::load_from_str(grammar, true, None).expect("the grammar in assets/ parses"));
        builder.build()
    })
}

/// The grammar for a code fence's language, and the set it belongs to.
fn syntax_for(lang: &str) -> Option<(&'static SyntaxSet, &'static SyntaxReference)> {
    if lang.is_empty() {
        None
    } else if VERILOG_FENCES.iter().any(|f| f.eq_ignore_ascii_case(lang)) {
        let set = verilog_syntaxes();
        set.syntaxes().first().map(|syntax| (set, syntax))
    } else {
        let set = syntaxes();
        set.find_syntax_by_token(lang).map(|syntax| (set, syntax))
    }
}

/// Code laid out in the editor font, highlighted with `theme` when the
/// language is known; `fg` is the plain text color.
pub fn highlight(code: &str, lang: &str, theme: Option<&Theme>, fonts: &GridFonts, fg: Color32) -> LayoutJob {
    let format = |color: Color32, style: FontStyle| {
        let (font_id, fake_italic) = fonts.font_id(style.contains(FontStyle::BOLD), style.contains(FontStyle::ITALIC));
        TextFormat { font_id, color, italics: fake_italic, ..Default::default() }
    };
    let mut job = LayoutJob::default();
    let (Some(theme), Some((ss, syntax))) = (theme, syntax_for(lang)) else {
        job.append(code, 0.0, format(fg, FontStyle::empty()));
        return job;
    };
    let mut theme = theme.clone();
    theme.settings.foreground.get_or_insert(syntect::highlighting::Color { r: fg.r(), g: fg.g(), b: fg.b(), a: 255 });
    let mut lines = HighlightLines::new(syntax, &theme);
    for line in LinesWithEndings::from(code) {
        match lines.highlight_line(line, ss) {
            Ok(ranges) => {
                for (style, text) in ranges {
                    let c = style.foreground;
                    job.append(text, 0.0, format(Color32::from_rgb(c.r, c.g, c.b), style.font_style));
                }
            }
            Err(_) => job.append(line, 0.0, format(fg, FontStyle::empty())),
        }
    }
    job
}

/// A colored stretch of a line of highlighted code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub color: Color32,
    pub bold: bool,
    pub italic: bool,
}

/// For a language whose block comments and strings aren't known (`Blocks`): how many
/// lines before a stretch to be colored are read first (and not kept), so one opened just
/// above it is colored as one.
pub const COLOR_LEAD_LINES: usize = 30;

/// The spans of `lines` (byte ranges in each line) in `lang`'s grammar with `theme`, for
/// the lines in `ranges` (in order); the others are `None`, and so is all when the language
/// is unknown. The diff view colors what it shows, not the lines its folds hide: each
/// stretch is read from where it starts, or, when it is inside a block comment or string
/// (found in the text, which is far quicker than reading it with the grammar), from where
/// that opened; for a language with no `Blocks` known, from `COLOR_LEAD_LINES` before.
/// For code drawn line by line, where `highlight`'s one job of text won't do; `fg` is the
/// plain text color.
pub fn spans(lines: &[String], ranges: &[std::ops::Range<usize>], lang: &str, theme: &Theme, fg: Color32) -> Option<Vec<Option<Vec<Span>>>> {
    let (ss, syntax) = syntax_for(lang)?;
    let mut theme = theme.clone();
    theme.settings.foreground.get_or_insert(syntect::highlighting::Color { r: fg.r(), g: fg.g(), b: fg.b(), a: 255 });
    let mut out: Vec<Option<Vec<Span>>> = vec![None; lines.len()];
    let starts: Vec<usize> = ranges.iter().map(|r| r.start).collect();
    let open = Blocks::of(&syntax.name).map(|b| b.open_at(lines, &starts));
    // Where the last stretch was read to, and how: a stretch that starts near it goes on.
    let mut reader: Option<(usize, HighlightLines)> = None;
    for (k, range) in ranges.iter().enumerate() {
        let end = range.end.min(lines.len());
        if range.start >= end {
            continue;
        }
        let lead = match &open {
            Some(open) => open[k].unwrap_or(range.start),
            None => range.start.saturating_sub(COLOR_LEAD_LINES),
        };
        let (mut at, mut hl) = match reader.take() {
            Some((at, hl)) if at >= lead && at <= range.start => (at, hl),
            _ => (lead, HighlightLines::new(syntax, &theme)),
        };
        while at < end {
            let mut line = String::with_capacity(lines[at].len() + 1);
            line.push_str(&lines[at]);
            line.push('\n');
            let ranges = hl.highlight_line(&line, ss).ok()?;
            if at >= range.start {
                out[at] = Some(line_spans(&ranges, lines[at].len()));
            }
            at += 1;
        }
        reader = Some((at, hl));
    }
    Some(out)
}

/// A language's block comments and multi-line strings, as text: where one opens and
/// closes (the same for `"""`), and how a line comment starts (after which an opener
/// doesn't count). Found by the grammar's name.
struct Blocks {
    pairs: &'static [(&'static str, &'static str)],
    line_comment: Option<&'static str>,
}

impl Blocks {
    fn of(syntax: &str) -> Option<Blocks> {
        const C: &[(&str, &str)] = &[("/*", "*/")];
        const PYTHON: &[(&str, &str)] = &[("\"\"\"", "\"\"\""), ("'''", "'''")];
        const MARKUP: &[(&str, &str)] = &[("<!--", "-->")];
        const MARKDOWN: &[(&str, &str)] = &[("```", "```"), ("~~~", "~~~"), ("<!--", "-->")];
        let (pairs, line_comment) = match syntax {
            "C" | "C++" | "Objective-C" | "Objective-C++" | "C#" | "D" | "Go" | "Java" | "JavaScript" | "Rust" | "Scala" | "Groovy"
            | "PHP Source" | "SystemVerilog" => (C, Some("//")),
            "CSS" => (C, None),
            "Python" | "TOML" => (PYTHON, Some("#")),
            "Lua" => (&[("--[[", "]]")][..], Some("--")),
            "Ruby" => (&[("=begin", "=end")][..], Some("#")),
            "HTML" | "XML" => (MARKUP, None),
            "Markdown" | "MultiMarkdown" => (MARKDOWN, None),
            _ => return None,
        };
        Some(Blocks { pairs, line_comment })
    }

    /// For each of `starts` (in order), the line where the block it is inside opened, if
    /// it is inside one: read through the text from the top once.
    fn open_at(&self, lines: &[String], starts: &[usize]) -> Vec<Option<usize>> {
        let mut out = Vec::with_capacity(starts.len());
        // The block open (its pair) and the line it opened on.
        let mut inside: Option<(usize, usize)> = None;
        let mut next = 0;
        for (i, line) in lines.iter().enumerate() {
            while starts.get(next) == Some(&i) {
                out.push(inside.map(|(_, at)| at));
                next += 1;
            }
            if next == starts.len() {
                break;
            }
            let mut rest = line.as_str();
            loop {
                match inside {
                    Some((p, _)) => match rest.find(self.pairs[p].1) {
                        Some(at) => {
                            rest = &rest[at + self.pairs[p].1.len()..];
                            inside = None;
                        }
                        None => break,
                    },
                    None => {
                        let first = self.pairs.iter().enumerate().filter_map(|(p, (o, _))| rest.find(o).map(|at| (at, p))).min();
                        let comment = self.line_comment.and_then(|c| rest.find(c));
                        match first {
                            // (A line comment that is the opener's start: "--[[" in Lua.)
                            Some((at, p)) if comment.is_none_or(|c| at <= c) => {
                                rest = &rest[at + self.pairs[p].0.len()..];
                                inside = Some((p, i));
                            }
                            _ => break,
                        }
                    }
                }
            }
        }
        // (Starts past the last line.)
        out.resize(starts.len(), inside.map(|(_, at)| at));
        out
    }
}

/// A highlighted line's spans, within its first `body` bytes (its end left out).
fn line_spans(ranges: &[(syntect::highlighting::Style, &str)], body: usize) -> Vec<Span> {
    let mut at = 0;
    let mut spans = Vec::new();
    for (style, text) in ranges {
        let (start, end) = (at, (at + text.len()).min(body));
        at += text.len();
        if start >= end {
            continue;
        }
        let c = style.foreground;
        spans.push(Span {
            start,
            end,
            color: Color32::from_rgb(c.r, c.g, c.b),
            bold: style.font_style.contains(FontStyle::BOLD),
            italic: style.font_style.contains(FontStyle::ITALIC),
        });
    }
    spans
}

/// Markdown split into blocks, with the code laid out once.
pub struct Rendered {
    blocks: Vec<Block>,
    code: Vec<Option<LayoutJob>>,
    cache: CommonMarkCache,
}

/// Lua's language server links its manual through a VS Code command
/// (`command:extension.lua.doc?["en-us/54/manual.html/pdf-string.find"]`),
/// which nothing else can open: the same page on lua.org instead.
fn open_lua_manual_links(markdown: &str) -> String {
    const START: &str = "command:extension.lua.doc?[\"";
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(at) = rest.find(START) {
        let after = &rest[at + START.len()..];
        let Some(end) = after.find("\"]") else { break };
        // "en-us/54/manual.html/pdf-string.find"
        let mut parts = after[..end].splitn(4, '/');
        let (_, version, _, anchor) = (parts.next(), parts.next().unwrap_or(""), parts.next(), parts.next().unwrap_or(""));
        out.push_str(&rest[..at]);
        match version.as_bytes() {
            [major, minor] => out.push_str(&format!(
                "https://www.lua.org/manual/{}.{}/manual.html#{anchor}",
                *major as char, *minor as char
            )),
            _ => out.push_str(&rest[at..at + START.len() + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

impl Rendered {
    pub fn new(markdown: &str) -> Self {
        let blocks = split(&open_lua_manual_links(markdown));
        Self { code: vec![None; blocks.len()], blocks, cache: CommonMarkCache::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn show(&mut self, ui: &mut Ui, theme: Option<&Theme>, fonts: &GridFonts, fg: Color32) {
        ui.spacing_mut().item_spacing.y = 6.0;
        for (i, block) in self.blocks.iter().enumerate() {
            match block {
                Block::Code { lang, text } => {
                    let job = self.code[i].get_or_insert_with(|| highlight(text, lang, theme, fonts, fg)).clone();
                    ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Wrap));
                }
                Block::Rule => {
                    ui.separator();
                }
                Block::Prose(text) => {
                    ui.push_id(i, |ui| CommonMarkViewer::new().show(ui, &mut self.cache, text));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_colored_only_in_the_ranges_asked_as_if_read_from_the_top() {
        let theme = syntect::highlighting::ThemeSet::load_defaults().themes["base16-ocean.dark"].clone();
        let colored = |lines: &[String], lang: &str, ranges: &[std::ops::Range<usize>]| spans(lines, ranges, lang, &theme, Color32::WHITE).expect("known");
        let whole = |lines: &[String]| 0..lines.len();
        // Rust: a comment opened 200 lines above the stretch and closed inside it.
        let mut rust: Vec<String> = (0..600).map(|i| format!("let x{i} = {i};")).collect();
        rust[100] = "/* a long comment".into();
        rust[302] = "*/".into();
        // A line comment's "/*" opens nothing.
        rust[400] = "// see /* here".into();
        let all = colored(&rust, "rs", std::slice::from_ref(&whole(&rust)));
        let some = colored(&rust, "rs", &[10..12, 300..305, 401..404]);
        assert_eq!(some.len(), 600);
        assert!(some[9].is_none() && some[12].is_none() && some[299].is_none() && some[305].is_none());
        for i in [10, 11, 300, 301, 302, 303, 304, 401, 402, 403] {
            assert_eq!(some[i], all[i], "rust line {i}");
        }
        assert_ne!(all[301], all[303], "the comment and the code after it differ");
        // Python: a docstring opened far above; one closed on its own line opens nothing.
        let mut py: Vec<String> = (0..500).map(|i| format!("x{i} = {i}")).collect();
        py[20] = r#"y = """closed""""#.into();
        py[50] = r#"    """A docstring"#.into();
        py[401] = r#"    """"#.into();
        let all = colored(&py, "py", std::slice::from_ref(&whole(&py)));
        let some = colored(&py, "py", &[30..33, 399..404]);
        for i in [30, 31, 32, 399, 400, 401, 402, 403] {
            assert_eq!(some[i], all[i], "python line {i}");
        }
        // Lua's block comment starts like its line comment.
        let mut lua: Vec<String> = (0..300).map(|i| format!("local x{i} = {i}")).collect();
        lua[10] = "--[[ a block".into();
        lua[250] = "]]".into();
        let all = colored(&lua, "lua", std::slice::from_ref(&whole(&lua)));
        let some = colored(&lua, "lua", &[200..202, 249..252]);
        for i in [200, 201, 249, 250, 251] {
            assert_eq!(some[i], all[i], "lua line {i}");
        }
        // An unknown language: none.
        assert!(spans(&rust, std::slice::from_ref(&whole(&rust)), "no-such-language", &theme, Color32::WHITE).is_none());
    }

    #[test]
    fn where_each_stretch_is_inside_a_block_is_found_in_the_text() {
        let c = Blocks::of("Rust").unwrap();
        let lines: Vec<String> = ["a", "/* b", "c", "d */ e", "f /* g */ h", "// /* i", "j"].iter().map(|l| l.to_string()).collect();
        assert_eq!(c.open_at(&lines, &[0, 1, 2, 3, 4, 5, 6]), vec![None, None, Some(1), Some(1), None, None, None]);
        let p = Blocks::of("Python").unwrap();
        let lines: Vec<String> = ["'''x'''", r#"""""#, "''' still in", r#"""""#, "z"].iter().map(|l| l.to_string()).collect();
        assert_eq!(p.open_at(&lines, &[1, 2, 3, 4]), vec![None, Some(1), Some(1), None]);
        assert!(Blocks::of("Bourne Again Shell (bash)").is_none());
    }

    fn code(lang: &str, text: &str) -> Block {
        Block::Code { lang: lang.into(), text: text.into() }
    }

    #[test]
    fn every_verilog_fence_gets_the_bundled_grammar_and_other_languages_keep_theirs() {
        for lang in VERILOG_FENCES.iter().copied().chain(["SystemVerilog", "SV"]) {
            let (_, syntax) = syntax_for(lang).unwrap_or_else(|| panic!("no grammar for {lang}"));
            assert_eq!(syntax.name, "SystemVerilog", "{lang}");
        }
        assert_eq!(syntax_for("rust").unwrap().1.name, "Rust");
        assert!(syntax_for("").is_none());
        assert!(syntax_for("no-such-language").is_none());
    }

    /// Hovers show one declaration, outside any module.
    #[test]
    fn a_lone_declaration_is_colored() {
        let (set, syntax) = syntax_for("systemverilog").unwrap();
        let themes = syntect::highlighting::ThemeSet::load_defaults();
        let mut lines = HighlightLines::new(syntax, &themes.themes["base16-ocean.dark"]);
        for line in ["input logic [3:0] req\n", "parameter int WIDTH = 4\n"] {
            let colors: std::collections::HashSet<_> = lines.highlight_line(line, set).unwrap().iter().filter(|(_, t)| !t.trim().is_empty()).map(|(s, _)| s.foreground).collect();
            assert!(colors.len() >= 3, "{line}: {colors:?}");
        }
    }

    #[test]
    fn lua_manual_links_go_to_lua_org() {
        let md = r#"see [§6.4.1](command:extension.lua.doc?["en-us/54/manual.html/6.4.1"]) and [View documents](command:extension.lua.doc?["en-us/53/manual.html/pdf-string.find"])."#;
        assert_eq!(
            open_lua_manual_links(md),
            "see [§6.4.1](https://www.lua.org/manual/5.4/manual.html#6.4.1) and [View documents](https://www.lua.org/manual/5.3/manual.html#pdf-string.find)."
        );
        // Others, and anything not as expected, stay.
        for same in ["[a](https://x.y)", r#"[a](command:extension.lua.doc?["odd"])"#, "command:extension.lua.doc?[\"cut"] {
            assert_eq!(open_lua_manual_links(same), same);
        }
    }

    #[test]
    fn splits_a_rust_analyzer_hover() {
        let md = "```rust\neshvi::cmdline\n```\n\n```rust\nfn chunk_job(\n    job: &mut LayoutJob,\n)\n```\n\n---\n\nAppend *chunks* to `job`.\n";
        assert_eq!(
            split(md),
            [
                code("rust", "eshvi::cmdline"),
                code("rust", "fn chunk_job(\n    job: &mut LayoutJob,\n)"),
                Block::Rule,
                Block::Prose("Append *chunks* to `job`.".into()),
            ]
        );
    }

    #[test]
    fn rules_headings_and_fences() {
        // A setext heading's underline is not a rule; edge/double rules go.
        assert_eq!(
            split("---\nTitle\n---\n\n***\n___\ntext\n\n---"),
            [Block::Prose("Title\n---".into()), Block::Rule, Block::Prose("text".into())]
        );
        // Tildes, longer fences, an unterminated block, info string words.
        assert_eq!(split("~~~~lua extra\nx = 1\n~~~~"), [code("lua", "x = 1")]);
        assert_eq!(split("````\nhas ``` inside\n````"), [code("", "has ``` inside")]);
        assert_eq!(split("```python\nopen"), [code("python", "open")]);
        // Inline code is not a fence.
        assert_eq!(split("``` a ` b"), [Block::Prose("``` a ` b".into())]);
    }
}
