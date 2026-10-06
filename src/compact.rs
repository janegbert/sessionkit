// sessionkit compact — compact a Claude Code conversation without a summary.
//
// Claude Code's own compaction asks a model to summarize the conversation. A summary is lossy: a
// path, an exact error, a constraint can disappear although it matters later. Here nothing is
// rewritten. Jev (TypeSafe) sees the whole conversation, with every tool output replaced by a
// short note, and for every tool call answers two questions: should the call stay, and should
// its full output stay verbatim. What it lets go is cut to its first lines or dropped with its
// call; all text of the person and of Claude stays as it was. A port of fast-jev-compaction.
//
// Claude Code hands the conversation over through a function hook (the plugin that sessionkit
// setup installs): `sessionkit compact --hook` reads the messages as JSON on stdin and prints a
// plan on stdout that says, per message, keep it, rebuild it from these parts, or leave it out.
// Anything that goes wrong makes the hook fall back to Claude Code's own summary.
//
// `sessionkit compact <session>` runs the same selection over a transcript and prints what it
// would keep, without changing anything.
//
// `sessionkit compact --fold` is for the end of a turn (the plugin's option foldTurns): without
// Jev, it cuts only the tool outputs of the last turn, so that everything before it stays in the
// prompt cache. See `fold`.

use crate::transcript::Entry as TranscriptEntry;
use crate::Result;
use crate::js;
use crate::jev::{PRICE_PER_MILLION_TOKENS, model, post};
use crate::start::{session_files};
use crate::usage::projects_dir;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::sync::LazyLock;

const USAGE: &str = "Usage: sessionkit compact <session> [--verbose]
       sessionkit compact --hook
       sessionkit compact --fold

Compacts a Claude Code conversation without a summary: Jev (TypeSafe) decides per tool call
whether the call and its output are still needed; the rest is cut or dropped, and all text stays
verbatim. In Claude Code this runs on /compact and auto-compaction, through the plugin that
sessionkit setup installs.

With a session (an id, an id prefix, a transcript file, or last) it shows what a compaction now
would keep, and changes nothing. That sends the conversation to TypeSafe and costs a little.

Options:
  --verbose   list the decision and both probabilities for every tool call
  --hook      read Claude Code's messages as JSON on stdin and print the plan (for the plugin)
  --fold      the same, for the last turn only and without Jev: every earlier message stays, and
              in the last turn the output of each call that changed no file is cut (for the plugin)
  --measure build <file>   collect labelled cuts from every session, split train/test (free)
  --measure run <file> <train|test|all> [max] [--out rows.json]
                           measure the questions on those cuts (costs money)
  -h, --help  show this help";

const KEEP_THRESHOLD: f64 = 0.5; // for the call question
// The result question answers around 0.7 for most outputs; at 0.78 it kept 40% of the outputs that
// came back and 10% of those that did not, on held-out cuts.
const KEEP_RESULT_THRESHOLD: f64 = 0.78;
pub const PRESERVE_RECENT_MESSAGES: usize = 6;
const MAX_STATE_TOKENS: usize = 25_000;
const MAX_REQUEST_TOKENS: usize = 30_000; // state plus questions, under Jev's 32k request limit
const REQUEST_OVERHEAD_TOKENS: usize = 20;
pub const TRUNCATE_HEAD_CHARS: usize = 300;
const MIN_REDUCTION: f64 = 0.25; // below this the summary of Claude Code frees more
const CONCURRENCY: usize = 6;
const INPUT_CHARS: [usize; 3] = [1000, 200, 60];
const TEXT_HEAD: usize = 400;
const TEXT_TAIL: usize = 150;

/// The wording of the state and the two questions. The state and the call question are
/// fast-jev-compaction's, the result question is measured (see below); a measurement can try another from a JSON file named in SESSIONKIT_COMPACT_WORDING, with the keys
/// context, call and result, where {id}, {tool}, {chars} and {input} (cut to 200 characters)
/// stand for the call's.
struct Wording {
    context: String,
    call: String,
    result: String,
}

static WORDING: LazyLock<Wording> = LazyLock::new(|| {
    let mut wording = Wording {
        context: STATE_CONTEXT.into(),
        call: "Tool call {id} ({tool}) should stay in the history: knowing this call was made, with its input, still matters for what the assistant does next".into(),
        // Measured against fast-jev's wording ("...re-running the tool would not do"), which put
        // every Read below 0.3: naming the input and asking about the work still in progress
        // ranked outputs that came back above those that did not at 0.70 against 0.56 on held-out
        // cuts (sessionkit compact --measure, 2026-09-29).
        result: "The output of tool call {id} ({tool}, input {input}, {chars} chars) concerns a file or subject that the assistant is still working on, as the goal and the newest messages show, so the assistant will likely need it again".into(),
    };
    let file = std::env::var("SESSIONKIT_COMPACT_WORDING").ok().and_then(|path| std::fs::read_to_string(path).ok()).and_then(|text| js::parse(&text));
    if let Some(file) = file {
        for (key, field) in [("context", &mut wording.context), ("call", &mut wording.call), ("result", &mut wording.result)] {
            if let Some(text) = file.get(key).and_then(Value::as_str) {
                *field = text.to_string();
            }
        }

    }
    wording
});

const STATE_CONTEXT: &str = "A coding assistant conversation is being compacted to free context. `history` is the whole conversation so far, oldest first; tool outputs are replaced by a short `result` note and long texts may be abridged. Each question asks whether one tool call, or the full output of that call, still needs to stay in the history verbatim. Whatever is not kept is deleted permanently, but the assistant can always re-run a tool or re-read a file.";

// ---------------------------------------------------------------- messages

pub struct ToolUse {
    pub id: String,
    pub tool: String,
    pub input: Value,
    pub text: Option<String>,
    pub is_error: bool,
}

pub struct ToolResult {
    pub id: String,
    pub text: String,
    pub is_error: bool,
}

pub struct Message {
    pub role: String,
    pub text: String,
    pub uses: Vec<ToolUse>,
    pub results: Vec<ToolResult>,
}

/// A message in the shape of Claude Code's SessionMessage.
fn message_of(value: &Value) -> Message {
    let list = |key: &str| value.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    Message {
        role: js::str_of(value, "role").to_string(),
        text: js::str_of(value, "text").to_string(),
        uses: list("toolUses").iter().map(|u| ToolUse {
            id: js::str_of(u, "tool_use_id").to_string(),
            tool: js::str_of(u, "tool").to_string(),
            input: u.get("input").cloned().unwrap_or_else(|| json!({})),
            text: u.get("text").and_then(Value::as_str).map(str::to_string),
            is_error: js::truthy(u.get("isError")),
        }).collect(),
        results: list("toolResults").iter().map(|r| ToolResult {
            id: js::str_of(r, "tool_use_id").to_string(),
            text: js::str_of(r, "text").to_string(),
            is_error: js::truthy(r.get("isError")),
        }).collect(),
    }
}

