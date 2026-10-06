// sessionkit ask — answer a question about a file without reading it into the agent's context.
//
// An agent often needs one fact about a file, not the file. Reading six hundred lines costs
// about twelve thousand tokens, and that text travels along with every later turn. Here the file
// goes to Jev (TypeSafe) and the agent gets a probability back. A port of the ask-jev plugin.
//
//   sessionkit ask <file> "question" ...       up to ten questions about one file
//   sessionkit ask --filter "question" <path>  one question about many files, highest first
//   sessionkit ask --find <file> "question"    which line answers it, and whether any line does
//
// Files with a sensitive name (.env, keys) or a private key in them stay on this machine unless
// --include-sensitive is given; folders given to --filter follow the rules of sessionkit grep.

use crate::Result;
use crate::grep::fs::{Policy, Reader, SnapshotResult};
use crate::js;
use crate::jev::{PRICE_PER_MILLION_TOKENS, model, post};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::atomic::AtomicBool;

const USAGE: &str = "Usage: sessionkit ask [options] <file> \"question\" [question options] [\"question\" ...]
       sessionkit ask --filter \"question\" [--yes text] [--no text] <file or folder> ...
       sessionkit ask --find <file> \"question\" [--max n]

Asks Jev (TypeSafe) about a file instead of reading it. The file is sent to TypeSafe; the
answer is a probability. Above 0.70 read it as yes, below 0.30 as no, in between read the file.
Under every answer that is not a clear no stands the line that answers it.

Question options, after the question they belong to:
  --options a,b,c    a choice: which one of these holds (\"none of these\" is added)
  --yes <text>       what counts as yes; put the boundary cases here
  --no <text>        what counts as no

Options:
  --filter           ask one yes/no question about every file; folders are searched with the
                     rules of sessionkit grep (no hidden, ignored or dependency files)
  --find             ask which line of the file answers the question
  --max <n>          lines to show with --find (default 5)
  --max-files <n>    stop --filter when a folder holds more files than this (default 200)
  --include-sensitive  also send files with a sensitive name or a private key
  -h, --help         show this help";

const NONE: &str = "none of these";
const MAX_QUESTIONS: usize = 10;
const MAX_CHARS: usize = 60000; // characters per part: one request, well inside Jev's limit
const OVERLAP_LINES: usize = 20; // a fact that straddles a boundary should land whole in one part
const MAX_PARTS: usize = 8; // a guard against paying for a very large file by accident
const WINDOW_LINES: usize = 200; // a Choice takes at most 255 options
const MAX_WINDOWS: usize = 12;
const LINE_CHARS: usize = 200;
const FOUND: f64 = 0.70;
const ABSENT: f64 = 0.30;
const CONCURRENCY: usize = 6;

static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Default)]
struct Question {
    text: String,
    yes: Option<String>,
    no: Option<String>,
    options: Vec<String>,
}

enum Mode {
    Ask,
    Filter,
    Find,
}

struct Options {
    mode: Mode,
    paths: Vec<String>,
    questions: Vec<Question>,
    max_lines: usize,
    max_files: usize,
    include_sensitive: bool,
}

fn fail(message: &str) -> ! {
    eprintln!("sessionkit ask: {message}\n\n{USAGE}");
    std::process::exit(2);
}

