// Cost awareness, an experiment behind the plugin option costAwareness: what tokens cost, put in
// the agent's context at the moments it can act on it, in as few tokens as possible.
//
// Once per session and once per subagent: why it matters (REASON). After that only numbers:
// - the main session, with a message the person typed, when its context has doubled since the
//   last note (the settings hook's cost note, which otherwise comes with every message);
// - a subagent, when its own context has doubled; it hears nothing else about cost;
// - the main session, when a subagent ends: what that subagent took.
// Where the status line has learned what a call takes from the 5-hour limit, the share of the
// limit stands beside the dollars. Energy and water are named, not quantified: there are no
// published figures per token.

use crate::Result;
use crate::claude::{cache_dir, clock, limit_per_call, read_json, session_id, setting, tokens, usd};
use crate::js::{self, num};
use crate::next::transcript_of;
use crate::pricing::{Rate, Ttl};
use crate::transcript::Entry;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const REASON: &str = "sessionkit: tokens are scarce. The user has a limited budget per period, and every call \
    costs money, energy and water. Work efficiently, not sparingly: quality first, but read and delegate no more than \
    the task needs.";

fn note_file(id: &str) -> PathBuf {
    cache_dir().join("awareness").join(id)
}

/// Whether cost awareness is on for this session: the plugin marks it at session start.
pub fn on(id: &str) -> bool {
    !id.is_empty() && note_file(id).exists()
}

/// The context at which the first numbers come; after that, each doubling.
pub fn first_note_at() -> f64 {
    setting("COST_NOTE_ABOVE", 50_000.0)
}

/// For the plugin at session start: {session_id}; marks the session and answers the reason and
/// where the first note comes.
pub fn start(input: &Value) -> Result<Value> {
    mark(&note_file(&session_id(input)))?;
    Ok(json!({"reason": REASON, "first_note_at": num(first_note_at())}))
}