/// Characters of text, tool input and tool output a message holds.
fn message_chars(message: &Message) -> usize {
    js::len(&message.text) + message.uses.iter().map(|u| js::len(&js::stringify(&u.input))).sum::<usize>()
        + message.results.iter().map(|r| js::len(&r.text)).sum::<usize>()
}

// ---------------------------------------------------------------- calls

struct ToolCall {
    id: String,
    use_id: String,
    tool: String,
    input: Value,
    call_index: usize,
    result_chars: usize,
    is_error: bool,
    pinned: bool,
}

fn is_pinned(index: usize, total: usize) -> bool {
    index == 0 || index + PRESERVE_RECENT_MESSAGES >= total
}

/// Every tool call paired with its result. A call without a result is no candidate: there is
/// nothing to drop yet.
fn collect_calls(messages: &[Message]) -> Vec<ToolCall> {
    let mut results: HashMap<&str, (usize, &ToolResult)> = HashMap::new();
    for (index, message) in messages.iter().enumerate() {
        for result in &message.results {
            results.insert(&result.id, (index, result));
        }
    }
    let mut calls = Vec::new();
    for (call_index, message) in messages.iter().enumerate() {
        for tool in &message.uses {
            let Some(&(result_index, result)) = results.get(tool.id.as_str()) else { continue };
            calls.push(ToolCall {
                id: format!("t{}", calls.len() + 1),
                use_id: tool.id.clone(),
                tool: tool.tool.clone(),
                input: tool.input.clone(),
                call_index,
                result_chars: js::len(&result.text),
                is_error: result.is_error,
                pinned: is_pinned(call_index, messages.len()) || is_pinned(result_index, messages.len()),
            });
        }
    }
    calls
}

// ---------------------------------------------------------------- state

static TOKEN_PIECES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z]+|[0-9]+|[^\sA-Za-z0-9]").unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Tokens without a tokenizer: a word costs one token per six letters, a digit half a token, any
/// other symbol nine tenths. fast-jev-compaction calibrated this against the usage Jev reports:
/// it lands a little above the real count, where characters per token undercount JSON.
fn estimate_tokens(text: &str) -> usize {
    let mut tokens = 0.0;
    for piece in TOKEN_PIECES.find_iter(text) {
        let piece = piece.as_str();
        let first = piece.as_bytes()[0];
        tokens += if first.is_ascii_digit() {
            piece.len() as f64 / 2.0
        } else if first.is_ascii_alphabetic() {
            (1 + (piece.len() - 1) / 6) as f64
        } else {
            0.9
        };
    }
    tokens.ceil() as usize
}

fn truncate(text: &str, limit: usize) -> String {
    if js::len(text) <= limit { text.to_string() } else { format!("{}…", js::head(text, limit.saturating_sub(1))) }
}

fn abridge(text: &str) -> String {
    let length = js::len(text);
    if length <= TEXT_HEAD + TEXT_TAIL + 40 {
        return text.to_string();
    }
    format!("{}\n[… {} chars omitted …]\n{}", js::head(text, TEXT_HEAD), length - TEXT_HEAD - TEXT_TAIL, js::slice(text, length - TEXT_TAIL, length))
}

fn input_text(input: &Value, limit: usize) -> String {
    truncate(&js::stringify(input), limit)
}

/// One call as a single line, for when the structured form is too costly.
fn call_line(call: &ToolCall) -> String {
    let input: Vec<String> = call.input.as_object().into_iter().flatten().map(|(key, value)| {
        let text = match value.as_str() {
            Some(text) => text.to_string(),
            None => input_text(&json!({key.as_str(): value}), 200),
        };
        format!("{key}={}", SPACE.replace_all(&text, " "))
    }).collect();
    format!("{} {} {} → {} {}ch", call.id, call.tool, truncate(&input.join(" "), INPUT_CHARS[2]), if call.is_error { "error" } else { "ok" }, call.result_chars)
}

enum Calls {
    Full(Vec<Value>),
    Lines(Vec<String>),
}

struct Entry {
    i: usize,
    role: String,
    text: String,
    calls: Option<Calls>,
}

impl Entry {
    fn json(&self) -> Value {
        let mut entry = json!({"i": self.i, "role": self.role, "text": self.text});
        match &self.calls {
            Some(Calls::Full(calls)) => entry["tool_calls"] = Value::Array(calls.clone()),
            Some(Calls::Lines(lines)) => entry["tool_calls"] = json!(lines),
            None => {}
        }
        entry
    }

    fn tokens(&self) -> usize {
        estimate_tokens(&js::stringify(&self.json())) + 1
    }
}

fn calls_by_message(calls: &[ToolCall]) -> HashMap<usize, Vec<&ToolCall>> {
    let mut by_message: HashMap<usize, Vec<&ToolCall>> = HashMap::new();
    for call in calls {
        by_message.entry(call.call_index).or_default().push(call);
    }
    by_message
}

fn history(messages: &[Message], calls: &[ToolCall], input_chars: usize) -> Vec<Entry> {
    let by_message = calls_by_message(calls);
    messages.iter().enumerate().filter_map(|(i, message)| {
        let own: Vec<Value> = by_message.get(&i).into_iter().flatten().map(|call| json!({
            "id": call.id,
            "tool": call.tool,
            "input": input_text(&call.input, input_chars),
            "result": format!("{}, {} chars (omitted)", if call.is_error { "error" } else { "ok" }, call.result_chars),
        })).collect();
        if js::trim(&message.text).is_empty() && own.is_empty() {
            return None;
        }
        Some(Entry { i, role: message.role.clone(), text: message.text.clone(), calls: (!own.is_empty()).then_some(Calls::Full(own)) })
    }).collect()
}

/// The last three prompts of the person: what the conversation is about now.
pub fn goal(messages: &[Message]) -> String {
    let prompts: Vec<String> = messages.iter()
        .filter(|m| m.role == "user" && !js::trim(&m.text).is_empty() && m.results.is_empty())
        .map(|m| truncate(&m.text, 500)).collect();
    prompts[prompts.len().saturating_sub(3)..].join("\n")
}

struct Fitted {
    state: Value,
    tokens: usize,
    stage: &'static str,
}

