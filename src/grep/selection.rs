// Declaration selection for one file (jevgrep's core/selection.ts). The file's units are asked in
// groups whether they implement and belong to what the query asks about; in the second pass also
// whether the evidence selected elsewhere references them, which can add or retract selections.
// Selected units grow into excerpts: three lines around them, adjacent comments, and for Python
// the class header and neighbouring methods. What is shown is stricter than what is selected.

use super::evaluator::{Answers, Failure, Kind};
use super::python;
use super::requests::{Declaration, evidence_request};
use super::source::{self, Bounds, Mode, Range, SourceUnit};
use serde_json::{Value, json};

const SOURCE_UNIT_BYTES: usize = 24_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// Lines, and byte bounds when the range does not cover whole lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EvidenceRange {
    pub start: usize,
    pub end: usize,
    pub bytes: Option<(usize, usize)>,
}

impl EvidenceRange {
    pub fn lines(range: Range) -> EvidenceRange {
        EvidenceRange { start: range.start, end: range.end, bytes: None }
    }

    pub fn range(&self) -> Range {
        Range::new(self.start, self.end)
    }

    /// {startLine, endLine[, sourceByteStart, sourceByteEnd]}, spread into other objects.
    pub fn fields(&self) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        map.insert("startLine".into(), self.start.into());
        map.insert("endLine".into(), self.end.into());
        if let Some((start, end)) = self.bytes {
            map.insert("sourceByteStart".into(), start.into());
            map.insert("sourceByteEnd".into(), end.into());
        }
        map
    }
}

#[derive(Clone, Debug)]
pub struct Excerpt {
    pub range: EvidenceRange,
    pub source: String,
    /// Byte-partial: exact bytes rather than whole lines.
    pub partial: bool,
}

#[derive(Clone, Debug)]
pub struct Lead {
    pub name: String,
    pub range: EvidenceRange,
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub range: EvidenceRange,
    pub score: f64,
}

#[derive(Clone, Debug)]
pub struct CallLead {
    pub caller: String,
    pub name: String,
    pub range: Range,
    pub unknown_earlier_bases: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FileEvidence {
    pub path: String,
    pub content_hash: String,
    pub score: f64,
    pub priority: Option<f64>,
    pub roles: Vec<String>,
    pub leads: Vec<Lead>,
    pub selected: Vec<EvidenceRange>,
    pub rendered: Vec<EvidenceRange>,
    pub excerpts: Vec<Excerpt>,
    pub presentation_excerpts: Option<Vec<Excerpt>>,
    pub selected_presentation_excerpts: Option<Vec<Excerpt>>,
    pub presentation_selected: Option<Vec<EvidenceRange>>,
    pub source_decisions: Vec<Decision>,
    pub call_leads: Vec<CallLead>,
    pub source_omitted: bool,
}

impl FileEvidence {
    pub fn admitted(path: &str, content_hash: &str, score: f64) -> FileEvidence {
        FileEvidence {
            path: path.into(), content_hash: content_hash.into(), score, priority: None, roles: Vec::new(), leads: Vec::new(),
            selected: Vec::new(), rendered: Vec::new(), excerpts: Vec::new(), presentation_excerpts: None,
            selected_presentation_excerpts: None, presentation_selected: None, source_decisions: Vec::new(), call_leads: Vec::new(),
            source_omitted: false,
        }
    }

