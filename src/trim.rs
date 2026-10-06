// sessionkit trim --measure — would cutting a large tool result down to the task, before it
// enters the context, keep what the assistant needs?
//
// A tool result stays in the context for every later call. The idea: split a large Bash output
// or Read into blocks of lines, keep the first and last blocks and, for Bash, blocks with an
// error line, and let Jev (TypeSafe) judge the rest in one Choice. This runs that selection over
// large results in earlier transcripts and compares it with what the assistant did next: a block
// counts as used when a later Edit changes text from it, a later Read reads its lines again, or
// a later call or text names a code name that first appeared in it.
//
// Measured on 2026-09-30: a Noul per block ranked used blocks below unused ones; one Choice over
// the blocks reached an AUC of 0.61 inside a result. Too weak to cut live, so there is no hook.

use crate::Result;
use crate::compact::{Message, PRESERVE_RECENT_MESSAGES, TRUNCATE_HEAD_CHARS, ToolUse, goal, read_segments};
use crate::js;
use crate::jev::{PRICE_PER_MILLION_TOKENS, ask_choice};
use crate::start::{session_files};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::LazyLock;

const USAGE: &str = "Usage: sessionkit trim --measure [results] [--verbose]
       sessionkit trim --outline [reads]
       sessionkit trim --articulation [sessions] [--days n]

Measures whether cutting a large tool result down to the task would keep what the assistant
needs: Jev (TypeSafe) judges the blocks of lines of large Bash and Read results in earlier
transcripts, and the judgment is compared with what the assistant used afterwards.

  --measure   the number of results to judge (default 40; costs money)
  --declarations  for the plugin: print the outline lines of the text on stdin as JSON
  --articulation  of the facts an agent used after a possible compaction, how many it had
              written in its own text, which compaction keeps (local, free)
  --outline   what an outline of a large file first, and reading the used parts after, would
              cost against reading the whole file (local, free)
  --verbose   with --measure: a line per judged block
  -h, --help  show this help";

const MIN_BASH_CHARS: usize = 10_000;
const MIN_READ_CHARS: usize = 20_000;
const BLOCK_LINES: usize = 40;
const BLOCK_CHARS: usize = 4000;
const CHOICE_CHARS: usize = 60_000; // all options together, inside one request
const AFTER_MESSAGES: usize = 30;
const PER_SESSION: usize = 3;

static ERROR_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(error|errors|failed|failure|panic|panicked|exception|traceback|fatal)\b").unwrap());
static LINE_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(\d+)\t").unwrap());
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_./:-]*[A-Za-z0-9_]").unwrap());
// A name of code, a path or a key, not a word of prose: long enough, with a mark that words lack.
static CODE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_./:0-9]|[a-z][A-Z]").unwrap());

fn code_names(text: &str) -> HashSet<&str> {
    NAME.find_iter(text).map(|m| m.as_str()).filter(|name| name.len() >= 8 && CODE_NAME.is_match(name)).collect()
}

pub struct Block {
    pub first: usize, // line in the result, from 1
    pub last: usize,
    pub text: String,
    pub always: bool,
}

/// The result in blocks of at most BLOCK_LINES lines and about BLOCK_CHARS characters.
pub fn blocks_of(text: &str, tool: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut blocks = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start;
        let mut chars = 0;
        while end < lines.len() && end - start < BLOCK_LINES && (end == start || chars + js::len(lines[end]) <= BLOCK_CHARS) {
            chars += js::len(lines[end]) + 1;
            end += 1;
        }
        let text = lines[start..end].join("\n");
        let always = tool == "Bash" && ERROR_LINE.is_match(&text);
        blocks.push(Block { first: start + 1, last: end, text, always });
        start = end;
    }
    let count = blocks.len();
    if let Some(first) = blocks.first_mut() {
        first.always = true;
    }
    if count > 1 {
        blocks[count - 1].always = true;
    }
    blocks
}

pub fn threshold_of(tool: &str) -> Option<usize> {
    match tool {
        "Bash" => Some(MIN_BASH_CHARS),
        "Read" => Some(MIN_READ_CHARS),
        _ => None,
    }
}

fn input_line(tool: &str, input: &Value) -> String {
    let text = match tool {
        "Bash" => js::str_of(input, "command").to_string(),
        "Read" => js::str_of(input, "file_path").to_string(),
        _ => js::stringify(input),
    };
    js::clip(&text, 500)
}

pub struct Context<'a> {
    pub tool: &'a str,
    pub input: &'a Value,
    pub task: &'a str,
    pub before: &'a str,
    /// The last steps before the call: what was said and which tools ran, oldest first.
    pub recent: &'a [Value],
}

