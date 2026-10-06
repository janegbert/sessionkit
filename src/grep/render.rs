// The stdout packet (jevgrep's cli/render.ts): a summary and the file list first, then verbatim
// source blocks, then declaration locations, so truncated output stays useful.

use super::retrieve::Outcome;
use super::selection::Excerpt;
use crate::js;

/// JSON.stringify, with C1 controls and bidirectional marks escaped as well.
pub(super) fn quote(value: &str) -> String {
    let mut out = String::new();
    for c in js::quote(value).chars() {
        let code = c as u32;
        if (0x7f..=0x9f).contains(&code) || (0x2028..=0x202e).contains(&code) || (0x2066..=0x2069).contains(&code) {
            out.push_str(&format!("\\u{code:04x}"));
        } else {
            out.push(c);
        }
    }
    out
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn render(outcome: &Outcome, max_source_bytes: usize) -> String {
    let mut remaining = if max_source_bytes == 0 { usize::MAX } else { max_source_bytes };
    let mut files: Vec<&super::selection::FileEvidence> = outcome.files.iter().collect();
    files.sort_by(|a, b| {
        let (pa, pb) = (a.priority.unwrap_or(a.score), b.priority.unwrap_or(b.score));
        pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| js::locale_compare(&a.path, &b.path))
    });
    let shown: Vec<(&super::selection::FileEvidence, Vec<&Excerpt>, bool)> = files.iter().map(|file| {
        let mut omitted = file.source_omitted;
        let excerpts = file.shown().iter().filter(|excerpt| {
            let bytes = excerpt.source.len();
            if bytes > remaining {
                omitted = true;
                return false;
            }
            remaining -= bytes;
            true
        }).collect();
        (*file, excerpts, omitted)
    }).collect();
    let omitted_count = shown.iter().filter(|(_, _, omitted)| *omitted).count();
    let mut lines: Vec<String> = vec![
        format!("Jevgrep: {} relevant files{}.", shown.len(), if outcome.status != "complete" { "; discovery incomplete" } else { "" }),
        "Symbols use name@start-end. Roles are estimates; locations-only files remain reading leads.".into(),
        format!("AGENTS.md lookup (root and returned-file ancestors): {}{}.",
            if outcome.instruction_files.is_empty() { "none found".into() } else { outcome.instruction_files.iter().map(|f| quote(f)).collect::<Vec<_>>().join(", ") },
            if outcome.instruction_lookup_incomplete { "; lookup incomplete" } else { "" }),
    ];
    if outcome.status == "interrupted" {
        lines.push("Interrupted.".into());
    }
    if omitted_count > 0 {
        lines.push(format!("Source omitted: {omitted_count} file(s)."));
    }
    for (kind, count) in &outcome.warnings {
        lines.push(format!("Warning: {}: {count}", quote(kind)));
    }
    for (kind, count) in &outcome.issues {
        lines.push(format!("Issue: {}: {count}", quote(kind)));
    }
    if let Some(failure) = &outcome.provider_failure {
        lines.push(format!("Provider error: {}", quote(failure)));
    }
    for path in &outcome.pytest_files {
        let unsafe_characters = path.chars().any(|c| (c as u32) < 32 || matches!(c as u32, 0x7f..=0x9f | 0x2028..=0x202e | 0x2066..=0x2069));
        lines.push(if unsafe_characters {
            format!("Suggested test arguments (not executed): [{}]", ["python", "-m", "pytest", "-q", path].iter().map(|a| quote(a)).collect::<Vec<_>>().join(", "))
        } else {
            format!("Suggested test entry point (not executed): python -m pytest -q {}", shell_quote(path))
        });
    }
    for (file, excerpts, omitted) in &shown {
        let roles = if file.roles.is_empty() { "relevant; role uncertain".to_string() } else { file.roles.join(", ") };
        let state = if !excerpts.is_empty() { "source below" } else if *omitted { "source omitted" } else { "locations only" };
        lines.push(format!("- {} — {roles}; {state}", quote(&file.path)));
    }
    lines.push("End file list. Declaration locations follow source.".into());
    for (file, excerpts, _) in &shown {
        for excerpt in excerpts {
            let byte_bounds = excerpt.range.bytes;
            let partial = excerpt.partial || byte_bounds.is_some();
            let annotation = if partial {
                format!(" (partial excerpt{})", byte_bounds.map_or(String::new(), |(a, b)| format!("; UTF-8 bytes [{a}, {b})")))
            } else {
                String::new()
            };
            let mut source_lines: Vec<&str> = excerpt.source.split('\n').collect();
            if partial && source_lines.len() > excerpt.range.end + 1 - excerpt.range.start && source_lines.last() == Some(&"") {
                source_lines.pop();
            }
            lines.push(String::new());
            lines.push(format!("Source block {} lines {}-{}{annotation}:", quote(&file.path), excerpt.range.start, excerpt.range.end));
            let longest = excerpt.source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(3.max(longest + 1));
            lines.push(fence.clone());
            lines.extend(source_lines.iter().map(|line| line.to_string()));
            lines.push(fence);
        }
    }
    lines.push(String::new());
    lines.push("Declaration locations:".into());
    for (file, _, omitted) in &shown {
        lines.push(format!("- {}", quote(&file.path)));
        let mut leads = file.leads.clone();
        leads.sort_by_key(|lead| lead.range.start);
        for lead in leads {
            lines.push(format!("  {}@{}-{}", lead.name, lead.range.start, lead.range.end));
        }
        for call in &file.call_leads {
            let bases = if call.unknown_earlier_bases.is_empty() {
                String::new()
            } else {
                format!("; earlier base(s) {} not inspected", call.unknown_earlier_bases.iter().map(|b| quote(b)).collect::<Vec<_>>().join(", "))
            };
            lines.push(format!("  Possible local call {} -> {}: lines {}-{}{bases}; runtime dispatch not verified.", call.caller, call.name, call.range.start, call.range.end));
        }
        if *omitted {
            lines.push("  Some source omitted; locations remain available.".into());
        }
    }
    format!("{}\n\nEnd context.\n", lines.join("\n"))
}