fn parse_arguments(argv: &[String]) -> Options {
    let mut options = Options { mode: Mode::Ask, paths: Vec::new(), questions: Vec::new(), max_lines: 5, max_files: 200, include_sensitive: false };
    let mut words: Vec<String> = Vec::new();
    // Question options attach to the word before them; that word is a question.
    let mut attached: Vec<(usize, Question)> = Vec::new();
    let mut i = 0;
    let value = |i: usize| argv.get(i).cloned().unwrap_or_else(|| fail(&format!("{} needs a value", argv[i - 1])));
    while i < argv.len() {
        let arg = argv[i].as_str();
        match arg {
            "--filter" => options.mode = Mode::Filter,
            "--find" => options.mode = Mode::Find,
            "--include-sensitive" => options.include_sensitive = true,
            "--max" | "--max-files" => {
                i += 1;
                let n = js::parse_number(Some(&value(i)));
                if !(n >= 1.0 && n.fract() == 0.0) {
                    fail(&format!("{arg} must be a positive whole number"));
                }
                if arg == "--max" { options.max_lines = (n as usize).min(20) } else { options.max_files = n as usize }
            }
            "--options" | "--yes" | "--no" => {
                i += 1;
                let text = value(i);
                let Some(index) = words.len().checked_sub(1) else { fail(&format!("{arg} belongs after a question")) };
                let entry = match attached.iter_mut().find(|(at, _)| *at == index) {
                    Some((_, question)) => question,
                    None => {
                        attached.push((index, Question::default()));
                        &mut attached.last_mut().unwrap().1
                    }
                };
                match arg {
                    "--options" => entry.options = text.split(',').map(|o| js::trim(o).to_string()).filter(|o| !o.is_empty()).collect(),
                    "--yes" => entry.yes = Some(text),
                    _ => entry.no = Some(text),
                }
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other if other.starts_with("--") => fail(&format!("unknown option {other}")),
            _ => words.push(argv[i].clone()),
        }
        i += 1;
    }
    let question_at = |index: usize, text: &str| {
        let mut question = attached.iter().find(|(at, _)| *at == index).map(|(_, q)| q.clone()).unwrap_or_default();
        question.text = text.to_string();
        question
    };
    let attached_to_path = |index: usize| attached.iter().any(|(at, _)| *at == index);
    match options.mode {
        Mode::Ask | Mode::Find => {
            let Some((path, rest)) = words.split_first() else { fail("give a file and a question") };
            if rest.is_empty() {
                fail("give at least one question");
            }
            if attached_to_path(0) {
                fail("question options belong after a question, not after the file");
            }
            options.paths = vec![path.clone()];
            options.questions = rest.iter().enumerate().map(|(i, text)| question_at(i + 1, text)).collect();
            if matches!(options.mode, Mode::Find) && (options.questions.len() > 1 || !attached.is_empty()) {
                fail("--find takes one plain question");
            }
            if options.questions.len() > MAX_QUESTIONS {
                fail(&format!("at most {MAX_QUESTIONS} questions per file"));
            }
        }
        Mode::Filter => {
            let Some((text, paths)) = words.split_first() else { fail("give a question and the files") };
            if paths.is_empty() {
                fail("give at least one file or folder");
            }
            if attached.iter().any(|(at, q)| *at != 0 || !q.options.is_empty()) {
                fail("--filter takes one yes/no question; --yes and --no go right after it");
            }
            options.questions = vec![question_at(0, text)];
            options.paths = paths.to_vec();
        }
    }
    options
}

// ---------------------------------------------------------------- reading

/// One explicitly named file, read with grep's rules for content: a text file, and no sensitive
/// name or private key unless asked. Ignore files do not apply, because the file was named.
fn read_file(path: &str, include_sensitive: bool) -> Result<String> {
    let full = Path::new(path);
    let parent = full.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = full.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| format!("{path}: not a file"))?;
    if !full.is_file() {
        return Err(format!("{path}: no such file"));
    }
    let policy = Policy { hidden: true, no_ignore: true, include_dependencies: true, include_sensitive };
    let reader = Reader::new(&parent.to_string_lossy(), policy, &[], &CANCELLED)?;
    match reader.read_snapshot(&name) {
        SnapshotResult::Ok(snapshot) => Ok(snapshot.source.clone()),
        SnapshotResult::Excluded => Err(format!("{path}: left out: not UTF-8 text, or a sensitive name or a private key (--include-sensitive sends those)")),
        SnapshotResult::Issue(kind) => Err(format!("{path}: {}", kind.name())),
    }
}

