// Compact presentation only: discovery and relevance decisions are unchanged.
// Reserve source for each shortlisted file before letting one large excerpt
// consume the remaining budget. Every crop is labeled and keeps source verbatim.
use super::retrieve::Outcome;
use super::selection::{EvidenceRange, Excerpt, FileEvidence};
// Match the full renderer's escaping for untrusted paths and identifiers.
mod js {
    pub use crate::js::locale_compare;
    pub fn quote(value: &str) -> String { super::super::render::quote(value) }
}
use std::cmp::Ordering;

fn score(file: &FileEvidence, range: &EvidenceRange) -> f64 {
    file.source_decisions.iter().filter(|d| d.range.start <= range.end && d.range.end >= range.start)
        .map(|d| d.score).fold(0.0, f64::max)
}

fn focus(file: &FileEvidence, excerpt: &Excerpt) -> usize {
    file.source_decisions.iter().filter(|d| d.range.start <= excerpt.range.end && d.range.end >= excerpt.range.start)
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal))
        .map_or(excerpt.range.start, |d| d.range.start.max(excerpt.range.start))
}

fn fit(excerpt: &Excerpt, budget: usize, focus: usize) -> Option<Excerpt> {
    if excerpt.source.len() <= budget { return Some(excerpt.clone()); }
    if budget == 0 { return None; }
    let lines: Vec<&str> = excerpt.source.split_inclusive('\n').collect();
    if lines.is_empty() { return None; }
    let target = focus.saturating_sub(excerpt.range.start).min(lines.len() - 1);
    let mut first = target.saturating_sub(2);
    // Context must not crowd out the selected declaration itself.
    while first < target && lines[first..=target].iter().map(|line| line.len()).sum::<usize>() > budget {
        first += 1;
    }
    let offset: usize = lines[..first].iter().map(|line| line.len()).sum();
    let rest = &excerpt.source[offset..];
    let mut end = budget.min(rest.len());
    while end > 0 && !rest.is_char_boundary(end) { end -= 1; }
    if end == 0 { return None; }
    // Prefer whole lines, but permit an explicitly partial single giant line.
    if end < rest.len() {
        if let Some(newline) = rest[..end].rfind('\n') { end = newline + 1; }
    }
    let source = rest[..end].to_string();
    let start_line = excerpt.range.start + first;
    let end_line = start_line + source.bytes().filter(|&b| b == b'\n').count()
        - usize::from(source.ends_with('\n'));
    Some(Excerpt {
        range: EvidenceRange { start: start_line, end: end_line.min(excerpt.range.end),
            bytes: excerpt.range.bytes.map(|(start, _)| (start + offset, start + offset + end)) },
        source, partial: true,
    })
}

fn excerpts(file: &FileEvidence) -> Vec<&Excerpt> {
    if file.source_omitted { return Vec::new(); }
    let mut items: Vec<_> = file.shown().iter().filter(|e| !e.source.is_empty()).collect();
    items.sort_by(|a, b| score(file, &b.range).partial_cmp(&score(file, &a.range)).unwrap_or(Ordering::Equal)
        .then_with(|| a.source.len().cmp(&b.source.len()))
        .then_with(|| a.range.start.cmp(&b.range.start)));
    items
}

fn source_block(lines: &mut Vec<String>, file: &FileEvidence, excerpt: &Excerpt) {
    let fence = "`".repeat(3.max(excerpt.source.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1));
    let partial = if excerpt.partial || excerpt.range.bytes.is_some() { " (partial snippet; read surrounding/missing lines as needed)" } else { "" };
    lines.push(format!("\nSource {} lines {}-{}{partial}:", js::quote(&file.path), excerpt.range.start, excerpt.range.end));
    lines.push(fence.clone());
    lines.push(excerpt.source.clone());
    lines.push(fence);
}