/// Jev's probability per block that the task is about it, from one Choice over the blocks, so
/// they add up to 1; None for a block that always stays. Measured: a Noul per block, asked whether
/// the assistant needs it or whether the task is about it, ranked used blocks below unused ones.
/// One Choice over all judged blocks: which part is the task about. Each block gets its probability.
fn judge_choice(state: &Value, blocks: &[Block], asked: &[usize]) -> Result<(Vec<f64>, f64)> {
    let per_block = (CHOICE_CHARS / asked.len().max(1)).clamp(200, 1500);
    let mut criteria = serde_json::Map::new();
    for &i in asked {
        criteria.insert(format!("lines_{}_{}", blocks[i].first, blocks[i].last), js::clip(&blocks[i].text, per_block).into());
    }
    let question = json!({
        "instructions": "The assistant ran `tool` with `input` while it worked on `task`. Its output comes in parts. \
            Which part shows the code, data, setting or message that the task is about?",
        "criteria": criteria,
    });
    let (answer, used) = ask_choice(state, question)?;
    let probabilities = answer.as_ref().and_then(|a| a.get("probabilities")).cloned().unwrap_or(Value::Null);
    Ok((asked.iter().map(|&i| probabilities.get(format!("lines_{}_{}", blocks[i].first, blocks[i].last)).and_then(Value::as_f64).unwrap_or(0.0)).collect(), used))
}

pub fn judge(context: &Context, blocks: &[Block]) -> Result<(Vec<Option<f64>>, f64)> {
    let asked: Vec<usize> = (0..blocks.len()).filter(|&i| !blocks[i].always).collect();
    let mut result = vec![None; blocks.len()];
    if asked.is_empty() {
        return Ok((result, 0.0));
    }
    let state = json!({
        "task": js::clip(context.task, 1500),
        "before": js::clip(context.before, 1500),
        "tool": context.tool,
        "input": input_line(context.tool, context.input),
        "recent_steps": context.recent,
    });
    let (probabilities, used) = judge_choice(&state, blocks, &asked)?;
    for (&i, p) in asked.iter().zip(probabilities) {
        result[i] = Some(p);
    }
    Ok((result, used))
}

/// Per block, the probability of the blocks ranked above it; a block stays while that is below the mass.
pub fn mass_before(fits: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut order: Vec<usize> = (0..fits.len()).filter(|&i| fits[i].is_some()).collect();
    order.sort_by(|&a, &b| fits[b].unwrap().total_cmp(&fits[a].unwrap()));
    let mut result = vec![None; fits.len()];
    let mut sum = 0.0;
    for i in order {
        result[i] = Some(sum);
        sum += fits[i].unwrap();
    }
    result
}

/// The first line number a Read shows in a block, as its own numbering (cat -n) gives it.
fn file_line(block: &Block) -> Option<usize> {
    block.text.split('\n').find_map(|line| LINE_NUMBER.captures(line)?.get(1)?.as_str().parse().ok())
}

// ---------------------------------------------------------------- measuring

#[derive(Clone, Copy, PartialEq, Debug)]
enum Use {
    Edited,
    Reread,
    Named,
    Unused,
}

fn without_numbers(text: &str) -> String {
    text.split('\n').map(|line| LINE_NUMBER.replace(line, "").into_owned()).collect::<Vec<_>>().join("\n")
}

/// The lines of text a later Edit replaced, long enough to point at one place.
fn edited_lines(message: &Message) -> Vec<String> {
    let mut lines = Vec::new();
    for tool in message.uses.iter().filter(|u| ["Edit", "MultiEdit"].contains(&u.tool.as_str())) {
        let edits = tool.input.get("edits").and_then(Value::as_array).cloned().unwrap_or_else(|| vec![tool.input.clone()]);
        for edit in edits {
            lines.extend(js::str_of(&edit, "old_string").split('\n').map(js::trim).filter(|l| js::len(l) >= 20).map(str::to_string));
        }
    }
    lines
}