/// The whole conversation as state, shrunk in stages until it fits MAX_STATE_TOKENS: tool
/// inputs cut shorter, then long texts abridged oldest first (pinned messages last), then old
/// texts replaced by a note, then old calls reduced to one line each, then old messages without
/// calls left out, then runs of old call-only messages folded together.
fn fit_state(messages: &[Message], calls: &[ToolCall]) -> Result<Fitted> {
    let goal = goal(messages);
    let state_of = |entries: &[Entry]| json!({"context": WORDING.context, "goal": goal, "history": entries.iter().map(Entry::json).collect::<Vec<_>>()});
    let base = estimate_tokens(&js::stringify(&state_of(&[])));
    let total = |entries: &[Entry]| base + entries.iter().map(Entry::tokens).sum::<usize>();

    for (n, &limit) in INPUT_CHARS.iter().enumerate() {
        let entries = history(messages, calls, limit);
        let tokens = total(&entries);
        if tokens <= MAX_STATE_TOKENS || n == INPUT_CHARS.len() - 1 {
            if tokens <= MAX_STATE_TOKENS {
                let stage = ["full", "inputs<=200", "inputs<=60"][n];
                return Ok(Fitted { state: state_of(&entries), tokens, stage });
            }
            return shrink(messages, calls, entries, tokens, &state_of, base);
        }
    }
    unreachable!()
}

fn shrink(messages: &[Message], calls: &[ToolCall], mut entries: Vec<Entry>, mut tokens: usize, state_of: &dyn Fn(&[Entry]) -> Value, base: usize) -> Result<Fitted> {
    let total = messages.len();
    let pinned = |entry: &Entry| is_pinned(entry.i, total);
    let mut per_entry: Vec<usize> = entries.iter().map(Entry::tokens).collect();
    let order: Vec<usize> = (0..entries.len()).filter(|&i| !pinned(&entries[i])).chain((0..entries.len()).filter(|&i| pinned(&entries[i]))).collect();
    let done = |entries: &[Entry], tokens: usize, stage: &'static str| Ok(Fitted { state: state_of(entries), tokens, stage });
    let mut change = |entries: &mut Vec<Entry>, index: usize, tokens: &mut usize, edit: &dyn Fn(&mut Entry)| {
        edit(&mut entries[index]);
        let now = entries[index].tokens();
        *tokens = *tokens + now - per_entry[index];
        per_entry[index] = now;
    };

    for &index in &order {
        if js::len(&entries[index].text) <= TEXT_HEAD + TEXT_TAIL + 40 {
            continue;
        }
        change(&mut entries, index, &mut tokens, &|e| e.text = abridge(&e.text));
        if tokens <= MAX_STATE_TOKENS {
            return done(&entries, tokens, "texts abridged");
        }
    }
    for &index in &order {
        if pinned(&entries[index]) || entries[index].text.is_empty() {
            continue;
        }
        let original = js::len(&messages[entries[index].i].text);
        change(&mut entries, index, &mut tokens, &|e| e.text = format!("[… {original} chars omitted …]"));
        if tokens <= MAX_STATE_TOKENS {
            return done(&entries, tokens, "old messages collapsed");
        }
    }
    let by_message = calls_by_message(calls);
    for &index in &order {
        let Some(own) = by_message.get(&entries[index].i).filter(|_| !pinned(&entries[index])) else { continue };
        let lines: Vec<String> = own.iter().map(|call| call_line(call)).collect();
        change(&mut entries, index, &mut tokens, &|e| e.calls = Some(Calls::Lines(lines.clone())));
        if tokens <= MAX_STATE_TOKENS {
            return done(&entries, tokens, "old calls compacted");
        }
    }
    let mut left = HashSet::new();
    for &index in &order {
        if pinned(&entries[index]) || entries[index].calls.is_some() {
            continue;
        }
        left.insert(index);
        tokens -= per_entry[index];
        if tokens <= MAX_STATE_TOKENS {
            let kept: Vec<Entry> = entries.into_iter().enumerate().filter(|(i, _)| !left.contains(i)).map(|(_, e)| e).collect();
            return done(&kept, tokens, "old messages left out");
        }
    }
    // Fold runs of old call-only entries of one role into one entry: the envelope once per run.
    let mut merged: Vec<Entry> = Vec::new();
    let foldable = |e: &Entry| !pinned(e) && e.text.is_empty() && matches!(e.calls, Some(Calls::Lines(_)));
    for (i, entry) in entries.into_iter().enumerate() {
        if left.contains(&i) {
            continue;
        }
        if let Some(previous) = merged.last_mut().filter(|p| foldable(p) && foldable(&entry) && p.role == entry.role) {
            if let (Some(Calls::Lines(into)), Some(Calls::Lines(lines))) = (&mut previous.calls, entry.calls) {
                into.extend(lines);
            }
            continue;
        }
        merged.push(entry);
    }
    let tokens = base + merged.iter().map(Entry::tokens).sum::<usize>();
    if tokens <= MAX_STATE_TOKENS {
        return done(&merged, tokens, "old calls merged");
    }
    Err(format!("history too large for Jev (~{tokens} tokens after shrinking, limit {MAX_STATE_TOKENS})"))
}

// ---------------------------------------------------------------- asking

/// The two questions about one call: keep the call, keep its output verbatim.
fn questions_for(call: &ToolCall) -> Map<String, Value> {
    let fill = |text: &str| text.replace("{id}", &call.id).replace("{tool}", &call.tool).replace("{chars}", &call.result_chars.to_string())
        .replace("{input}", &input_text(&call.input, 200));
    let mut questions = Map::new();
    questions.insert(format!("call_{}", call.id), json!({"type": "noul", "instructions": fill(&WORDING.call)}));
    questions.insert(format!("result_{}", call.id), json!({"type": "noul", "instructions": fill(&WORDING.result)}));
    questions
}

/// Split the candidates into requests that fit beside the state, which every request repeats.
fn batches<'a>(candidates: &[&'a ToolCall], state_tokens: usize) -> Result<Vec<Vec<&'a ToolCall>>> {
    let budget = MAX_REQUEST_TOKENS.saturating_sub(state_tokens + REQUEST_OVERHEAD_TOKENS);
    let mut out: Vec<Vec<&ToolCall>> = Vec::new();
    let (mut current, mut size) = (Vec::new(), 0);
    for &call in candidates {
        let tokens = estimate_tokens(&js::stringify(&Value::Object(questions_for(call))));
        if !current.is_empty() && size + tokens > budget {
            out.push(std::mem::take(&mut current));
            size = 0;
        }
        if current.is_empty() && tokens > budget {
            return Err(format!("the state leaves no room for questions (~{state_tokens} of {MAX_REQUEST_TOKENS} tokens)"));
        }
        current.push(call);
        size += tokens;
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    Pinned,
    Keep,
    DropResult,
    DropCall,
}

struct Decision {
    action: Action,
    keep_call: f64,
    keep_result: f64,
}