pub fn render(outcome: &Outcome, source_budget: usize, max_files: usize) -> String {
    let mut files: Vec<_> = outcome.files.iter().collect();
    files.sort_by(|a, b| b.priority.unwrap_or(b.score).partial_cmp(&a.priority.unwrap_or(a.score)).unwrap_or(Ordering::Equal)
        .then_with(|| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal))
        .then_with(|| js::locale_compare(&a.path, &b.path)));
    files.truncate(max_files);
    let candidates: Vec<_> = files.iter().map(|file| excerpts(file)).collect();
    let source_files = candidates.iter().filter(|items| !items.is_empty()).count();
    let share = if source_files == 0 { 0 } else { source_budget / source_files };
    let mut remaining = source_budget;
    let mut shown: Vec<Vec<Excerpt>> = vec![Vec::new(); files.len()];
    // First pass reserves a fair share for every file that has selected source.
    for (index, file) in files.iter().enumerate() {
        if let Some(first) = candidates[index].first() {
            if let Some(excerpt) = fit(first, share.min(remaining), focus(file, first)) {
                remaining -= excerpt.source.len();
                shown[index].push(excerpt);
            }
        }
    }
    // Then expand the best excerpt, or add further excerpts, in file priority order.
    for (index, file) in files.iter().enumerate() {
        for (at, original) in candidates[index].iter().enumerate() {
            if remaining == 0 { break; }
            let previous = if at == 0 { shown[index].first().map_or(0, |e| e.source.len()) } else { 0 };
            if let Some(excerpt) = fit(original, remaining + previous, focus(file, original)) {
                if excerpt.source.len() < previous { continue; }
                remaining -= excerpt.source.len() - previous;
                if at == 0 && previous > 0 { shown[index][0] = excerpt; } else { shown[index].push(excerpt); }
            }
        }
    }
    let hidden = outcome.files.len() - files.len();
    let mut lines = vec![
        format!("SessionKit search: showing {} of {} relevant files. Search status: {}.", files.len(), outcome.files.len(), outcome.status),
        "Ranking and roles are estimates. Inspect excerpts; read only missing context before editing.".into(),
    ];
    if hidden > 0 {
        lines.push(format!("Presentation limited: {hidden} additional file(s) not listed. Increase max_files (CLI: --max-files) with the same question to show more, or narrow the question/root. Hidden results are not evidence of absence."));
    }
    lines.push(format!("Source budget: {source_budget} bytes; {} bytes included. Partial snippets and locations-only entries require further reading.", source_budget - remaining));
    lines.push(format!("AGENTS.md lookup: {}{}.",
        if outcome.instruction_files.is_empty() { "none found".into() } else { outcome.instruction_files.iter().map(|p| js::quote(p)).collect::<Vec<_>>().join(", ") },
        if outcome.instruction_lookup_incomplete { "; incomplete" } else { "" }));
    for (kind, count) in &outcome.warnings { lines.push(format!("Warning: {}: {count}", js::quote(kind))); }
    for (kind, count) in &outcome.issues { lines.push(format!("Issue: {}: {count}", js::quote(kind))); }
    if let Some(error) = &outcome.provider_failure { lines.push(format!("Provider error: {}", js::quote(error))); }
    for (index, file) in files.iter().enumerate() {
        let roles = if file.roles.is_empty() { "role uncertain".into() } else { file.roles.join(", ") };
        let state = if shown[index].is_empty() { "locations only; no source included" } else { "source below" };
        lines.push(format!("\n- {} — {roles}; {state}", js::quote(&file.path)));
        let mut leads: Vec<_> = file.leads.iter().collect();
        leads.sort_by(|a, b| score(file, &b.range).partial_cmp(&score(file, &a.range)).unwrap_or(Ordering::Equal)
            .then_with(|| a.range.start.cmp(&b.range.start)));
        for lead in leads.iter().take(6) { lines.push(format!("  {}@{}-{}", js::quote(&lead.name), lead.range.start, lead.range.end)); }
        if leads.len() > 6 { lines.push(format!("  {} additional symbol locations not displayed.", leads.len() - 6)); }
        if file.source_omitted { lines.push("  Upstream source unavailable/omitted; verify this file directly.".into()); }
        for call in &file.call_leads {
            lines.push(format!("  Possible local call {} -> {}@{}-{}; runtime dispatch not verified{}.",
                js::quote(&call.caller), js::quote(&call.name), call.range.start, call.range.end,
                if call.unknown_earlier_bases.is_empty() { "" } else { "; earlier bases not inspected" }));
        }
    }
    for (index, file) in files.iter().enumerate() {
        for excerpt in &shown[index] { source_block(&mut lines, file, excerpt); }
    }
    // Test suggestions remain data, never executed, and only for displayed files.
    for path in &outcome.pytest_files {
        if files.iter().any(|file| &file.path == path) {
            lines.push(format!("Suggested test arguments (not executed): [\"python\", \"-m\", \"pytest\", \"-q\", {}]", js::quote(path)));
        }
    }
    format!("{}\n\nEnd context.\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::selection::{Decision, Lead};

    fn file(path: &str, source: &str, priority: f64) -> FileEvidence {
        let mut f = FileEvidence::admitted(path, "hash", priority);
        let range = EvidenceRange::lines(super::super::source::Range::new(1, source.lines().count().max(1)));
        f.excerpts.push(Excerpt { source: source.into(), range, partial: false });
        f.roles.push("implementation".into());
        f.leads.push(Lead { name: "locate".into(), range });
        f
    }
    fn outcome(files: Vec<FileEvidence>) -> Outcome {
        Outcome { status: "complete", files, issues: vec![], warnings: vec![], provider_failure: None,
            instruction_files: vec![], instruction_lookup_incomplete: false, pytest_files: vec![] }
    }

    #[test]
    fn large_first_file_does_not_starve_other_core_files() {
        let o = outcome(vec![file("Controller.php", &"unrelated();\n".repeat(1000), 0.99),
            file("Locator.php", "function locate() { return find_quote(); }", 0.98),
            file("Panel.tsx", "function PlacedQuote() { return evidence; }", 0.97)]);
        let result = render(&o, 300, 8);
        assert!(result.contains("find_quote()"));
        assert!(result.contains("return evidence"));
        assert!(result.contains("partial snippet"));
        assert!(!result.contains("300 bytes included")); // whole-line boundaries may leave slack
    }

    #[test]
    fn crop_starts_near_the_relevant_declaration_not_the_file_header() {
        let mut f = file("Controller.php", &format!("{}function locate() {{ return find_quote(); }}\n{}", "unrelated();\n".repeat(100), "trailing();\n".repeat(100)), 0.9);
        f.source_decisions.push(Decision { range: EvidenceRange::lines(super::super::source::Range::new(101, 101)), score: 0.99 });
        let result = render(&outcome(vec![f]), 100, 8);
        assert!(result.contains("function locate()"));
        assert!(result.contains("lines 99-"));
    }

    #[test]
    fn shortlist_preserves_total_and_incomplete_diagnostics() {
        let mut o = outcome((0..43).map(|i| file(&format!("file-{i:02}.php"), "code", 1.0 - i as f64 / 100.0)).collect());
        o.status = "incomplete";
        o.provider_failure = Some("provider unavailable".into());
        o.issues.push(("unjudged".into(), 2));
        let result = render(&o, 1000, 8);
        assert!(result.contains("showing 8 of 43"));
        assert!(result.contains("35 additional file(s)"));
        assert!(result.contains("provider unavailable"));
        assert!(result.contains("unjudged"));
        assert!(!result.contains("file-08.php"));
        assert!(render(&o, 1000, 43).contains("file-42.php"));
    }

    #[test]
    fn utf8_and_tiny_budgets_are_safe_and_do_not_invent_complete_lines() {
        let source = Excerpt { source: "ééé".into(), range: EvidenceRange { start: 20, end: 20, bytes: Some((50, 56)) }, partial: false };
        assert!(fit(&source, 1, 20).is_none());
        let fitted = fit(&source, 3, 20).unwrap();
        assert_eq!(fitted.source, "é");
        assert_eq!(fitted.range.bytes, Some((50, 52)));
        assert!(fitted.partial);
        assert!(render(&outcome(vec![]), 0, 8).contains("showing 0 of 0"));
        let o = outcome(vec![file("unicode.php", "ééé", 0.9), file("ascii.ts", "abc\n", 0.8)]);
        for budget in [0, 1, 2, 3, 4, 5, 7, 100] {
            let rendered = render(&o, budget, 8);
            let summary = rendered.lines().find(|line| line.starts_with("Source budget:")).unwrap();
            let included: usize = summary.split("bytes; ").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
            assert!(included <= budget);
        }
    }
}