/// What the assistant did with each judged block in the messages after the result.
fn uses(tool: &str, input: &Value, blocks: &[Block], before: &str, after: &[Message]) -> Vec<Use> {
    let file = js::str_of(input, "file_path");
    let edits: Vec<String> = after.iter().flat_map(edited_lines).collect();
    let rereads: Vec<(usize, usize)> = after.iter().flat_map(|m| &m.uses)
        .filter(|u| tool == "Read" && u.tool == "Read" && js::str_of(&u.input, "file_path") == file)
        .filter_map(|u| {
            let offset = u.input.get("offset").and_then(Value::as_u64)? as usize;
            Some((offset, offset + u.input.get("limit").and_then(Value::as_u64).unwrap_or(2000) as usize))
        }).collect();
    let later: String = after.iter().flat_map(|m| std::iter::once(m.text.clone()).chain(m.uses.iter().map(|u| js::stringify(&u.input))))
        .collect::<Vec<_>>().join("\n");
    // A name counts for one block only: one that appears in several says nothing about which.
    let names: Vec<HashSet<&str>> = blocks.iter().map(|b| code_names(&b.text)).collect();
    let mut seen_in: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for set in &names {
        for name in set {
            *seen_in.entry(name).or_default() += 1;
        }
    }
    blocks.iter().zip(&names).map(|(block, names)| {
        let plain = without_numbers(&block.text);
        if edits.iter().any(|line| plain.contains(line.as_str())) {
            return Use::Edited;
        }
        if let (Some(from), Some(to)) = (file_line(block), block.text.split('\n').rev().find_map(|l| LINE_NUMBER.captures(l)?.get(1)?.as_str().parse::<usize>().ok())) {
            if rereads.iter().any(|&(start, end)| start <= to && from < end) {
                return Use::Reread;
            }
        }
        let named = names.iter().any(|name| seen_in.get(name) == Some(&1) && !before.contains(name) && later.contains(name));
        if named { Use::Named } else { Use::Unused }
    }).collect()
}

struct Sample {
    tool: String,
    input: Value,
    task: String,
    before: String,
    recent: Vec<Value>,
    blocks: Vec<Block>,
    uses: Vec<Use>,
    /// Assistant messages after the result in its stretch: about the calls that read it again.
    later: usize,
}

const RECENT_STEPS: usize = 12;

/// The last steps before message `at`: texts and tool calls, without their results.
pub fn recent_steps(messages: &[Message], at: usize) -> Vec<Value> {
    let mut steps: Vec<Value> = messages[..at].iter().rev().flat_map(|m| {
        let mut parts: Vec<Value> = m.uses.iter().rev().map(|u| json!({"tool": u.tool, "input": js::clip(&input_line(&u.tool, &u.input), 300)})).collect();
        if !js::trim(&m.text).is_empty() && m.results.is_empty() {
            parts.push(json!({m.role.clone(): js::clip(&m.text, 600)}));
        }
        parts
    }).take(RECENT_STEPS).collect();
    steps.reverse();
    steps
}

/// A large Bash or Read result, for the Jev measurement.
fn large(call: &ToolUse, text: &str) -> bool {
    threshold_of(&call.tool).is_some_and(|min| js::len(text) >= min)
}

/// Large results in a stretch of conversation that `select` takes, with what came after them.
fn samples_of(messages: &[Message], per_session: usize, select: fn(&ToolUse, &str) -> bool) -> Vec<Sample> {
    let mut samples = Vec::new();
    for (i, message) in messages.iter().enumerate() {
        for result in &message.results {
            if samples.len() >= per_session || result.is_error {
                continue;
            }
            let Some(call) = messages[..i].iter().rev().flat_map(|m| &m.uses).find(|u| u.id == result.id) else { continue };
            if !select(call, &result.text) || i + 3 >= messages.len() {
                continue;
            }
            // What the assistant knew before: a name from there says nothing about this result.
            let mut before_text: String = messages[..i].iter()
                .flat_map(|m| std::iter::once(m.text.as_str()).chain(m.results.iter().map(|r| r.text.as_str())))
                .collect::<Vec<_>>().join("\n");
            before_text.push_str(&js::stringify(&call.input));
            let said = messages[..i].iter().rev().find(|m| m.role == "assistant" && !js::trim(&m.text).is_empty()).map_or("", |m| &m.text);
            let blocks = blocks_of(&result.text, &call.tool);
            let after = &messages[i + 1..(i + 1 + AFTER_MESSAGES).min(messages.len())];
            let uses = uses(&call.tool, &call.input, &blocks, &before_text, after);
            let later = messages[i + 1..].iter().filter(|m| m.role == "assistant").count();
            samples.push(Sample { tool: call.tool.clone(), input: call.input.clone(), task: goal(&messages[..i]), before: said.to_string(),
                recent: recent_steps(messages, i), blocks, uses, later });
        }
    }
    samples
}

fn auc(used: &[f64], unused: &[f64]) -> Option<f64> {
    if used.is_empty() || unused.is_empty() {
        return None;
    }
    let wins: f64 = used.iter().map(|u| unused.iter().map(|n| if u > n { 1.0 } else if u == n { 0.5 } else { 0.0 }).sum::<f64>()).sum();
    Some(wins / (used.len() * unused.len()) as f64)
}