    /// What is shown: the presentation when there is one, else the expanded excerpts.
    pub fn shown(&self) -> &[Excerpt] {
        self.presentation_excerpts.as_deref().unwrap_or(&self.excerpts)
    }
}

pub struct Selection {
    pub file: FileEvidence,
    pub declarations: Vec<(String, Range)>,
    pub issues: indexmap::IndexMap<String, usize>,
    pub provider_failure: Option<String>,
}

pub fn merge_spans(spans: &[Span]) -> Vec<Span> {
    let mut sorted: Vec<Span> = spans.iter().copied().filter(|span| span.end > span.start).collect();
    sorted.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    let mut merged: Vec<Span> = Vec::new();
    for span in sorted {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    merged
}

/// Line coordinates of one snapshot.
pub struct Lines<'a> {
    pub lines: Vec<&'a str>,
    pub bytes: &'a [u8],
    pub offsets: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub fn new(source: &'a str) -> Lines<'a> {
        let lines: Vec<&str> = source.split('\n').collect();
        let mut offsets = vec![0];
        for line in &lines {
            offsets.push(source.len().min(offsets[offsets.len() - 1] + line.len() + 1));
        }
        Lines { lines, bytes: source.as_bytes(), offsets }
    }

    fn offset(&self, index: usize) -> usize {
        self.offsets.get(index).copied().unwrap_or(self.bytes.len())
    }

    /// offsets[index] as JavaScript reads it: None past the last line. TypeScript's line numbers
    /// can go past the LF lines when a file breaks lines at a lone CR, U+2028 or U+2029.
    fn offset_at(&self, index: usize) -> Option<usize> {
        self.offsets.get(index).copied()
    }

    pub fn span_for(&self, range: &EvidenceRange) -> Span {
        match range.bytes {
            Some((start, end)) => Span { start, end },
            None => Span { start: self.offset(range.start - 1), end: self.offset(range.end) },
        }
    }

    fn line_at(&self, byte: usize) -> usize {
        let (mut low, mut high) = (0, self.lines.len());
        while low + 1 < high {
            let middle = (low + high) / 2;
            if self.offsets[middle] <= byte { low = middle } else { high = middle }
        }
        low + 1
    }

    pub fn range_for(&self, span: Span) -> EvidenceRange {
        let start = self.line_at(span.start);
        let end = self.line_at(span.start.max(span.end.saturating_sub(1)));
        let whole = span.start == self.offset(start - 1) && span.end == self.offset(end);
        EvidenceRange { start, end, bytes: (!whole).then_some((span.start, span.end)) }
    }

    /// lines.slice(start - 1, end).join("\n")
    pub fn join(&self, start: usize, end: usize) -> String {
        let from = (start.saturating_sub(1)).min(self.lines.len());
        let to = end.min(self.lines.len()).max(from);
        self.lines[from..to].join("\n")
    }

    fn partial_line(&self, unit: &SourceUnit) -> bool {
        unit.byte_start != self.offset(unit.range.start - 1) || unit.byte_end != self.offset(unit.range.end)
    }
}

/// The follow-up pass expands prior context once more; positive selections keep their own
/// provenance. `prepare` checks the file is unchanged before each group and gives the selected
/// evidence of the second pass: Ok(None) when the file changed, Ok(Some(None)) in the first pass.
pub fn select_file(
    snapshot: &super::fs::Snapshot,
    query: &str,
    score: f64,
    evaluate: &dyn Fn(&Value) -> Result<Answers, Failure>,
    prepare: &dyn Fn() -> Result<Option<Option<Value>>, Failure>,
    previous: Option<&FileEvidence>,
) -> Selection {
    let mut issues: indexmap::IndexMap<String, usize> = indexmap::IndexMap::new();
    let mut provider_failure: Option<String> = None;
    let warn = |kind: &str, issues: &mut indexmap::IndexMap<String, usize>| *issues.entry(kind.to_string()).or_default() += 1;
    let previous = match previous {
        Some(previous) if previous.path != snapshot.path || previous.content_hash != snapshot.content_hash => {
            warn("changed", &mut issues);
            None
        }
        other => other,
    };
    let source = snapshot.source.as_str();
    let lines = Lines::new(source);
    let giant_line = lines.lines.iter().any(|line| line.len() > SOURCE_UNIT_BYTES);
    let syntax = source::inspect(&snapshot.path, source, Bounds {
        max_unit_bytes: if giant_line { SOURCE_UNIT_BYTES } else { SOURCE_UNIT_BYTES.max(source.len()) },
        ..Bounds::default()
    });
    // Giant lines keep byte coordinates; an ordinary fallback uses complete-source line fragments.
    let mut units = syntax.units.clone();
    if !giant_line && (syntax.mode == Mode::Text || units.iter().all(|unit| unit.partial)) {
        units = source::split_source(source, 3000).into_iter().map(|mut unit| {
            unit.range.end = lines.line_at(unit.byte_start.max(unit.byte_end.saturating_sub(1)));
            unit
        }).collect();
    }
    if !giant_line {
        units = units.into_iter().flat_map(|unit| {
            if lines.join(unit.range.start, unit.range.end).len() <= SOURCE_UNIT_BYTES {
                return vec![unit];
            }
            let mut blocks = Vec::new();
            let mut start = unit.range.start;
            while start <= unit.range.end {
                let end = unit.range.end.min(start + 15);
                blocks.push(SourceUnit {
                    name: unit.name.clone(), range: Range::new(start, end), byte_start: lines.offset(start - 1), byte_end: lines.offset(end),
                    partial: true, owner_headers: Vec::new(),
                });
                start += 16;
            }
            blocks
        }).collect();
    }
    let mut selected_coordinates: Vec<Range> = Vec::new();
    let mut selected: Vec<Span> = previous.map(|p| p.selected.iter().map(|r| lines.span_for(r)).collect()).unwrap_or_default();
    let mut decisions: indexmap::IndexMap<(usize, usize), Decision> = indexmap::IndexMap::new();
    for decision in previous.map(|p| p.source_decisions.as_slice()).unwrap_or_default() {
        let span = lines.span_for(&decision.range);
        decisions.insert((span.start, span.end), decision.clone());
    }
    let mut context_spans: Vec<Span> = previous.map(|p| p.rendered.iter().map(|r| lines.span_for(r)).collect()).unwrap_or_default();
    let mut leads: indexmap::IndexMap<String, Lead> = indexmap::IndexMap::new();
    let lead_key = |lead: &Lead| crate::js::stringify(&json!([lead.name, Value::Object(lead.range.fields())]));
    for lead in previous.map(|p| p.leads.as_slice()).unwrap_or_default() {
        leads.insert(lead_key(lead), lead.clone());
    }
    let mut groups: Vec<Vec<SourceUnit>> = Vec::new();
    let mut pending: Vec<SourceUnit> = Vec::new();
    for unit in units.iter() {
        if !pending.is_empty() && (pending.len() >= 8 || lines.join(pending[0].range.start, unit.range.end).len() > 14000) {
            groups.push(std::mem::take(&mut pending));
        }
        pending.push(unit.clone());
    }
    if !pending.is_empty() {
        groups.push(pending);
    }
    let mut invalidated = false;
    for group in &groups {
        let outcome = (|| -> Result<bool, Failure> {
            let Some(evidence) = prepare()? else { return Ok(false) };
            let first = 1.max(group[0].range.start.saturating_sub(8));
            let last = lines.lines.len().min(group[group.len() - 1].range.end + 8);
            let oversized = lines.lines.iter().take(20).chain(lines.lines[(first - 1).min(lines.lines.len())..last.min(lines.lines.len())].iter())
                .any(|line| line.len() > SOURCE_UNIT_BYTES);
            // Line windows cannot describe a partial giant line; send only the parser's byte spans.
            let context = if group.iter().any(|unit| lines.partial_line(unit)) || oversized {
                group.iter().map(|unit| format!("Source lines {}-{}; source bytes {}-{}:\n{}", unit.range.start, unit.range.end,
                    unit.byte_start, unit.byte_end, source::source_for_unit(source, unit))).collect::<Vec<_>>().join("\n")
            } else if source.len() <= 16000 {
                source.to_string()
            } else {
                format!("Opening context:\n{}\nSource lines {first}-{last}:\n{}", lines.join(1, 20), lines.join(first, last))
            };
            let declarations: Vec<Declaration> = group.iter().map(|unit| Declaration { name: unit.name.clone(), start: unit.range.start, end: unit.range.end }).collect();
            let request = evidence_request(query, &snapshot.path, &context, &declarations, evidence.as_ref());
            let answers = evaluate(&request)?;
            let mut values = Vec::new();
            for (index, unit) in group.iter().enumerate() {
                let mut asked = vec![answers.get(&format!("q{index}")).copied(), answers.get(&format!("scope{index}")).copied()];
                if evidence.is_some() {
                    asked.push(answers.get(&format!("ref{index}")).copied());
                }
                if asked.iter().any(|value| !value.is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v))) {
                    return Err(Failure::of(Kind::Provider));
                }
                let asked: Vec<f64> = asked.into_iter().flatten().collect();
                values.push((unit, asked[0].min(asked[1]).max(asked.get(2).copied().unwrap_or(0.0))));
            }
            for (unit, value) in values {
                let span = Span { start: unit.byte_start, end: unit.byte_end };
                decisions.insert((span.start, span.end), Decision { range: lines.range_for(span), score: value });
                // Only a valid contextual rejection retracts an earlier selection; failed or
                // unprocessed groups keep their earlier spans.
                if evidence.is_some() && value <= 0.5 {
                    selected = selected.iter().flat_map(|kept| {
                        if kept.end <= span.start || kept.start >= span.end {
                            return vec![*kept];
                        }
                        let mut parts = Vec::new();
                        if kept.start < span.start {
                            parts.push(Span { start: kept.start, end: span.start });
                        }
                        if kept.end > span.end {
                            parts.push(Span { start: span.end, end: kept.end });
                        }
                        parts
                    }).collect();
                }
                if value > 0.5 {
                    selected.push(span);
                    context_spans.push(span);
                    if !lines.partial_line(unit) {
                        selected_coordinates.push(unit.range);
                    }
                }
                if value > 0.25 && !unit.name.ends_with(".context") {
                    let range = if lines.partial_line(unit) { lines.range_for(span) } else { EvidenceRange::lines(unit.range) };
                    let lead = Lead { name: unit.name.clone(), range };
                    leads.insert(lead_key(&lead), lead);
                }
            }
            Ok(true)
        })();
        match outcome {
            Ok(true) => {}
            Ok(false) => {
                invalidated = true;
                selected.clear();
                selected_coordinates.clear();
                context_spans.clear();
                leads.clear();
                break;
            }
            Err(failure) => {
                warn(failure.kind.name(), &mut issues);
                if failure.kind == Kind::Provider {
                    provider_failure.get_or_insert(failure.message);
                } else {
                    break;
                }
            }
        }
    }
    let chosen = merge_spans(&selected);
    let mut whole_ranges: Vec<Range> = selected_coordinates.clone();
    let mut rendered: Vec<Span> = Vec::new();
    for span in merge_spans(&context_spans) {
        let range = lines.range_for(span);
        if range.bytes.is_some() { rendered.push(span) } else { whole_ranges.push(range.range()) }
    }
    let neighborhood = if !whole_ranges.is_empty() && source::is_python(&snapshot.path) && source.len() <= 1_000_000 {
        python::neighborhood(source, &whole_ranges)
    } else {
        Vec::new()
    };
    let excerpts_for = |ranges: &[Range], mut rendered: Vec<Span>| -> (Vec<EvidenceRange>, Vec<Excerpt>) {
        let count = lines.lines.len();
        let windows: Vec<Range> = ranges.iter().map(|range| Range::new(1.max(range.start.saturating_sub(3)), count.min(range.end + 3))).collect();
        let mut grown = windows.clone();
        for window in &mut grown {
            let mut changed = true;
            while changed {
                changed = false;
                for comment in &syntax.comments {
                    let blank = |from: usize, to: usize| lines.lines[from.min(count)..to.min(count).max(from.min(count))].iter().all(|line| crate::js::trim(line).is_empty());
                    let before = comment.end < window.start && blank(comment.end, window.start - 1);
                    let after = comment.start > window.end && blank(window.end, comment.start - 1);
                    if (comment.start <= window.end && comment.end >= window.start) || before || after {
                        let start = window.start.min(comment.start);
                        let end = window.end.max(comment.end);
                        if start != window.start || end != window.end {
                            *window = Range::new(start, end);
                            changed = true;
                        }
                    }
                }
            }
            // A bound past the last line is undefined in JavaScript; a span with one is dropped.
            let mut segment_start = lines.offset_at(window.start - 1);
            for line in window.start..=window.end {
                let (Some(start), Some(end)) = (lines.offset_at(line - 1), lines.offset_at(line)) else { continue };
                // An adjacent selected declaration must not pull in an unselected giant line.
                if end - start > SOURCE_UNIT_BYTES {
                    if let Some(segment) = segment_start {
                        rendered.push(Span { start: segment, end: start });
                    }
                    for span in &chosen {
                        if span.start < end && span.end > start {
                            rendered.push(Span { start: span.start.max(start), end: span.end.min(end) });
                        }
                    }
                    segment_start = Some(end);
                }
            }
            if let (Some(start), Some(end)) = (segment_start, lines.offset_at(window.end)) {
                rendered.push(Span { start, end });
            }
        }
        let output = merge_spans(&rendered);
        let rendered_range = |span: Span| {
            let mut range = lines.range_for(span);
            // A trailing empty line has no bytes, but stays part of a line-based window.
            if range.bytes.is_none() && span.end == lines.bytes.len() && grown.iter().any(|window| window.end == count) {
                range.end = count;
            }
            range
        };
        let ranges: Vec<EvidenceRange> = output.iter().map(|span| rendered_range(*span)).collect();
        let excerpts = output.iter().map(|span| {
            let range = rendered_range(*span);
            let partial = range.bytes.is_some();
            Excerpt {
                range,
                source: if partial { String::from_utf8_lossy(&lines.bytes[span.start..span.end]).into_owned() } else { lines.join(range.start, range.end) },
                partial,
            }
        }).collect();
        (ranges, excerpts)
    };
    let expanded = excerpts_for(&[whole_ranges.clone(), neighborhood].concat(), rendered);
    // Presentation can be stricter without narrowing the evidence sent to Jev.
    let displayed = merge_spans(&decisions.values().filter(|d| d.score > 0.7).map(|d| lines.span_for(&d.range)).flat_map(|span| {
        chosen.iter().filter_map(move |kept| {
            let (start, end) = (span.start.max(kept.start), span.end.min(kept.end));
            (start < end).then_some(Span { start, end })
        })
    }).collect::<Vec<_>>());
    let presentation_for = |spans: &[Span]| -> Vec<Excerpt> {
        let selected_ranges: Vec<EvidenceRange> = spans.iter().map(|span| lines.range_for(*span)).collect();
        let mut headers: indexmap::IndexMap<(usize, usize), Range> = indexmap::IndexMap::new();
        for unit in &syntax.units {
            if !spans.iter().any(|span| span.start < unit.byte_end && span.end > unit.byte_start) {
                continue;
            }
            for header in &unit.owner_headers {
                let size = lines.offset_at(header.end).zip(lines.offset_at(header.start - 1)).map(|(end, start)| end - start);
                if size.is_some_and(|size| size <= SOURCE_UNIT_BYTES) {
                    headers.insert((header.start, header.end), *header);
                }
            }
        }
        let mut ranges: Vec<Range> = selected_ranges.iter().filter(|r| r.bytes.is_none()).map(EvidenceRange::range).collect();
        ranges.extend(headers.values().copied());
        let partial: Vec<Span> = spans.iter().copied().filter(|span| lines.range_for(*span).bytes.is_some()).collect();
        excerpts_for(&ranges, partial).1
    };
    let presentation = presentation_for(&displayed);
    let file = FileEvidence {
        path: snapshot.path.clone(),
        content_hash: snapshot.content_hash.clone(),
        score,
        priority: None,
        roles: previous.map(|p| p.roles.clone()).unwrap_or_default(),
        leads: leads.into_values().collect(),
        selected: chosen.iter().map(|span| lines.range_for(*span)).collect(),
        rendered: expanded.0,
        excerpts: expanded.1,
        presentation_excerpts: Some(presentation),
        selected_presentation_excerpts: Some(presentation_for(&chosen)),
        presentation_selected: Some(displayed.iter().map(|span| lines.range_for(*span)).collect()),
        source_decisions: decisions.into_values().collect(),
        call_leads: Vec::new(),
        source_omitted: invalidated,
    };
    Selection { file, declarations: units.iter().map(|unit| (unit.name.clone(), unit.range)).collect(), issues, provider_failure }
}