fn decide(pinned: bool, keep_call: f64, keep_result: f64) -> Decision {
    let action = if pinned {
        Action::Pinned
    } else if keep_result >= KEEP_RESULT_THRESHOLD {
        Action::Keep
    } else if keep_call >= KEEP_THRESHOLD {
        Action::DropResult
    } else {
        Action::DropCall
    };
    Decision { action, keep_call, keep_result }
}

// ---------------------------------------------------------------- the plan

fn cut(text: &str, is_error: bool) -> String {
    let length = js::len(text);
    if length <= TRUNCATE_HEAD_CHARS + 120 {
        return text.to_string();
    }
    format!("{}\n[sessionkit compact cut {} chars of this tool result{}; re-run the tool if needed]",
        js::head(text, TRUNCATE_HEAD_CHARS), length - TRUNCATE_HEAD_CHARS, if is_error { " (error)" } else { "" })
}

/// Per message: keep it as it is ({"keep": i}), rebuild it from its parts ({"from": i, ...}),
/// or leave it out. A part is {"keep": j}, the j-th tool block as it was, or {"keep": j, "text"},
/// the same block with its output cut. A dropped call goes together with its result, so no
/// result is ever left without its call. Also returns the characters left.
fn plan(messages: &[Message], actions: &HashMap<&str, Action>) -> (Vec<Value>, usize) {
    let mut out = Vec::new();
    let mut chars = 0;
    for (i, message) in messages.iter().enumerate() {
        let action = |id: &str| actions.get(id).copied().unwrap_or(Action::Keep);
        let mut changed = false;
        let mut size = js::len(&message.text);
        let mut uses = Vec::new();
        for (j, tool) in message.uses.iter().enumerate() {
            match action(&tool.id) {
                Action::DropCall => changed = true,
                Action::DropResult => {
                    let text = tool.text.as_deref().unwrap_or("");
                    let short = cut(text, tool.is_error);
                    size += js::len(&js::stringify(&tool.input));
                    if short != text {
                        changed = true;
                        uses.push(json!({"keep": j, "text": short}));
                    } else {
                        uses.push(json!({"keep": j}));
                    }
                }
                _ => {
                    size += js::len(&js::stringify(&tool.input));
                    uses.push(json!({"keep": j}));
                }
            }
        }
        let mut results = Vec::new();
        for (j, result) in message.results.iter().enumerate() {
            match action(&result.id) {
                Action::DropCall => changed = true,
                Action::DropResult => {
                    let short = cut(&result.text, result.is_error);
                    size += js::len(&short);
                    if short != result.text {
                        changed = true;
                        results.push(json!({"keep": j, "text": short}));
                    } else {
                        results.push(json!({"keep": j}));
                    }
                }
                _ => {
                    size += js::len(&result.text);
                    results.push(json!({"keep": j}));
                }
            }
        }
        if !changed {
            out.push(json!({"keep": i}));
        } else if js::trim(&message.text).is_empty() && uses.is_empty() && results.is_empty() {
            continue;
        } else {
            out.push(json!({"from": i, "toolUses": uses, "toolResults": results}));
        }
        chars += size;
    }
    (out, chars)
}

pub struct Compaction {
    plan: Vec<Value>,
    messages_before: usize,
    chars_before: usize,
    chars_after: usize,
    counts: [usize; 4],
    state_tokens: usize,
    stage: &'static str,
    requests: usize,
    tokens: f64,
    /// Per call: its id in the state, its tool_use id, the tool and the decision.
    decisions: Vec<(String, String, String, Decision)>,
}

impl Compaction {
    fn reduction(&self) -> f64 {
        if self.chars_before == 0 { 0.0 } else { (self.chars_before - self.chars_after) as f64 / self.chars_before as f64 }
    }

    fn summary(&self) -> String {
        let [pinned, kept, cut, dropped] = self.counts;
        let parts: Vec<String> = [(kept, "kept"), (cut, "outputs cut"), (dropped, "calls dropped"), (pinned, "pinned")].iter()
            .filter(|(n, _)| *n > 0).map(|(n, label)| format!("{n} {label}")).collect();
        format!("{}% smaller; {}; state ~{} tokens ({}) in {} request(s), ${}",
            js::round(self.reduction() * 100.0), if parts.is_empty() { "no tool calls".into() } else { parts.join(", ") },
            self.state_tokens, if self.stage.is_empty() { "none" } else { self.stage }, self.requests,
            js::fixed(self.tokens / 1e6 * PRICE_PER_MILLION_TOKENS, 4))
    }
}

/// Ask Jev about every call outside the pinned first and newest messages, and make the plan.
pub fn compact(messages: &[Message]) -> Result<Compaction> {
    let calls = collect_calls(messages);
    let candidates: Vec<&ToolCall> = calls.iter().filter(|c| !c.pinned).collect();
    let mut answers: HashMap<String, (f64, f64)> = HashMap::new();
    let (mut state_tokens, mut stage, mut requests, mut tokens) = (0, "", 0, 0.0);
    if !candidates.is_empty() {
        let fitted = fit_state(messages, &calls)?;
        let groups = batches(&candidates, fitted.tokens)?;
        let model = model();
        let replies = js::pool(&groups, CONCURRENCY, |group, _| -> Result<(Vec<(String, f64, f64)>, f64)> {
            let questions: Map<String, Value> = group.iter().flat_map(|call| questions_for(call)).collect();
            let data = post(&json!({"model": model, "state": fitted.state, "questions": questions}))?;
            let noul = |name: String| data["answers"][&name]["noul"].as_f64().filter(|p| p.is_finite()).ok_or(format!("Jev gave no answer for {name}"));
            let found = group.iter().map(|call| Ok((call.id.clone(), noul(format!("call_{}", call.id))?, noul(format!("result_{}", call.id))?)))
                .collect::<Result<Vec<_>>>()?;
            Ok((found, data["usage"]["input_tokens"].as_f64().unwrap_or(0.0)))
        });
        for reply in replies {
            let (found, used) = reply?;
            tokens += used;
            for (id, keep_call, keep_result) in found {
                answers.insert(id, (keep_call, keep_result));
            }
        }
        (state_tokens, stage, requests) = (fitted.tokens, fitted.stage, groups.len());
    }
    let mut counts = [0; 4];
    let mut actions: HashMap<&str, Action> = HashMap::new();
    let mut decisions = Vec::new();
    for call in &calls {
        let (keep_call, keep_result) = answers.get(&call.id).copied().unwrap_or((1.0, 1.0));
        let decision = decide(call.pinned, keep_call, keep_result);
        counts[decision.action as usize] += 1;
        actions.insert(&call.use_id, decision.action);
        decisions.push((call.id.clone(), call.use_id.clone(), call.tool.clone(), decision));
    }
    let (plan, chars_after) = plan(messages, &actions);
    Ok(Compaction {
        plan,
        messages_before: messages.len(),
        chars_before: messages.iter().map(message_chars).sum(),
        chars_after,
        counts,
        state_tokens,
        stage,
        requests,
        tokens,
        decisions,
    })
}