fn measure(max: usize, verbose: bool) -> Result<()> {
    let paths = session_files(true, usize::MAX)?;
    let found = js::pool(&paths, 8, |path, _| {
        read_segments(path).unwrap_or_default().iter().flat_map(|segment| samples_of(segment, PER_SESSION, large)).take(PER_SESSION).collect::<Vec<_>>()
    });
    // Spread over sessions: newest first, a few per session.
    let samples: Vec<Sample> = found.into_iter().flatten().filter(|s| s.uses.iter().zip(&s.blocks).any(|(u, b)| !b.always && *u != Use::Unused)).take(max).collect();
    if samples.is_empty() {
        return Err("No large Bash or Read results with a used block found.".into());
    }
    let judged = js::pool(&samples, 4, |sample, _| {
        judge(&Context { tool: &sample.tool, input: &sample.input, task: &sample.task, before: &sample.before, recent: &sample.recent }, &sample.blocks)
    });
    let (mut used, mut unused, mut tokens) = (Vec::new(), Vec::new(), 0.0);
    let mut within = Vec::new(); // the AUC inside each result that has both kinds of block
    let (mut total_chars, mut judged_chars) = (0usize, Vec::new());
    for (sample, result) in samples.iter().zip(judged) {
        let (fits, spent) = result?;
        tokens += spent;
        let before = mass_before(&fits);
        let judged: Vec<(f64, bool)> = fits.iter().zip(&sample.uses).filter_map(|(p, how)| Some(((*p)?, *how != Use::Unused))).collect();
        let (yes, no): (Vec<&(f64, bool)>, Vec<&(f64, bool)>) = judged.iter().partition(|b| b.1);
        if let Some(a) = auc(&yes.iter().map(|b| b.0).collect::<Vec<_>>(), &no.iter().map(|b| b.0).collect::<Vec<_>>()) {
            within.push(a);
        }
        for (((block, fit), how), above) in sample.blocks.iter().zip(&fits).zip(&sample.uses).zip(&before) {
            total_chars += js::len(&block.text) + 1;
            let (Some(p), Some(above)) = (fit, above) else { continue };
            judged_chars.push((*above, js::len(&block.text) + 1, *how));
            if *how == Use::Unused { unused.push(-above) } else { used.push(-above) }
            if verbose {
                eprintln!("  {} {}  {:<7} {:<5} lines {}-{}  {}", js::fixed(*p, 2), js::fixed(*above, 2), format!("{how:?}"), sample.tool, block.first, block.last,
                    js::clip(&js::collapse_space(&input_line(&sample.tool, &sample.input)), 60));
            }
        }
    }
    let count = |how: Use| judged_chars.iter().filter(|b| b.2 == how).count();
    println!("{} large results ({} Bash, {} Read), {} blocks judged: {} edited, {} read again, {} named, {} unused",
        samples.len(), samples.iter().filter(|s| s.tool == "Bash").count(), samples.iter().filter(|s| s.tool == "Read").count(),
        judged_chars.len(), count(Use::Edited), count(Use::Reread), count(Use::Named), count(Use::Unused));
    // A lower mass above a block ranks it higher; negate it so that higher means likelier kept.
    let of = |how: Use| judged_chars.iter().filter(|b| b.2 == how).map(|b| -b.0).collect::<Vec<_>>();
    let show = |value: Option<f64>| value.map_or("—".into(), |a| js::fixed(a, 2));
    println!("AUC inside a result, averaged over {}: {}", within.len(), show((!within.is_empty()).then(|| within.iter().sum::<f64>() / within.len() as f64)));
    println!("AUC used against unused: {} (edited {}, read again {}, named {})", show(auc(&used, &unused)),
        show(auc(&of(Use::Edited), &unused)), show(auc(&of(Use::Reread), &unused)), show(auc(&of(Use::Named), &unused)));
    println!();
    println!("   mass  used kept  unused cut  output cut");
    for mass in [0.5, 0.7, 0.8, 0.9, 0.95, 0.98] {
        let kept_used = used.iter().filter(|p| -**p < mass).count();
        let cut_unused = unused.iter().filter(|p| -**p >= mass).count();
        let cut_chars: usize = judged_chars.iter().filter(|b| b.0 >= mass).map(|b| b.1).sum();
        println!("{:>7}  {:>9}  {:>10}  {:>10}", js::fixed(mass, 2),
            format!("{}%", js::round(100.0 * kept_used as f64 / used.len().max(1) as f64)),
            format!("{}%", js::round(100.0 * cut_unused as f64 / unused.len().max(1) as f64)),
            format!("{}%", js::round(100.0 * cut_chars as f64 / total_chars.max(1) as f64)));
    }
    println!();
    println!("mass: blocks stay from the likeliest down until their probabilities add up to this.");
    println!("used kept: blocks the assistant used afterwards that stay. output cut: of all characters of these results.");
    println!("Jev: ${}", js::fixed(tokens / 1e6 * PRICE_PER_MILLION_TOKENS, 4));
    Ok(())
}