/// The files of a folder, as sessionkit grep would see them.
fn files_in(folder: &str, include_sensitive: bool, max_files: usize) -> Result<Vec<String>> {
    let policy = Policy { include_sensitive, ..Policy::default() };
    let reader = Reader::new(folder, policy, &[], &CANCELLED)?;
    let mut files = Vec::new();
    let mut folders = vec![String::new()];
    while let Some(directory) = folders.pop() {
        let mut cursor = None;
        loop {
            let page = reader.list_page(&directory, cursor);
            for entry in page.entries {
                if entry.directory {
                    folders.push(entry.path);
                } else {
                    files.push(Path::new(folder).join(&entry.path).to_string_lossy().into_owned());
                    if files.len() > max_files {
                        return Err(format!("{folder} holds more than {max_files} files; name a narrower folder, or raise --max-files"));
                    }
                }
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
    }
    files.sort();
    Ok(files)
}

struct Part {
    text: String,
    from: usize,
    to: usize,
}

/// Cut a file into parts on line boundaries, each within the request budget, overlapping a
/// little. Also returns how many lines lie beyond the last part.
fn parts(text: &str) -> (Vec<Part>, usize) {
    let lines: Vec<&str> = text.split('\n').collect();
    if js::len(text) <= MAX_CHARS {
        return (vec![Part { text: text.to_string(), from: 1, to: lines.len() }], 0);
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < lines.len() && out.len() < MAX_PARTS {
        let (mut end, mut size) = (start, 0);
        while end < lines.len() && size + js::len(lines[end]) + 1 <= MAX_CHARS {
            size += js::len(lines[end]) + 1;
            end += 1;
        }
        if end == start {
            end = start + 1; // one line longer than the budget
        }
        out.push(Part { text: lines[start..end].join("\n"), from: start + 1, to: end });
        if end >= lines.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP_LINES).max(start + 1);
    }
    let covered = out.last().map_or(0, |part| part.to);
    (out, lines.len() - covered)
}

// ---------------------------------------------------------------- asking

/// Options make a Choice, with a way out when none fits; anything else is a yes/no Noul.
fn to_question(question: &Question) -> Value {
    if !question.options.is_empty() {
        let mut criteria: Map<String, Value> = question.options.iter().map(|o| (o.clone(), Value::Null)).collect();
        criteria.insert(NONE.into(), "None of the other options holds for the file shown, or the file does not show it.".into());
        return json!({"type": "choice", "instructions": question.text, "criteria": criteria});
    }
    json!({"type": "noul", "instructions": question.text, "criteria": {
        "true": question.yes.clone().unwrap_or_else(|| "Yes, this holds for the file shown.".into()),
        "false": question.no.clone().unwrap_or_else(|| "No, it does not hold.".into()),
    }})
}

fn tokens_of(data: &Value) -> f64 {
    data.get("usage").and_then(|u| u.get("input_tokens")).and_then(Value::as_f64).unwrap_or(0.0)
}

/// How much a part says about a question: the yes of a Noul, the share off "none" of a Choice.
fn strength(answer: &Value) -> Option<f64> {
    if js::str_of(answer, "type") == "choice" {
        return Some(1.0 - answer.get("probabilities").and_then(|p| p.get(NONE)).and_then(Value::as_f64).unwrap_or(0.0));
    }
    answer.get("noul").and_then(Value::as_f64)
}

struct Best {
    value: Option<f64>,
    answer: Value,
    part: usize,
}

/// Ask every part all questions, and keep the strongest answer per question with its part.
fn ask_parts(path: &str, parts: &[Part], questions: &[Question], concurrency: usize) -> Result<(Vec<Best>, f64, String)> {
    let asked: Map<String, Value> = questions.iter().enumerate().map(|(i, q)| (format!("q{i}"), to_question(q))).collect();
    let model = model();
    let replies = js::pool(parts, concurrency, |part, _| {
        post(&json!({"model": model, "state": {"path": path, "content": part.text}, "questions": asked}))
    });
    let replies: Vec<Value> = replies.into_iter().collect::<Result<_>>()?;
    let tokens = replies.iter().map(tokens_of).sum();
    let used = replies.first().map(|data| js::str_of(data, "model").to_string()).filter(|m| !m.is_empty()).unwrap_or(model);
    let best = (0..questions.len()).map(|i| {
        let mut top = Best { value: None, answer: Value::Null, part: 0 };
        for (p, data) in replies.iter().enumerate() {
            let answer = &data["answers"][format!("q{i}")];
            if let Some(v) = strength(answer).filter(|v| top.value.is_none_or(|t| *v > t)) {
                top = Best { value: Some(v), answer: answer.clone(), part: p };
            }
        }
        top
    }).collect();
    Ok((best, tokens, used))
}

fn pct(value: Option<f64>) -> String {
    value.map_or("—".into(), |v| js::fixed(v, 2))
}

fn cost(tokens: f64) -> String {
    format!("${}", js::fixed(tokens / 1e6 * PRICE_PER_MILLION_TOKENS, 4))
}

fn place(parts: &[Part], part: usize) -> String {
    if parts.len() < 2 { String::new() } else { format!("  [lines {}-{}]", parts[part].from, parts[part].to) }
}

/// A yes/no answer is its probability; a choice is its likeliest option, the runners-up and the
/// confidence.
fn answer_line(question: &Question, best: &Best, parts: &[Part]) -> String {
    let at = place(parts, best.part);
    if js::str_of(&best.answer, "type") != "choice" {
        return format!("{}{at}  {}", pct(best.value), question.text);
    }
    let mut ranked: Vec<(String, f64)> = best.answer.get("probabilities").and_then(Value::as_object).into_iter().flatten()
        .map(|(o, p)| (o.clone(), p.as_f64().unwrap_or(0.0))).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let Some((option, probability)) = ranked.first().cloned() else { return format!("—{at}  {}", question.text) };
    let mut notes: Vec<String> = ranked[1..].iter().filter(|(_, p)| *p >= 0.05).map(|(o, p)| format!("{o} {}", js::fixed(*p, 2))).collect();
    notes.push(format!("confidence {}", pct(best.answer.get("confidence").and_then(Value::as_f64))));
    format!("{}  {option}  ({}){at}  {}", js::fixed(probability, 2), notes.join(" · "), question.text)
}

fn ask_file(options: &Options) -> Result<String> {
    let path = &options.paths[0];
    let text = read_file(path, options.include_sensitive)?;
    let (parts, short) = parts(&text);
    let (best, mut tokens, model) = ask_parts(path, &parts, &options.questions, CONCURRENCY)?;
    // Where the answer stands, for every question that is not a clear no: a probability alone
    // cannot answer "where" or "what", and a yes is only as good as the line it rests on.
    let open: Vec<usize> = (0..best.len()).filter(|&i| best[i].value.is_some_and(|v| v >= ABSENT)).collect();
    let (found, used) = evidence(path, &text, &open.iter().map(|&i| &options.questions[i]).collect::<Vec<_>>())?;
    tokens += used;
    let source: Vec<&str> = text.split('\n').collect();
    let mut lines = Vec::new();
    for (i, (question, best)) in options.questions.iter().zip(&best).enumerate() {
        lines.push(answer_line(question, best, &parts));
        let Some(k) = open.iter().position(|&o| o == i) else { continue };
        lines.push(match found[k] {
            Some((n, score)) if score >= ABSENT => format!("      line {n}  {}", js::head(js::trim(source.get(n - 1).unwrap_or(&"")), 160)),
            _ => "      no single line answers it".into(),
        });
    }
    let spread = if parts.len() > 1 { format!(" · {} parts", parts.len()) } else { String::new() };
    let missed = if short > 0 { format!(" · {short} lines beyond part {MAX_PARTS} not read") } else { String::new() };
    lines.push(String::new());
    lines.push(format!("{path} · {} chars{spread}{missed} · {} tokens · {} · {model}", js::len(&text), js::number(tokens), cost(tokens)));
    Ok(lines.join("\n"))
}

fn filter_files(options: &Options) -> Result<String> {
    let mut paths = Vec::new();
    for path in &options.paths {
        if Path::new(path).is_dir() {
            paths.extend(files_in(path, options.include_sensitive, options.max_files)?);
        } else {
            paths.push(path.clone());
        }
    }
    let question = &options.questions[0];
    let results = js::pool(&paths, CONCURRENCY, |path, _| -> Result<(Option<f64>, String, f64)> {
        let text = read_file(path, options.include_sensitive)?;
        let (parts, _) = parts(&text);
        let (best, tokens, _) = ask_parts(path, &parts, std::slice::from_ref(question), 1)?;
        Ok((best[0].value, place(&parts, best[0].part), tokens))
    });
    let mut ok: Vec<(&String, Option<f64>, String)> = Vec::new();
    let mut failed = Vec::new();
    let mut tokens = 0.0;
    for (path, result) in paths.iter().zip(results) {
        match result {
            Ok((value, at, used)) => {
                tokens += used;
                ok.push((path, value, at));
            }
            Err(error) => failed.push(error),
        }
    }
    ok.sort_by(|a, b| b.1.unwrap_or(0.0).total_cmp(&a.1.unwrap_or(0.0)));
    let mut lines = vec![question.text.clone(), String::new()];
    lines.extend(ok.iter().map(|(path, value, at)| format!("{}  {path}{at}", pct(*value))));
    if !failed.is_empty() {
        lines.push(String::new());
        lines.push("not asked:".into());
        lines.extend(failed.iter().map(|error| format!("  {error}")));
    }
    lines.push(String::new());
    lines.push(format!("{} files · {} tokens · {} · {}", ok.len(), js::number(tokens), cost(tokens), model()));
    Ok(lines.join("\n"))
}

struct Window {
    lines: Vec<(usize, String)>,
}

/// Windows of numbered, non-empty lines, and how many non-empty lines lie beyond the last one.
fn windows(text: &str) -> (Vec<Window>, usize, usize) {
    let numbered: Vec<(usize, String)> = text.split('\n').enumerate()
        .filter(|(_, line)| !js::trim(line).is_empty())
        .map(|(i, line)| (i + 1, js::head(js::trim(line), LINE_CHARS).to_string())).collect();
    let out: Vec<Window> = numbered.chunks(WINDOW_LINES).take(MAX_WINDOWS).map(|chunk| Window { lines: chunk.to_vec() }).collect();
    let seen: usize = out.iter().map(|w| w.lines.len()).sum();
    (out, numbered.len() - seen, numbered.len())
}

/// Which line of a window answers the question, and whether any line does. The lines go in the
/// state once, so both questions read them; the choice options are bare line numbers.
fn ask_window(path: &str, question: &str, window: &Window) -> Result<(f64, Vec<(usize, f64)>, f64)> {
    let lines: Map<String, Value> = window.lines.iter().map(|(n, text)| (n.to_string(), Value::from(text.as_str()))).collect();
    let criteria: Map<String, Value> = lines.keys().map(|n| (n.clone(), Value::Null)).collect();
    let data = post(&json!({
        "model": model(),
        "state": {"path": path, "lines": lines},
        "questions": {
            "line": {"type": "choice", "instructions": format!("{question} Answer with the line id."), "criteria": criteria},
            "exists": {"type": "noul", "instructions": format!("Do the lines shown answer this at all: {question}"), "criteria": {
                "true": "One of the lines shown answers it.",
                "false": "None of them do; the answer is elsewhere or absent.",
            }},
        },
    }))?;
    let exists = data["answers"]["exists"]["noul"].as_f64().unwrap_or(0.0);
    let probabilities = data["answers"]["line"]["probabilities"].as_object().into_iter().flatten()
        .filter_map(|(n, p)| Some((n.parse().ok()?, p.as_f64()?))).collect();
    Ok((exists, probabilities, tokens_of(&data)))
}

/// Per question, the line that answers it and how sure that is: per window one request with, per
/// question, a choice of line and whether the window answers it at all. The best line is the one
/// with the highest probability times that verdict, as in --find.
fn evidence(path: &str, text: &str, questions: &[&Question]) -> Result<(Vec<Option<(usize, f64)>>, f64)> {
    if questions.is_empty() {
        return Ok((Vec::new(), 0.0));
    }
    let (windows, _, _) = windows(text);
    let model = model();
    let replies = js::pool(&windows, CONCURRENCY, |window, _| {
        let lines: Map<String, Value> = window.lines.iter().map(|(n, text)| (n.to_string(), Value::from(text.as_str()))).collect();
        let criteria: Map<String, Value> = lines.keys().map(|n| (n.clone(), Value::Null)).collect();
        let mut asked = Map::new();
        for (i, question) in questions.iter().enumerate() {
            asked.insert(format!("line{i}"), json!({"type": "choice", "instructions": format!("{} Answer with the id of the line that answers it.", question.text), "criteria": criteria}));
            asked.insert(format!("exists{i}"), json!({"type": "noul", "instructions": format!("Do the lines shown answer this at all: {}", question.text), "criteria": {
                "true": "One of the lines shown answers it.",
                "false": "None of them do; the answer is elsewhere or absent.",
            }}));
        }
        post(&json!({"model": model, "state": {"path": path, "lines": lines}, "questions": asked}))
    });
    let replies: Vec<Value> = replies.into_iter().collect::<Result<_>>()?;
    let tokens = replies.iter().map(tokens_of).sum();
    let found = (0..questions.len()).map(|i| {
        let mut top: Option<(usize, f64)> = None;
        for data in &replies {
            let exists = data["answers"][format!("exists{i}")]["noul"].as_f64().unwrap_or(0.0);
            for (n, p) in data["answers"][format!("line{i}")]["probabilities"].as_object().into_iter().flatten() {
                let (Ok(n), Some(p)) = (n.parse::<usize>(), p.as_f64()) else { continue };
                if top.is_none_or(|(_, best)| p * exists > best) {
                    top = Some((n, p * exists));
                }
            }
        }
        top
    }).collect();
    Ok((found, tokens))
}

fn find_in_file(options: &Options) -> Result<String> {
    let path = &options.paths[0];
    let question = &options.questions[0].text;
    let text = read_file(path, options.include_sensitive)?;
    let (windows, short, total) = windows(&text);
    if windows.is_empty() {
        return Ok(format!("{path} is empty."));
    }
    let answers: Vec<(f64, Vec<(usize, f64)>, f64)> =
        js::pool(&windows, CONCURRENCY, |w, _| ask_window(path, question, w)).into_iter().collect::<Result<_>>()?;
    let mut found: Vec<(usize, f64)> = answers.iter().flat_map(|(exists, lines, _)| lines.iter().map(move |(n, p)| (*n, p * exists))).collect();
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found.truncate(options.max_lines);
    let tokens: f64 = answers.iter().map(|a| a.2).sum();
    let best = answers.iter().map(|a| a.0).fold(0.0, f64::max);
    let source: Vec<&str> = text.split('\n').collect();
    let mut out = vec![question.clone(), String::new()];
    if best < ABSENT {
        out.extend([format!("Not in this file ({}). The lines below are only the closest.", js::fixed(best, 2)), String::new()]);
    } else if best < FOUND {
        out.extend([format!("Partly answered ({}). Read the lines below before you rely on them.", js::fixed(best, 2)), String::new()]);
    }
    for (n, score) in &found {
        out.push(format!("{}  line {n}  {}", js::fixed(*score, 2), js::head(js::trim(source.get(n - 1).unwrap_or(&"")), 160)));
    }
    let missed = if short > 0 { format!(" · {short} lines beyond window {MAX_WINDOWS} not read") } else { String::new() };
    out.push(String::new());
    out.push(format!("{path} · {total} non-empty lines · {} windows{missed} · answer present {} · {} tokens · {} · {}",
        windows.len(), js::fixed(best, 2), js::number(tokens), cost(tokens), model()));
    Ok(out.join("\n"))
}

pub fn ask_main(argv: &[String]) -> Result<()> {
    let options = parse_arguments(argv);
    let output = match options.mode {
        Mode::Ask => ask_file(&options),
        Mode::Filter => filter_files(&options),
        Mode::Find => find_in_file(&options),
    }?;
    println!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn question_options_attach_to_the_question_before_them() {
        let options = parse_arguments(&args(&["a.ts", "Which runner?", "--options", "vitest, jest", "Does it touch a database?", "--yes", "SQL runs", "--no", "mocks"]));
        assert_eq!(options.paths, ["a.ts"]);
        assert_eq!(options.questions.len(), 2);
        assert_eq!(options.questions[0].options, ["vitest", "jest"]);
        assert_eq!(options.questions[1].yes.as_deref(), Some("SQL runs"));
        assert_eq!(options.questions[1].no.as_deref(), Some("mocks"));
        let choice = to_question(&options.questions[0]);
        assert_eq!(choice["type"], "choice");
        assert!(choice["criteria"].get(NONE).is_some());
        assert_eq!(to_question(&options.questions[1])["criteria"]["true"], "SQL runs");
    }

    #[test]
    fn filter_takes_the_question_first() {
        let options = parse_arguments(&args(&["--filter", "Does it call the network?", "--no", "mocks do not count", "src", "b.rs"]));
        assert_eq!(options.questions[0].text, "Does it call the network?");
        assert_eq!(options.questions[0].no.as_deref(), Some("mocks do not count"));
        assert_eq!(options.paths, ["src", "b.rs"]);
    }

    #[test]
    fn a_large_file_is_cut_with_overlap() {
        let line = "x".repeat(99);
        let text = vec![line.as_str(); 1500].join("\n");
        let (parts, short) = parts(&text);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].from, 1);
        assert_eq!(parts[1].from, parts[0].to - OVERLAP_LINES + 1);
        assert_eq!(parts.last().unwrap().to, 1500);
        assert_eq!(short, 0);
        assert!(parts.iter().all(|p| js::len(&p.text) <= MAX_CHARS));
    }

    #[test]
    fn a_choice_counts_what_is_not_none() {
        assert_eq!(strength(&json!({"type": "choice", "probabilities": {"a": 0.7, NONE: 0.3}})), Some(0.7));
        assert_eq!(strength(&json!({"type": "noul", "noul": 0.2})), Some(0.2));
    }

    #[test]
    fn windows_skip_empty_lines() {
        let (windows, short, total) = windows("a\n\n  \nb\n");
        assert_eq!(total, 2);
        assert_eq!(short, 0);
        assert_eq!(windows[0].lines, vec![(1, "a".to_string()), (4, "b".to_string())]);
    }
}