fn mark(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if !path.exists() {
        std::fs::write(path, "").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The share of the 5-hour limit `read` tokens take, and how much is used, from what the status
/// line learned for this session; empty without that.
pub fn limit_part(id: &str, read: f64) -> String {
    let Some(share) = limit_per_call(read) else { return String::new() };
    let state = read_json(&cache_dir().join("limit-state").join(format!("{id}.json"))).unwrap_or_default();
    let used = state.get("percentage").and_then(Value::as_f64);
    let reset = state.get("resetsAt").and_then(Value::as_f64).filter(|&at| js::truthy_number(at)).map(clock);
    let mut text = format!(", {} of the 5-hour limit", percent(share));
    if let Some(used) = used {
        text.push_str(&format!(" ({used:.0}% used{})", reset.map_or(String::new(), |at| format!(", resets {at}"))));
    }
    text
}

fn percent(share: f64) -> String {
    if share < 0.1 { "under 0.1%".into() } else { format!("{share:.1}%") }
}

/// For the main session's message hook: the note, or None when it is not due. The first note of
/// the session carries the reason; numbers come from first_note_at on, and again at each doubling.
pub fn main_note(id: &str, context: f64, numbers: Option<String>) -> Option<String> {
    main_note_at(&note_file(id), context, numbers)
}

fn main_note_at(path: &Path, context: f64, numbers: Option<String>) -> Option<String> {
    let noted = std::fs::read_to_string(path).ok()?;
    let last: Option<f64> = js::trim(&noted).parse().ok();
    let due = context >= first_note_at() && last.is_none_or(|last| context >= 2.0 * last);
    let first = noted.is_empty();
    if !due && !first {
        return None;
    }
    let mut parts = Vec::new();
    if first {
        parts.push(REASON.to_string());
    }
    if due {
        parts.push(numbers?);
    }
    let _ = std::fs::write(path, if due { js::round(context).to_string() } else { "0".into() });
    Some(parts.join(" "))
}

/// A subagent's transcript, by its agent id.
fn subagent_transcript(session: &str, agent: &str) -> Option<PathBuf> {
    let folder = transcript_of(session)?.with_extension("").join("subagents");
    [format!("agent-{agent}.jsonl"), format!("agent-a{agent}.jsonl")].into_iter()
        .map(|name| folder.join(name)).find(|path| path.exists())
}

/// What a subagent took: its calls, the tokens they read, written and produced, and their price.
pub struct Taken {
    pub calls: usize,
    pub tokens: f64,
    pub cost: f64,
    pub peak: f64,
}

pub fn taken(transcript: &Path) -> Taken {
    let text = std::fs::read_to_string(transcript).unwrap_or_default();
    let mut seen = HashSet::new();
    let mut taken = Taken { calls: 0, tokens: 0.0, cost: 0.0, peak: 0.0 };
    for call in text.lines().filter(|l| Entry::may_be_call(l)).filter_map(Entry::parse).filter_map(|e| e.api_call()) {
        if let Some(id) = &call.id && !seen.insert(id.clone()) {
            continue;
        }
        taken.calls += 1;
        taken.tokens += call.input + call.read + call.write() + call.output;
        taken.peak = taken.peak.max(call.context());
        if let Some(rate) = Rate::of(call.model.as_deref(), call.fast) {
            taken.cost += rate.input(call.input) + rate.read(call.read) + rate.write(call.write5m, Ttl::FiveMinutes)
                + rate.write(call.write1h, Ttl::OneHour) + rate.output(call.output);
        }
    }
    taken
}

/// For the plugin: {session_id, agent_id, kind, description, context, model}. `grown`: the
/// subagent's own note at a doubling of its context; `ended`: the main session's note on what
/// the subagent took. Answers {text}, empty when there is nothing to say.
pub fn agent(input: &Value) -> Result<Value> {
    let id = session_id(input);
    let text = match js::str_of(input, "kind") {
        "grown" => {
            let context = input.get("context").and_then(Value::as_f64).unwrap_or(0.0);
            let per_call = Rate::of(Some(js::str_of(input, "model")), false).map(|rate| rate.read(context));
            match per_call {
                Some(cost) => format!("sessionkit: your context is {} tokens; each of your calls reads all of it, about {}{}.",
                    tokens(context), usd(cost), limit_part(&id, context)),
                None => format!("sessionkit: your context is {} tokens; each of your calls reads all of it.", tokens(context)),
            }
        }
        "ended" => match subagent_transcript(&id, js::str_of(input, "agent_id")).map(|path| taken(&path)) {
            Some(taken) if taken.calls > 0 => format!("sessionkit: the subagent \"{}\" ended: {} model calls, {} tokens, about {}{}; its context peaked at {}.",
                js::clip(&js::collapse_space(js::str_of(input, "description")), 60), taken.calls, tokens(taken.tokens),
                usd(taken.cost), limit_part(&id, taken.tokens), tokens(taken.peak)),
            _ => String::new(),
        },
        other => return Err(format!("unknown kind {other:?}")),
    };
    Ok(json!({"text": text}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A marked session's note file, outside HOME so tests that set HOME do not move it.
    fn session() -> PathBuf {
        let path = std::env::temp_dir().join(format!("sessionkit-awareness-{}", uuid::Uuid::new_v4()));
        mark(&path).unwrap();
        path
    }

    #[test]
    fn the_reason_comes_once_and_numbers_only_at_each_doubling() {
        let path = session();
        let note = |context: f64| main_note_at(&path, context, Some("N".to_string()));
        assert_eq!(note(10_000.0).as_deref(), Some(REASON));
        assert_eq!(note(20_000.0), None);
        assert_eq!(note(60_000.0).as_deref(), Some("N"));
        assert_eq!(note(100_000.0), None);
        assert_eq!(note(120_000.0).as_deref(), Some("N"));
        assert_eq!(note(200_000.0), None);
        assert_eq!(note(240_000.0).as_deref(), Some("N"));
    }

    #[test]
    fn a_large_first_note_carries_reason_and_numbers_and_a_session_without_the_option_gets_none() {
        assert_eq!(main_note_at(&session(), 300_000.0, Some("N".into())).unwrap(), format!("{REASON} N"));
        let unmarked = std::env::temp_dir().join(format!("sessionkit-awareness-{}", uuid::Uuid::new_v4()));
        assert_eq!(main_note_at(&unmarked, 300_000.0, Some("N".into())), None);
    }

    #[test]
    fn what_a_subagent_took_is_summed_once_per_call() {
        let path = std::env::temp_dir().join(format!("sessionkit-taken-{}.jsonl", uuid::Uuid::new_v4()));
        let row = |id: &str, read: u64| format!(r#"{{"type":"assistant","timestamp":"2026-10-03T10:00:00Z","message":{{"id":"{id}","model":"claude-sonnet-5-5","usage":{{"input_tokens":10,"cache_read_input_tokens":{read},"cache_creation_input_tokens":0,"output_tokens":100}}}}}}"#);
        std::fs::write(&path, [row("m1", 20_000), row("m1", 20_000), row("m2", 40_000)].join("\n")).unwrap();
        let taken = taken(&path);
        assert_eq!(taken.calls, 2);
        assert_eq!(taken.tokens, 60_220.0);
        assert_eq!(taken.peak, 40_010.0);
        assert!(taken.cost > 0.0);
    }
}