// ---------------------------------------------------------------- an outline first

const OUTLINE_MIN_LINES: usize = 400;
const OUTLINE_HEAD_LINES: usize = 20;
const OUTLINE_LINE_CHARS: usize = 120;

static CAT_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^(?:cd \S+ *(?:&&|;) *)*cat ("[^"]+"|'[^']+'|\S+)\s*$"#).unwrap());
static DECLARATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(concat!(
    r"^\s{0,4}(?:(?:pub(?:\([^)]*\))?|export|default|async|abstract|final|static|public|private|protected|readonly|unsafe)\s+)*",
    r"(?:fn|struct|enum|trait|impl|mod|type|interface|class|function|def|const|macro_rules!)\b|^#{1,6}\s")).unwrap());

/// A whole file read at once: a Read without offset or limit, or a Bash `cat` of one file.
fn whole_file(call: &ToolUse, text: &str) -> bool {
    let whole = match call.tool.as_str() {
        "Read" => call.input.get("offset").is_none() && call.input.get("limit").is_none(),
        "Bash" => CAT_FILE.is_match(js::trim(js::str_of(&call.input, "command"))),
        _ => false,
    };
    whole && text.split('\n').count() >= OUTLINE_MIN_LINES
}

/// Every line after the first OUTLINE_HEAD_LINES that declares something or is a heading, with its
/// line number: from Read's own numbering when the text has it, else counted from 1.
pub fn declarations(text: &str) -> Vec<String> {
    text.split('\n').enumerate().skip(OUTLINE_HEAD_LINES).filter_map(|(i, line)| {
        let (number, plain) = match LINE_NUMBER.captures(line) {
            Some(c) => (c[1].to_string(), &line[c.get(0).unwrap().end()..]),
            None => ((i + 1).to_string(), line),
        };
        DECLARATION.is_match(plain).then(|| format!("{number}: {}", js::clip(js::trim_end(plain), OUTLINE_LINE_CHARS)))
    }).collect()
}

/// What the agent would get instead of the file: its first lines, then its declarations.
pub fn outline_of(text: &str) -> String {
    let mut out: Vec<String> = text.split('\n').take(OUTLINE_HEAD_LINES).map(str::to_string).collect();
    out.extend(declarations(text));
    out.join("\n")
}

/// sessionkit trim --declarations: for the plugin's read hook, the file's text on stdin.
fn print_declarations() -> Result<()> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| e.to_string())?;
    println!("{}", js::stringify(&json!({"head": OUTLINE_HEAD_LINES, "declarations": declarations(&text)})));
    Ok(())
}

/// sessionkit trim --outline: what an outline first, and reading the used blocks after, would
/// have cost against the whole file. Local and free.
fn measure_outline(max: usize) -> Result<()> {
    let paths = session_files(true, usize::MAX)?;
    let found = js::pool(&paths, 8, |path, _| {
        read_segments(path).unwrap_or_default().iter().flat_map(|segment| samples_of(segment, usize::MAX, whole_file)).collect::<Vec<_>>()
    });
    let samples: Vec<Sample> = found.into_iter().flatten().take(max).collect();
    if samples.is_empty() {
        return Err("No whole reads of large files found.".into());
    }
    let (mut full, mut instead, mut full_weighted, mut instead_weighted) = (0.0, 0.0, 0.0, 0.0);
    let (mut reads, mut untouched, mut outline_share) = (Vec::new(), 0, Vec::new());
    for sample in &samples {
        let text: String = sample.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
        let outline = js::len(&outline_of(&text)) as f64;
        // Every used block is read again, one Read per run of neighbouring used blocks.
        let used: Vec<bool> = sample.uses.iter().map(|u| *u != Use::Unused).collect();
        let needed: f64 = sample.blocks.iter().zip(&used).filter(|(_, u)| **u).map(|(b, _)| js::len(&b.text) as f64 + 1.0).sum();
        let runs = used.iter().enumerate().filter(|(i, u)| **u && (*i == 0 || !used[i - 1])).count();
        let size = js::len(&text) as f64;
        full += size;
        instead += outline + needed;
        full_weighted += size * sample.later as f64;
        instead_weighted += (outline + needed) * sample.later as f64;
        reads.push(runs);
        untouched += usize::from(runs == 0);
        outline_share.push(outline / size.max(1.0));
    }
    reads.sort_unstable();
    outline_share.sort_by(f64::total_cmp);
    let n = samples.len();
    let percent = |x: f64| format!("{}%", js::round(100.0 * x));
    println!("{n} whole reads of files of {OUTLINE_MIN_LINES} lines or more ({} Read, {} cat)",
        samples.iter().filter(|s| s.tool == "Read").count(), samples.iter().filter(|s| s.tool == "Bash").count());
    println!("outline: median {} of the file", percent(outline_share[n / 2]));
    println!("outline plus the used blocks: {} of the characters, {} weighted by the later calls that read them",
        percent(instead / full), percent(instead_weighted / full_weighted));
    println!("extra reads per file: median {}, mean {}; {} of the files needed none",
        reads[n / 2], js::fixed(reads.iter().sum::<usize>() as f64 / n as f64, 1), percent(untouched as f64 / n as f64));
    println!();
    println!("A block counts as used when a later Edit changes its text, a later Read reads its lines, or a later");
    println!("call or text names a code name first seen in it, within {AFTER_MESSAGES} messages. Use that leaves no such trace is");
    println!("missed, so the real cost of reading after the outline is higher than this.");
    Ok(())
}