// ---------------------------------------------------------------- the fold

/// Tools whose call changes a file: a fold keeps their output.
const CHANGES_FILES: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

struct Fold {
    plan: Vec<Value>,
    /// Outputs cut, and the characters that freed.
    outputs: usize,
    chars: usize,
}

/// Fold the last turn, right after it ended: every message before its prompt stays as the engine
/// has it, so the prompt cache still holds for them, and in the turn itself the output of every
/// call that only looked is cut to its first lines. Only messages that hold tool results are
/// rebuilt. A rebuilt message of the assistant loses its thinking block, and Claude Code then
/// sends the whole conversation without thinking, which writes all of it to the cache again
/// (measured with Claude Code 2.1.287). So every call stays, with its input.
///
/// Without Jev: a call that changed a file keeps its output, any other loses it. The next step
/// is to ask Jev the result question of `compact` for the calls of this one turn.
fn fold(messages: &[Message]) -> Fold {
    let start = messages.iter().rposition(|m| m.role == "user" && m.results.is_empty() && !js::trim(&m.text).is_empty()).unwrap_or(0);
    let looked: HashSet<&str> = messages[start..].iter().flat_map(|m| &m.uses)
        .filter(|u| !CHANGES_FILES.contains(&u.tool.as_str())).map(|u| u.id.as_str()).collect();
    let (mut outputs, mut chars) = (0, 0);
    let plan = messages.iter().enumerate().map(|(i, message)| {
        let results: Vec<Value> = message.results.iter().enumerate().map(|(j, result)| {
            let short = cut(&result.text, result.is_error);
            if i < start || !looked.contains(result.id.as_str()) || short == result.text {
                return json!({"keep": j});
            }
            outputs += 1;
            chars += js::len(&result.text) - js::len(&short);
            json!({"keep": j, "text": short})
        }).collect();
        if results.iter().any(|r| r.get("text").is_some()) { json!({"from": i, "toolUses": [], "toolResults": results}) } else { json!({"keep": i}) }
    }).collect();
    Fold { plan, outputs, chars }
}

// ---------------------------------------------------------------- commands

/// The smallest reduction worth replacing the summary for. A build with the test-api feature
/// takes it from SESSIONKIT_COMPACT_MIN, so a test can compare every plan with fast-jev's.
fn min_reduction() -> f64 {
    #[cfg(feature = "test-api")]
    if let Ok(value) = std::env::var("SESSIONKIT_COMPACT_MIN") {
        return js::parse_number(Some(&value));
    }
    MIN_REDUCTION
}

/// The messages the plugin hands over on stdin.
fn stdin_messages() -> Result<Vec<Message>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| e.to_string())?;
    let data = js::parse(&input).ok_or("stdin is not JSON")?;
    Ok(data["messages"].as_array().ok_or("no messages on stdin")?.iter().map(message_of).collect())
}

/// For the plugin, after a turn: messages on stdin, the plan of the fold or the reason to leave
/// the conversation as it is on stdout.
fn fold_hook() -> Result<()> {
    let folded = fold(&stdin_messages()?);
    let answer = if folded.outputs == 0 {
        json!({"fallback": "no long tool output in the last turn"})
    } else {
        json!({"plan": folded.plan, "summary": format!("folded the last turn: {} tool output(s) cut, {} characters", folded.outputs, js::grouped(folded.chars as f64))})
    };
    println!("{}", js::stringify(&answer));
    Ok(())
}

/// For the plugin: messages on stdin, the plan or the reason to fall back on stdout.
fn hook() -> Result<()> {
    let messages = stdin_messages()?;
    let answer = match compact(&messages) {
        Err(error) => json!({"fallback": error}),
        Ok(result) if result.reduction() < min_reduction() => {
            json!({"fallback": format!("below the minimum of {}%: {}", js::round(MIN_REDUCTION * 100.0), result.summary())})
        }
        Ok(result) => {
            let kept = result.plan.len();
            let [pinned, kept_calls, cut, dropped] = result.counts;
            json!({
                "plan": result.plan,
                "summary": format!("kept {kept}/{} messages, no summary ({})", result.messages_before, result.summary()),
                "counts": {"pinned": pinned, "kept": kept_calls, "cut": cut, "dropped": dropped},
            })
        }
    };
    println!("{}", js::stringify(&answer));
    Ok(())
}

/// A transcript file: a path, `last` for the newest in this folder, or an id or id prefix.
fn transcript(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(name);
    if path.is_file() {
        return Ok(path);
    }
    if name == "last" {
        return session_files(false, 1)?.into_iter().next().ok_or("no sessions in this folder".into());
    }
    let mut found = Vec::new();
    for folder in js::read_dir_names(&projects_dir())? {
        for file in js::read_dir_names(&projects_dir().join(&folder)).unwrap_or_default() {
            if file.starts_with(name) && file.ends_with(".jsonl") {
                found.push(projects_dir().join(&folder).join(file));
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!("no session {name}")),
        n => Err(format!("{n} sessions start with {name}; give more of the id")),
    }
}

fn joined_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| js::str_of(b, "type") == "text").map(|b| js::str_of(b, "text")).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The conversation since the last compaction boundary, as Claude Code hands it to the hook: one message
/// per transcript entry, so the thinking, text and tool blocks of one response come apart.
fn read_transcript(path: &PathBuf) -> Result<Vec<Message>> {
    Ok(read_segments(path)?.pop().unwrap_or_default())
}

/// Every stretch of the conversation between two compaction boundaries, oldest first: each is a
/// conversation as the hook would have been handed it at its end.
pub fn read_segments(path: &PathBuf) -> Result<Vec<Vec<Message>>> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut segments = vec![Vec::new()];
    for entry in text.lines().filter_map(TranscriptEntry::parse).filter(|e| !e.is_sidechain()) {
        if entry.compaction().is_some() {
            segments.push(Vec::new());
        } else if ["user", "assistant"].contains(&entry.kind()) {
            segments.last_mut().unwrap().push(message_from(entry.value()));
        }
    }
    Ok(segments)
}

