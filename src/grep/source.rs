// Source coordinates and units: lines split on LF only, byte offsets per line, and the units a
// file is judged in. A unit is a declaration from the Python, PHP or TypeScript parser, or a text
// chunk when a file has no usable syntax. Ported from jevgrep's core/source.ts.

use super::{php, python, typescript};
#[cfg(feature = "test-api")]
use serde_json::{Value, json};

/// Lines, both ends included, counted from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl Range {
    pub fn new(start: usize, end: usize) -> Range {
        Range { start, end }
    }

    #[cfg(feature = "test-api")]
    pub fn json(&self) -> Value {
        json!({"startLine": self.start, "endLine": self.end})
    }
}

#[derive(Clone, Debug)]
pub struct SourceUnit {
    pub name: String,
    pub range: Range,
    pub byte_start: usize,
    pub byte_end: usize,
    pub partial: bool,
    pub owner_headers: Vec<Range>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Python,
    TypeScript,
    Php,
    Text,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fallback {
    Unsupported,
    Syntax,
    Size,
}

pub struct Inspection {
    pub units: Vec<SourceUnit>,
    pub comments: Vec<Range>,
    pub mode: Mode,
    pub fallback: Option<Fallback>,
}

/// A declaration as a parser reports it, before it gets byte bounds.
pub struct Declared {
    pub name: String,
    pub range: Range,
    pub owner_headers: Vec<Range>,
}

/// The lines of a source split on LF, with the byte offset where each one starts.
pub struct SourceText<'a> {
    pub bytes: &'a [u8],
    /// offsets[i] is where line i + 1 starts; one more entry than lines.
    pub offsets: Vec<usize>,
    pub line_count: usize,
}

impl<'a> SourceText<'a> {
    pub fn new(source: &'a str) -> SourceText<'a> {
        let mut offsets = vec![0];
        let mut line_count = 0;
        for line in source.split('\n') {
            offsets.push(offsets[offsets.len() - 1] + line.len() + 1);
            line_count += 1;
        }
        SourceText { bytes: source.as_bytes(), offsets, line_count }
    }

    /// offsets[index] ?? bytes.length, clamped to the source.
    pub fn offset(&self, index: usize) -> usize {
        self.offsets.get(index).copied().unwrap_or(self.bytes.len()).min(self.bytes.len())
    }
}

pub fn is_python(path: &str) -> bool {
    path.ends_with(".py") || path.ends_with(".pyi")
}

pub fn is_php(path: &str) -> bool {
    path.ends_with(".php") || path.ends_with(".phtml")
}

/// /\.(?:[cm]?[jt]s|[jt]sx)$/
pub fn is_script(path: &str) -> bool {
    [".js", ".ts", ".cjs", ".mjs", ".cts", ".mts", ".jsx", ".tsx"].iter().any(|ext| path.ends_with(ext))
}

/// Byte chunks of at most max_bytes over a range of lines. A cut moves back to a UTF-8 boundary,
/// then to just after the last LF in the chunk when there is one.
pub fn text_units(text: &SourceText, range: Range, name: &str, max_bytes: usize, partial: bool) -> Vec<SourceUnit> {
    let raw = text.bytes;
    let first = raw.len().min(text.offsets.get(range.start - 1).copied().unwrap_or(raw.len()));
    let end = raw.len().min(text.offsets.get(range.end).copied().unwrap_or(raw.len()));
    let mut units: Vec<SourceUnit> = Vec::new();
    let (mut start, mut line) = (first, range.start);
    while start < end {
        let mut finish = end.min(start + max_bytes);
        if finish < end {
            while finish > start && (raw[finish] & 0xc0) == 0x80 {
                finish -= 1;
            }
            if let Some(newline) = raw[start..finish].iter().rposition(|&b| b == b'\n') {
                finish = start + newline + 1;
            }
        }
        let part = &raw[start..finish];
        let newlines = part.iter().filter(|&&b| b == b'\n').count();
        let end_line = line + newlines - usize::from(part.last() == Some(&b'\n'));
        units.push(SourceUnit {
            name: name.to_string(),
            range: Range::new(line, end_line),
            byte_start: start,
            byte_end: finish,
            partial: partial || first != start || finish != end,
            owner_headers: Vec::new(),
        });
        line += newlines;
        start = finish;
    }
    // The final empty line has no bytes but still belongs to the snapshot coordinates.
    if raw.last() == Some(&b'\n') && range.end == text.line_count {
        if let Some(last) = units.last_mut() {
            last.range.end = text.line_count;
        }
    }
    units
}

/// Complete-file fragments keep admission independent of declaration-name sampling.
pub fn split_source(source: &str, max_bytes: usize) -> Vec<SourceUnit> {
    let text = SourceText::new(source);
    let lines = text.line_count;
    text_units(&text, Range::new(1, lines), "source", max_bytes, false)
}

/// Byte spans include original line endings; a partial unit is never widened to whole lines.
pub fn source_for_unit(source: &str, unit: &SourceUnit) -> String {
    String::from_utf8_lossy(&source.as_bytes()[unit.byte_start..unit.byte_end]).into_owned()
}

/// JavaScript's /^\s*#/ per LF-split line: conservative whole-line Python comments, including
/// lines inside multiline strings.
fn python_comments(source: &str) -> Vec<Range> {
    source.split('\n').enumerate()
        .filter(|(_, line)| line.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}').starts_with('#'))
        .map(|(index, _)| Range::new(index + 1, index + 1))
        .collect()
}

pub struct Bounds {
    pub max_unit_bytes: usize,
    pub max_parse_bytes: usize,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds { max_unit_bytes: 24_000, max_parse_bytes: 1_000_000 }
    }
}