// ---------------------------------------------------------------- articulation

const POINTS_PER_SESSION: usize = 3;
const CUT_SLACK: usize = 120; // compact::cut keeps a result this close to its head whole

static LINE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_.-]+\.[a-z]{1,5}:\d+").unwrap());

/// The facts of a text that an agent can need again: code names, and file:line references.
fn facts(text: &str) -> HashSet<String> {
    let mut found: HashSet<String> = code_names(text).into_iter().map(str::to_string).collect();
    found.extend(LINE_REF.find_iter(text).map(|m| m.as_str().to_string()));
    found
}

fn inputs_of(message: &Message) -> String {
    message.uses.iter().map(|u| js::stringify(&u.input)).collect::<Vec<_>>().join("\n")
}

fn is_prompt(message: &Message) -> bool {
    message.role == "user" && message.results.is_empty() && !js::trim(&message.text).is_empty() && !js::trim(&message.text).starts_with('<')
}

#[derive(Default)]
struct Articulation {
    sessions: usize,
    points: usize,
    results_used: usize,
    results_unused: usize,
    facts_used: usize,
    facts_said: usize,
    facts_in_input: usize,
    results_none_said: usize,
    by_tool: std::collections::BTreeMap<String, (usize, usize)>,
    tool_calls: usize,
    notes: usize,
}

/// One compaction point: of the facts in results before `cut` that compaction may cut and that
/// the agent used after it, which had it written in its own text, or in a tool input, before it?
/// A fact counts as used when the agent names it after the point before any later result brings
/// it back: then it can only have come from the old result, or from its own text.
fn articulation_at(messages: &[Message], cut: usize, stats: &mut Articulation) {
    let tool_of: std::collections::HashMap<&str, &str> = messages.iter().flat_map(|m| &m.uses).map(|u| (u.id.as_str(), u.tool.as_str())).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut candidates: Vec<(usize, String, HashSet<String>)> = Vec::new();
    for (i, message) in messages[..cut].iter().enumerate() {
        for result in &message.results {
            let pinned = i == 0 || i + PRESERVE_RECENT_MESSAGES >= cut;
            if js::len(&result.text) > TRUNCATE_HEAD_CHARS + CUT_SLACK && !pinned {
                let head = js::head(&result.text, TRUNCATE_HEAD_CHARS);
                let kept = facts(head);
                let new: HashSet<String> = facts(&result.text[head.len()..]).into_iter().filter(|f| !kept.contains(f) && !seen.contains(f)).collect();
                candidates.push((i, tool_of.get(result.id.as_str()).unwrap_or(&"?").to_string(), new));
            }
            seen.extend(facts(&result.text));
        }
        seen.extend(facts(&message.text));
        seen.extend(facts(&inputs_of(message)));
    }
    let (mut first_use, mut first_back) = (std::collections::HashMap::new(), std::collections::HashMap::new());
    for (j, message) in messages[cut..].iter().enumerate() {
        let own = if message.role == "assistant" { message.text.as_str() } else { "" };
        for fact in facts(&format!("{own}\n{}", inputs_of(message))) {
            first_use.entry(fact).or_insert(j);
        }
        for result in &message.results {
            for fact in facts(&result.text) {
                first_back.entry(fact).or_insert(j);
            }
        }
    }
    for (i, tool, new) in candidates {
        let used: Vec<&String> = new.iter().filter(|f| first_use.get(*f).is_some_and(|u| u <= first_back.get(*f).unwrap_or(&usize::MAX))).collect();
        if used.is_empty() {
            stats.results_unused += 1;
            continue;
        }
        let between = &messages[i + 1..cut];
        let own: String = between.iter().filter(|m| m.role == "assistant").map(|m| m.text.as_str()).collect::<Vec<_>>().join("\n");
        let inputs: String = between.iter().map(inputs_of).collect::<Vec<_>>().join("\n");
        let said = used.iter().filter(|f| own.contains(f.as_str())).count();
        let in_input = used.iter().filter(|f| !own.contains(f.as_str()) && inputs.contains(f.as_str())).count();
        stats.results_used += 1;
        stats.facts_used += used.len();
        stats.facts_said += said;
        stats.facts_in_input += in_input;
        stats.results_none_said += usize::from(said == 0);
        let entry = stats.by_tool.entry(tool).or_default();
        entry.0 += used.len();
        entry.1 += said;
    }
}