fn message_from(entry: &Value) -> Message {
    let mut message = Message { role: js::str_of(entry, "type").to_string(), text: String::new(), uses: Vec::new(), results: Vec::new() };
    let blocks = match &entry["message"]["content"] {
        Value::String(text) => vec![json!({"type": "text", "text": text})],
        Value::Array(blocks) => blocks.clone(),
        _ => Vec::new(),
    };
    let mut texts = Vec::new();
    for block in blocks {
        match js::str_of(&block, "type") {
            "text" => texts.push(js::str_of(&block, "text").to_string()),
            "tool_use" => message.uses.push(ToolUse {
                id: js::str_of(&block, "id").to_string(),
                tool: js::str_of(&block, "name").to_string(),
                input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                text: None,
                is_error: false,
            }),
            "tool_result" => message.results.push(ToolResult {
                id: js::str_of(&block, "tool_use_id").to_string(),
                text: joined_text(&block["content"]),
                is_error: js::truthy(block.get("is_error")),
            }),
            _ => {}
        }
    }
    message.text = texts.join("\n");
    message
}

pub fn compact_main(argv: &[String]) -> Result<()> {
    let mut verbose = false;
    let mut session = None;
    for arg in argv {
        match arg.as_str() {
            "--hook" => return hook(),
            "--fold" => return fold_hook(),
            "--measure" => return measure(&argv[1..]),
            "--verbose" => verbose = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if other.starts_with("--") => {
                eprintln!("sessionkit compact: unknown option {other}\n\n{USAGE}");
                std::process::exit(2);
            }
            other => session = Some(other.to_string()),
        }
    }
    let Some(session) = session else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let path = transcript(&session)?;
    let messages = read_transcript(&path)?;
    let result = compact(&messages)?;
    if verbose {
        for (id, _, tool, d) in result.decisions.iter().filter(|(_, _, _, d)| d.action != Action::Pinned) {
            println!("{id:>5} {:<12} {:<12} call {}  output {}", js::clip(tool, 12), format!("{:?}", d.action), js::fixed(d.keep_call, 2), js::fixed(d.keep_result, 2));
        }
        println!();
    }
    let kept = result.plan.len();
    println!("{}\n{} of {} messages, {} of {} characters: {}", path.display(), kept, result.messages_before,
        js::grouped(result.chars_after as f64), js::grouped(result.chars_before as f64), result.summary());
    if result.reduction() < MIN_REDUCTION {
        println!("Below {}%: Claude Code would use its own summary here.", js::round(MIN_REDUCTION * 100.0));
    }
    Ok(())
}

// ---------------------------------------------------------------- measuring

/// What happened to a file after the cut, for a Read before it: edited before it was read again
/// (its contents were still needed from the context), read again, touched by another tool (a grep,
/// a shell command), or never touched again. The first three came back: dropping their output
/// costs a read later. Dropping an unused output is free.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Label {
    Needed,
    Reread,
    Touched,
    Unused,
}

fn came_back(label: Label) -> bool {
    label != Label::Unused
}

fn file_of(input: &Value) -> &str {
    input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Value::as_str).unwrap_or("")
}

fn label(path: &str, after: &[Message]) -> Option<Label> {
    let name = path.rsplit('/').next().unwrap_or(path);
    // The folder with the name: a bare name like mod.rs or index.ts may be another file.
    let tail = path.rsplitn(3, '/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
    let mut mentioned = false;
    for message in after {
        for tool in &message.uses {
            let file = file_of(&tool.input);
            if file == path && tool.tool == "Read" {
                return Some(Label::Reread);
            }
            if file == path && CHANGES_FILES.contains(&tool.tool.as_str()) {
                return Some(Label::Needed);
            }
            let input = js::stringify(&tool.input);
            if input.contains(&tail) {
                return Some(Label::Touched);
            }
            if input.contains(name) {
                return None; // the name, but maybe of another file: no clean label
            }
        }
        mentioned |= message.text.contains(name);
    }
    // Named in text but never opened again: whether it was needed is not clear from code.
    (!mentioned).then_some(Label::Unused)
}

/// The share of (needed, unused) pairs in which the needed Read has the higher probability.
fn auc(needed: &[f64], unused: &[f64]) -> Option<f64> {
    if needed.is_empty() || unused.is_empty() {
        return None;
    }
    let mut wins = 0.0;
    for n in needed {
        for u in unused {
            wins += if n > u { 1.0 } else if n == u { 0.5 } else { 0.0 };
        }
    }
    Some(wins / (needed.len() * unused.len()) as f64)
}

fn median(values: &mut [f64]) -> String {
    if values.is_empty() {
        return "—".into();
    }
    values.sort_by(f64::total_cmp);
    js::fixed(values[values.len() / 2], 2)
}

/// Cuts in a stretch of conversation, before which a Read came back later and another did not.
/// Auto-compaction strikes in the middle of a run, so a cut is any user message (a prompt or tool
/// results) between 5% and 95% of the stretch, spread evenly. Each cut keeps its labelled Reads by
/// tool use id.
fn cuts_of(messages: &[Message], per_session: usize) -> Vec<(usize, Vec<(String, Label)>)> {
    let points: Vec<usize> = (0..messages.len())
        .filter(|&i| messages[i].role == "user" && i * 20 >= messages.len() && i * 20 <= messages.len() * 19).collect();
    let step = (points.len() / (per_session * 4).max(1)).max(1);
    let prompts: Vec<usize> = points.into_iter().step_by(step).collect();
    let mut cuts = Vec::new();
    for &cut in &prompts {
        if cuts.len() >= per_session {
            break;
        }
        let (before, after) = messages.split_at(cut);
        // Only Reads compaction may touch: not in the first message or the newest ones.
        let reads: Vec<(String, Label)> = before.iter().enumerate().filter(|(i, _)| !is_pinned(*i, before.len()))
            .flat_map(|(_, m)| &m.uses).filter(|u| u.tool == "Read" && labelled_kind(file_of(&u.input)))
            .filter_map(|u| Some((u.id.clone(), label(file_of(&u.input), after)?))).collect();
        if reads.iter().any(|r| came_back(r.1)) && reads.iter().any(|r| r.1 == Label::Unused) {
            cuts.push((cut, reads));
        }
    }
    cuts
}

/// A file whose Read says something about need: not an image, which is almost never used again
/// and would make the measurement easy, and not the output of a background task, which is read
/// again because it changed.
fn labelled_kind(path: &str) -> bool {
    let lower = path.to_lowercase();
    let image = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".pdf"].iter().any(|ext| lower.ends_with(ext));
    let task_output = lower.contains("/tasks/") && lower.ends_with(".output");
    !image && !task_output
}

fn label_of(name: &str) -> Option<Label> {
    [Label::Needed, Label::Reread, Label::Touched, Label::Unused].into_iter().find(|label| format!("{label:?}") == name)
}