pub fn inspect(path: &str, source: &str, bounds: Bounds) -> Inspection {
    let text = SourceText::new(source);
    let python = is_python(path);
    let comments_of_python = if python { python_comments(source) } else { Vec::new() };
    let whole = Range::new(1, text.line_count);
    let fallback = |reason: Fallback, comments: Vec<Range>| Inspection {
        mode: Mode::Text,
        fallback: Some(reason),
        comments,
        units: if source.is_empty() { Vec::new() } else { text_units(&text, whole, "source", bounds.max_unit_bytes, true) },
    };
    if source.len() > bounds.max_parse_bytes {
        return fallback(Fallback::Size, comments_of_python);
    }
    let (declared, mut comments, mode) = if python {
        match python::declarations(source) {
            Some(declared) => (declared, comments_of_python, Mode::Python),
            None => return fallback(Fallback::Syntax, comments_of_python),
        }
    } else if is_php(path) {
        match php::declarations(source) {
            Some(parsed) => (parsed.units, parsed.comments, Mode::Php),
            None => return fallback(Fallback::Syntax, Vec::new()),
        }
    } else if is_script(path) {
        let parsed = typescript::parse(path, source);
        let mut comments = parsed.comments;
        dedupe_comments(&mut comments);
        // Invalid declarations fall back to text, but comments still own the context-window boundaries.
        if parsed.syntax_error {
            return fallback(Fallback::Syntax, comments);
        }
        (parsed.units, comments, Mode::TypeScript)
    } else {
        return fallback(Fallback::Unsupported, comments_of_python);
    };
    dedupe_comments(&mut comments);
    if declared.is_empty() && !source.is_empty() {
        return Inspection { units: text_units(&text, whole, "source", bounds.max_unit_bytes, true), comments, mode, fallback: None };
    }
    let units = declared.into_iter().flat_map(|unit| {
        // Parsers report their own line coordinates, while the caller slices on LF. Keep those
        // coordinates even for CR-only source or an empty LF slice.
        let start = text.offset(unit.range.start - 1);
        let end = text.offset(unit.range.end);
        if end.saturating_sub(start) <= bounds.max_unit_bytes && end >= start {
            vec![SourceUnit { name: unit.name, range: unit.range, byte_start: start, byte_end: end, partial: false, owner_headers: unit.owner_headers }]
        } else {
            text_units(&text, unit.range, &unit.name, bounds.max_unit_bytes, true).into_iter()
                .map(|part| SourceUnit { owner_headers: unit.owner_headers.clone(), ..part })
                .collect()
        }
    }).collect();
    Inspection { units, comments, mode, fallback: None }
}

/// Keep the first range per start:end, sorted by start line (a stable sort, as in JavaScript).
fn dedupe_comments(comments: &mut Vec<Range>) {
    let mut seen = indexmap::IndexMap::new();
    for range in comments.drain(..) {
        // A later duplicate replaces the value but keeps the place, as a JavaScript Map does.
        seen.insert((range.start, range.end), range);
    }
    let mut unique: Vec<Range> = seen.into_values().collect();
    unique.sort_by_key(|range| range.start);
    *comments = unique;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cut_after_a_newline_and_cover_the_file() {
        let source = "aaaa\nbbbb\ncccc\n";
        let units = split_source(source, 7);
        let spans: Vec<(usize, usize, usize, usize)> = units.iter().map(|u| (u.byte_start, u.byte_end, u.range.start, u.range.end)).collect();
        assert_eq!(spans, vec![(0, 5, 1, 1), (5, 10, 2, 2), (10, 15, 3, 4)]);
        assert!(units.iter().all(|u| u.partial));
        let whole = split_source(source, 100);
        assert_eq!((whole[0].range.start, whole[0].range.end, whole[0].partial), (1, 4, false));
    }
}