/// sessionkit trim --articulation: how much of what agents learn from tool output they write
/// down in their own text, which a compaction without summary keeps. Local and free.
fn measure_articulation(max: usize, days: Option<f64>) -> Result<()> {
    let since = days.map_or(0.0, |d| js::now() - d * 86_400_000.0);
    let paths: Vec<_> = session_files(true, usize::MAX)?.into_iter().filter(|p| js::mtime_ms(p).unwrap_or(0.0) >= since).take(max).collect();
    let found = js::pool(&paths, 8, |path, _| {
        let mut stats = Articulation::default();
        for messages in read_segments(path).unwrap_or_default() {
            stats.tool_calls += messages.iter().map(|m| m.uses.len()).sum::<usize>();
            stats.notes += messages.iter().filter(|m| m.role == "assistant").map(|m| m.text.matches("Noted:").count()).sum::<usize>();
            let prompts: Vec<usize> = (0..messages.len()).filter(|&i| is_prompt(&messages[i]) && i * 5 >= messages.len() && i * 10 <= messages.len() * 9).collect();
            let step = (prompts.len() / POINTS_PER_SESSION).max(1);
            for &cut in prompts.iter().step_by(step).take(POINTS_PER_SESSION) {
                articulation_at(&messages, cut, &mut stats);
                stats.points += 1;
            }
        }
        stats
    });
    let mut total = Articulation::default();
    for stats in found {
        total.sessions += usize::from(stats.points > 0);
        total.points += stats.points;
        total.results_used += stats.results_used;
        total.results_unused += stats.results_unused;
        total.facts_used += stats.facts_used;
        total.facts_said += stats.facts_said;
        total.facts_in_input += stats.facts_in_input;
        total.results_none_said += stats.results_none_said;
        total.tool_calls += stats.tool_calls;
        total.notes += stats.notes;
        for (tool, (used, said)) in stats.by_tool {
            let entry = total.by_tool.entry(tool).or_default();
            entry.0 += used;
            entry.1 += said;
        }
    }
    if total.facts_used == 0 {
        return Err("No compaction points with a used fact found for these options.".into());
    }
    let share = |part: usize, whole: usize| format!("{}%", js::fixed(100.0 * part as f64 / whole.max(1) as f64, 1));
    println!("{} sessions{}, {} compaction points; {} results compaction may cut, {} with a fact used after the point",
        total.sessions, days.map_or(String::new(), |d| format!(" of the last {} days", js::number(d))), total.points,
        total.results_used + total.results_unused, total.results_used);
    println!("facts used after the point: {}", total.facts_used);
    println!("  in the agent's own text before it (articulation): {}", share(total.facts_said, total.facts_used));
    println!("  only in a tool input, which compaction keeps:       {}", share(total.facts_in_input, total.facts_used));
    println!("  only in tool output, which compaction may cut:      {}",
        share(total.facts_used - total.facts_said - total.facts_in_input, total.facts_used));
    println!("results whose used facts the agent wrote none of: {}", share(total.results_none_said, total.results_used));
    let mut tools: Vec<_> = total.by_tool.iter().collect();
    tools.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (tool, (used, said)) in tools.into_iter().take(5) {
        println!("  {tool}: {} of {used}", share(*said, *used));
    }
    println!("Noted: lines: {} over {} tool calls ({} per call)", total.notes, total.tool_calls, js::fixed(total.notes as f64 / total.tool_calls.max(1) as f64, 2));
    Ok(())
}