/// sessionkit compact --measure build <file>: the labelled cuts of every session, split into train
/// and test by session, so that cuts of one session never land on both sides. Local and free.
fn build_dataset(file: &str, per_session: usize) -> Result<()> {
    let paths = session_files(true, usize::MAX)?;
    let found = js::pool(&paths, 8, |path, _| {
        let segments = read_segments(path).unwrap_or_default();
        let mut cuts = Vec::new();
        for (segment, messages) in segments.iter().enumerate() {
            cuts.extend(cuts_of(messages, per_session.saturating_sub(cuts.len())).into_iter().map(|(cut, reads)| (segment, cut, reads)));
        }
        cuts
    });
    let mut cuts = Vec::new();
    let mut counts: HashMap<(&str, String), usize> = HashMap::new();
    for (path, session_cuts) in paths.iter().zip(found) {
        let session = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let split = if u8::from_str_radix(&js::sha256_hex(session.as_bytes())[..2], 16).unwrap_or(0) % 2 == 0 { "train" } else { "test" };
        for (segment, cut, reads) in session_cuts {
            for (_, label) in &reads {
                *counts.entry((split, format!("{label:?}"))).or_default() += 1;
            }
            let reads: Map<String, Value> = reads.into_iter().map(|(id, label)| (id, format!("{label:?}").into())).collect();
            cuts.push(json!({"transcript": path.to_string_lossy(), "session": session, "segment": segment, "cut": cut, "split": split, "reads": reads}));
        }
    }
    for split in ["train", "test"] {
        let n = cuts.iter().filter(|c| c["split"] == split).count();
        let label = |name: &str| counts.get(&(split, name.to_string())).copied().unwrap_or(0);
        println!("{split}: {n} cuts; Reads needed {}, reread {}, touched {}, unused {}", label("Needed"), label("Reread"), label("Touched"), label("Unused"));
    }
    std::fs::write(file, js::stringify_pretty(&json!({"cuts": cuts}))).map_err(|e| e.to_string())
}

/// sessionkit compact --measure run <file> <train|test|all> [max cuts] [--out rows.json]: compact
/// each cut with the current wording and compare the probabilities of Needed and Unused Reads.
fn run_dataset(argv: &[String]) -> Result<()> {
    let file = argv.first().ok_or("--measure run takes a dataset file")?;
    let split = argv.get(1).map_or("train", String::as_str);
    let max = argv.get(2).and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
    let out = argv.iter().position(|a| a == "--out").and_then(|i| argv.get(i + 1));
    let data = js::parse(&std::fs::read_to_string(file).map_err(|e| e.to_string())?).ok_or("the dataset is not JSON")?;
    // At most two cuts of a session, so that one long session does not decide the outcome.
    let mut per_session: HashMap<String, usize> = HashMap::new();
    let cuts: Vec<&Value> = data["cuts"].as_array().into_iter().flatten().filter(|c| split == "all" || c["split"] == split)
        .filter(|c| {
            let seen = per_session.entry(js::str_of(c, "session").to_string()).or_default();
            *seen += 1;
            *seen <= 2
        })
        .take(max).collect();
    if cuts.is_empty() {
        return Err(format!("no {split} cuts in {file}"));
    }
    // One cut at a time: compact() already sends its requests in parallel.
    let mut rows: Vec<(usize, Label, f64, f64)> = Vec::new();
    let mut saved = Vec::new();
    let mut cost = 0.0;
    for (n, cut) in cuts.iter().enumerate() {
        let segments = read_segments(&PathBuf::from(js::str_of(cut, "transcript")))?;
        let Some(messages) = segments.get(cut["segment"].as_u64().unwrap_or(0) as usize) else { continue };
        let at = cut["cut"].as_u64().unwrap_or(0) as usize;
        if at > messages.len() {
            continue; // the transcript changed since the dataset was built
        }
        let result = compact(&messages[..at])?;
        // How far back each call sits, in messages: the free rule to beat is that recent work comes back.
        let age: HashMap<&str, usize> = messages[..at].iter().enumerate().flat_map(|(i, m)| m.uses.iter().map(move |u| (u.id.as_str(), at - i))).collect();
        cost += result.tokens / 1e6 * PRICE_PER_MILLION_TOKENS;
        for (_, use_id, _, d) in &result.decisions {
            let Some(label) = cut["reads"].get(use_id).and_then(Value::as_str).and_then(label_of) else { continue };
            if d.action != Action::Pinned {
                rows.push((n, label, d.keep_call, d.keep_result));
                saved.push(json!({"cut": n, "use_id": use_id, "label": format!("{label:?}"), "call": d.keep_call, "result": d.keep_result,
                    "age": age.get(use_id.as_str()).copied().unwrap_or(0)}));
            }
        }
        eprint!("\rcut {} of {}", n + 1, cuts.len());
    }
    eprintln!();
    if let Some(out) = out {
        std::fs::write(out, js::stringify(&Value::Array(saved))).map_err(|e| e.to_string())?;
    }
    println!("{split}: {} cuts, {} labelled Reads, ${}\n", cuts.len(), rows.len(), js::fixed(cost, 4));
    println!("{:<8} {:>5} {:>12} {:>14} {:>16}", "label", "n", "median call", "median output", "kept");
    for label in [Label::Needed, Label::Reread, Label::Touched, Label::Unused] {
        let of: Vec<&(usize, Label, f64, f64)> = rows.iter().filter(|r| r.1 == label).collect();
        let kept = of.iter().filter(|r| r.3 >= KEEP_RESULT_THRESHOLD || r.2 >= KEEP_THRESHOLD).count();
        println!("{:<8} {:>5} {:>12} {:>14} {:>16}", format!("{label:?}"), of.len(), median(&mut of.iter().map(|r| r.2).collect::<Vec<_>>()),
            median(&mut of.iter().map(|r| r.3).collect::<Vec<_>>()), format!("{kept} of {}", of.len()));
    }
    // Within each cut: does a Read that came back rank above an unused one? A cut compares Reads
    // that saw the same state, so the model's level on that cut drops out.
    let (mut call_aucs, mut output_aucs) = (Vec::new(), Vec::new());
    for n in 0..cuts.len() {
        let pick = |back: bool, by: fn(&(usize, Label, f64, f64)) -> f64| rows.iter().filter(|r| r.0 == n && came_back(r.1) == back).map(by).collect::<Vec<_>>();
        if let Some(a) = auc(&pick(true, |r| r.2), &pick(false, |r| r.2)) { call_aucs.push(a) }
        if let Some(a) = auc(&pick(true, |r| r.3), &pick(false, |r| r.3)) { output_aucs.push(a) }
    }
    let mean = |v: &[f64]| if v.is_empty() { "—".to_string() } else { js::fixed(v.iter().sum::<f64>() / v.len() as f64, 3) };
    println!("\nCame back above unused, within a cut (0.5 is chance), over {} cuts: call mean {}, output mean {}",
        call_aucs.len(), mean(&call_aucs), mean(&output_aucs));
    Ok(())
}