pub fn trim_main(argv: &[String]) -> Result<()> {
    match argv.first().map(String::as_str) {
        Some("--declarations") => print_declarations(),
        Some("--articulation") => {
            let days = argv.iter().position(|a| a == "--days").and_then(|i| argv.get(i + 1)).and_then(|n| n.parse().ok());
            let max = argv.get(1).filter(|a| !a.starts_with("--")).and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
            measure_articulation(max, days)
        }
        Some("--outline") => measure_outline(argv.get(1).and_then(|n| n.parse().ok()).unwrap_or(usize::MAX)),
        Some("--measure") => {
            let max = argv.get(1).and_then(|n| n.parse().ok()).unwrap_or(40);
            measure(max, argv.iter().any(|a| a == "--verbose"))
        }
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(lines: usize) -> String {
        (1..=lines).map(|n| format!("{n:>6}\tline {n}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn the_first_and_last_blocks_and_error_blocks_always_stay() {
        let mut lines: Vec<String> = (1..=200).map(|n| format!("step {n} ok")).collect();
        lines[100] = "error: could not compile".into();
        let blocks = blocks_of(&lines.join("\n"), "Bash");
        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks.iter().map(|b| b.always).collect::<Vec<_>>(), [true, false, true, false, true]);
        assert_eq!((blocks[2].first, blocks[2].last), (81, 120));
    }

    #[test]
    fn an_outline_keeps_the_head_and_the_declarations_with_their_numbers() {
        let mut lines: Vec<String> = (1..=60).map(|n| format!("{n:>6}\t    let x{n} = {n};")).collect();
        lines[29] = format!("{:>6}\tpub fn build(options: &Options) -> Result<()> {{", 30);
        lines[44] = format!("{:>6}\t## Install", 45);
        let outline = outline_of(&lines.join("\n"));
        assert_eq!(outline.lines().count(), 22);
        assert!(outline.ends_with("30: pub fn build(options: &Options) -> Result<()> {\n45: ## Install"));
    }

    #[test]
    fn a_cat_of_one_file_is_a_whole_read() {
        assert!(CAT_FILE.is_match("cd /p && cat src/start.rs"));
        assert!(CAT_FILE.is_match("cat 'a b.md'"));
        assert!(!CAT_FILE.is_match("cat src/a.rs | head -50"));
        assert!(!CAT_FILE.is_match("cat a.rs b.rs"));
    }

    #[test]
    fn a_fact_in_the_agent_s_own_text_survives_the_cut() {
        let msg = |role: &str, text: &str| Message { role: role.into(), text: text.into(), uses: Vec::new(), results: Vec::new() };
        let tail = "x".repeat(400);
        let mut messages = vec![msg("user", "fix the upload")];
        let mut call = msg("assistant", "");
        call.uses.push(ToolUse { id: "t1".into(), tool: "Read".into(), input: json!({"file_path": "a.rs"}), text: None, is_error: false });
        messages.push(call);
        let mut result = msg("user", "");
        result.results.push(crate::compact::ToolResult { id: "t1".into(), text: format!("{tail} fn retry_upload() and MAX_RETRIES_TOTAL"), is_error: false });
        messages.push(result);
        messages.push(msg("assistant", "Noted: `retry_upload` retries three times."));
        messages.extend((0..PRESERVE_RECENT_MESSAGES).map(|_| msg("assistant", "working")));
        messages.push(msg("user", "go on"));
        let cut = messages.len() - 1;
        messages.push(msg("assistant", "I change retry_upload and MAX_RETRIES_TOTAL."));
        let mut stats = Articulation::default();
        articulation_at(&messages, cut, &mut stats);
        assert_eq!((stats.facts_used, stats.facts_said), (2, 1));
    }

    #[test]
    fn only_names_of_code_count_not_words() {
        let names = code_names("the existing controller calls ownerUserId in app/Http/Kernel.php with LIBXML_NONET, explicitly.");
        assert_eq!(names, HashSet::from(["ownerUserId", "app/Http/Kernel.php", "LIBXML_NONET"]));
    }

    #[test]
    fn blocks_stay_from_the_likeliest_down_until_the_mass() {
        let before = mass_before(&[None, Some(0.125), Some(0.5), Some(0.375)]);
        assert_eq!(before, [None, Some(0.875), Some(0.0), Some(0.5)]);
    }

    #[test]
    fn a_block_is_used_when_a_later_edit_changes_its_text() {
        let text = numbered(120);
        let blocks = blocks_of(&text, "Read");
        let edit = Message { role: "assistant".into(), text: String::new(), results: Vec::new(), uses: vec![crate::compact::ToolUse {
            id: "e".into(), tool: "Edit".into(), input: json!({"old_string": "line 57 is long enough"}), text: None, is_error: false }] };
        let blocks = blocks.into_iter().map(|b| if b.first == 41 { Block { text: b.text.replace("line 57", "line 57 is long enough"), ..b } } else { b }).collect::<Vec<_>>();
        let found = uses("Read", &json!({"file_path": "/p/a.rs"}), &blocks, "", &[edit]);
        assert_eq!(found, [Use::Unused, Use::Edited, Use::Unused]);
    }
}