/// sessionkit compact --measure build|run: the measurement of the two questions.
fn measure(argv: &[String]) -> Result<()> {
    match argv.first().map(String::as_str) {
        Some("build") => build_dataset(argv.get(1).ok_or("--measure build takes a file to write")?, 8),
        Some("run") => run_dataset(&argv[1..]),
        _ => Err("use --measure build <file>, or --measure run <file> <train|test|all> [max cuts] [--out rows.json]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(role: &str, text: &str) -> Message {
        Message { role: role.into(), text: text.into(), uses: Vec::new(), results: Vec::new() }
    }

    fn call(id: &str, output: &str) -> [Message; 2] {
        let mut use_ = text("assistant", "");
        use_.uses.push(ToolUse { id: id.into(), tool: "Read".into(), input: json!({"file_path": "a.rs"}), text: Some(output.into()), is_error: false });
        let mut result = text("user", "");
        result.results.push(ToolResult { id: id.into(), text: output.into(), is_error: false });
        [use_, result]
    }

    fn conversation(outputs: usize) -> Vec<Message> {
        let mut messages = vec![text("user", "fix the test")];
        for n in 0..outputs {
            messages.extend(call(&format!("u{n}"), &"line\n".repeat(200)));
        }
        messages.extend((0..PRESERVE_RECENT_MESSAGES).map(|_| text("assistant", "done")));
        messages
    }

    #[test]
    fn tokens_are_estimated_as_fast_jev_does() {
        assert_eq!(estimate_tokens("hello"), 1);
        assert_eq!(estimate_tokens("abcdefghijklm"), 3);
        assert_eq!(estimate_tokens("1234"), 2);
        assert_eq!(estimate_tokens("{}"), 2);
    }

    #[test]
    fn the_first_and_newest_messages_are_pinned() {
        let messages = conversation(3);
        let calls = collect_calls(&messages);
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|c| !c.pinned));
        let short = conversation(0);
        assert!(is_pinned(0, short.len()) && is_pinned(short.len() - 1, short.len()));
    }

    #[test]
    fn decisions_follow_the_threshold() {
        assert_eq!(decide(false, 0.9, 0.8).action, Action::Keep);
        assert_eq!(decide(false, 0.9, 0.7).action, Action::DropResult); // the result question has its own, higher threshold
        assert_eq!(decide(false, 0.9, 0.2).action, Action::DropResult);
        assert_eq!(decide(false, 0.1, 0.2).action, Action::DropCall);
        assert_eq!(decide(true, 0.0, 0.0).action, Action::Pinned);
    }

    #[test]
    fn the_plan_cuts_outputs_and_drops_calls_with_their_results() {
        let messages = conversation(3);
        let actions: HashMap<&str, Action> = [("u0", Action::DropResult), ("u1", Action::DropCall)].into_iter().collect();
        let (plan, chars) = plan(&messages, &actions);
        assert_eq!(plan[0], json!({"keep": 0}));
        assert_eq!(plan[1]["from"], 1);
        assert!(plan[1]["toolUses"][0]["text"].as_str().unwrap().contains("sessionkit compact cut"));
        assert_eq!(plan[2]["toolResults"][0]["keep"], 0);
        assert!(plan[2]["toolResults"][0]["text"].as_str().unwrap().starts_with("line\n"));
        // u1: both messages are left out, since nothing else is in them.
        assert_eq!(plan[3], json!({"keep": 5}));
        assert_eq!(plan.len(), messages.len() - 2);
        assert!(chars < messages.iter().map(message_chars).sum::<usize>());
    }

    /// Two turns: one that read a file, and one that read a file and edited it.
    fn two_turns() -> Vec<Message> {
        let output = "line\n".repeat(200);
        let mut messages = vec![text("user", "read a.rs")];
        messages.extend(call("u0", &output));
        messages.push(text("assistant", "read it"));
        messages.push(text("user", "now fix it"));
        messages.extend(call("u1", &output));
        let mut edit = call("u2", &output);
        edit[0].uses[0].tool = "Edit".into();
        messages.extend(edit);
        messages.push(text("assistant", "fixed"));
        messages
    }

    #[test]
    fn the_fold_keeps_earlier_turns_and_cuts_what_the_last_one_only_looked_at() {
        let messages = two_turns();
        let folded = fold(&messages);
        assert_eq!(folded.plan.len(), messages.len());
        // Turn one stays as the engine has it, its long output included.
        for i in 0..5 {
            assert_eq!(folded.plan[i], json!({"keep": i}));
        }
        // Turn two: the output of the Read is cut in the message that holds the result.
        assert_eq!(folded.plan[6]["from"], 6);
        assert_eq!(folded.plan[6]["toolUses"], json!([]));
        assert!(folded.plan[6]["toolResults"][0]["text"].as_str().unwrap().contains("sessionkit compact cut 700 chars"));
        // No message of the assistant is rebuilt, and the Edit keeps its output.
        for i in [5, 7, 8, 9] {
            assert_eq!(folded.plan[i], json!({"keep": i}));
        }
        assert_eq!((folded.outputs, folded.chars), (1, 1000 - js::len(&cut(&"line\n".repeat(200), false))));
    }

    #[test]
    fn a_turn_without_long_outputs_is_not_folded() {
        let mut messages = two_turns();
        messages.push(text("user", "thanks"));
        messages.push(text("assistant", "welcome"));
        let folded = fold(&messages);
        assert_eq!(folded.outputs, 0);
        assert!(folded.plan.iter().enumerate().all(|(i, step)| *step == json!({"keep": i})));
    }

    #[test]
    fn the_state_shrinks_until_it_fits() {
        let mut messages = vec![text("user", "fix the test")];
        for n in 0..400 {
            messages.push(text("user", &"please look at this ".repeat(30)));
            messages.extend(call(&format!("u{n}"), "x"));
        }
        messages.extend((0..PRESERVE_RECENT_MESSAGES).map(|_| text("assistant", "done")));
        let calls = collect_calls(&messages);
        let fitted = fit_state(&messages, &calls).unwrap();
        assert!(fitted.tokens <= MAX_STATE_TOKENS);
        assert_ne!(fitted.stage, "full");
        let small = conversation(2);
        assert_eq!(fit_state(&small, &collect_calls(&small)).unwrap().stage, "full");
    }

    #[test]
    fn questions_are_split_into_requests_that_fit() {
        let messages = conversation(400);
        let calls = collect_calls(&messages);
        let candidates: Vec<&ToolCall> = calls.iter().collect();
        let groups = batches(&candidates, 25_000).unwrap();
        assert!(groups.len() > 1);
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 400);
    }
}
