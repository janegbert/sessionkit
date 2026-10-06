// sessionkit usage — where the tokens and the money go in your Claude Code sessions.
//
// Reads the transcripts under ~/.claude/projects, main sessions and their subagents, and adds up
// the usage that every API call records: uncached input, cache reads, cache writes (5 minutes and
// 1 hour) and output. Everything stays on this machine; no model is asked anything.
//
// Costs are API-equivalent: token counts times the published per-token prices. With a
// subscription you pay a flat fee instead, so read them as the value of what you used.
//
// Claude Code's own "cost-state" per session is not used: it counts per process, so a resumed
// session starts again, and totals from it came out both too low and too high.

use crate::Result;
use crate::js::{self, num, opt};
use crate::pricing::{Rate, Ttl};
use crate::transcript::{ApiCall, Entry};
use indexmap::{IndexMap, IndexSet};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::BufRead;
use std::path::PathBuf;

const REBUILD_MIN_TOKENS: f64 = 20000.0; // a cache write this large after a pause rebuilds the context
const FRESH_CONTEXT_TOKENS: f64 = 15000.0; // about what sessionkit start hands a new session
const RESTART_MIN_CONTEXT: f64 = 150000.0;
const RESTART_RECENT_DAYS: f64 = 14.0;
const CONCURRENCY: usize = 8;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn projects_dir() -> PathBuf {
    home().join(".claude").join("projects")
}

const USAGE: &str = "Usage: sessionkit usage [options]

Shows where the tokens and the money go in your Claude Code sessions: by kind of token, by
model, main sessions against subagents, by project, the largest sessions, cold cache rebuilds,
and sessions that are cheaper to restart than to continue. Local only.

Options:
  --days <n>        only API calls from the last n days (default: all)
  --project <text>  only projects whose folder contains this text
  --top <n>         rows per table (default 10)
  --tools           which tools fill the context: the size of their results, and how often
                    later calls read them again from the cache
  --json            print the numbers as JSON
  -h, --help        show this help";

struct Options {
    days: f64,
    project: String,
    top: usize,
    json: bool,
    tools: bool,
}

fn parse_arguments(argv: &[String]) -> Options {
    let mut options = Options { days: f64::NAN, project: String::new(), top: 10, json: false, tools: false };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--days" => {
                i += 1;
                options.days = js::parse_number(argv.get(i).map(String::as_str));
            }
            "--project" => {
                i += 1;
                options.project = argv.get(i).cloned().unwrap_or_default().to_lowercase();
            }
            "--top" => {
                i += 1;
                let top = js::parse_number(argv.get(i).map(String::as_str));
                options.top = if js::truthy_number(top) { top.max(1.0) as usize } else { 10 };
            }
            "--json" => options.json = true,
            "--tools" => options.tools = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => {
                eprintln!("sessionkit usage: unknown option {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    options
}

/// An API call with the time of its entry.
#[derive(Clone)]
struct Call {
    time: f64,
    call: ApiCall,
}

impl std::ops::Deref for Call {
    type Target = ApiCall;
    fn deref(&self) -> &ApiCall {
        &self.call
    }
}

#[derive(Clone, Copy, Default)]
struct Parts {
    input: f64,
    read: f64,
    write: f64,
    output: f64,
}

impl Parts {
    /// Added in the order of KINDS: read, write, output, input.
    fn sum(&self) -> f64 {
        0.0 + self.read + self.write + self.output + self.input
    }

    fn json(&self) -> Value {
        json!({"input": num(self.input), "read": num(self.read), "write": num(self.write), "output": num(self.output)})
    }
}

/// The cost of one call in USD, split by kind of token; None when the model has no price.
fn cost_of(call: &Call) -> Option<Parts> {
    let rate = Rate::of(call.model.as_deref(), call.fast)?;
    Some(Parts {
        input: rate.input(call.input),
        read: rate.read(call.read),
        write: rate.write(call.write5m, Ttl::FiveMinutes) + rate.write(call.write1h, Ttl::OneHour),
        output: rate.output(call.output),
    })
}

// ---------------------------------------------------------------- reading

/// The project a folder belongs to: its path under home, without a worktree suffix.
fn project_of(cwd: &str, folder: &str) -> String {
    if cwd.is_empty() {
        return folder.to_string();
    }
    let mut root = cwd;
    for marker in ["/.claude/worktrees/", "/.worktrees/"] {
        if let Some(at) = root.find(marker) {
            root = &root[..at];
        }
    }
    let home = home().to_string_lossy().into_owned();
    match root.strip_prefix(&format!("{home}/")) {
        Some(rest) => format!("~/{rest}"),
        None => root.to_string(),
    }
}

struct TranscriptFile {
    path: PathBuf,
    folder: String,
    session: String,
    subagent: bool,
}

/// Every transcript: main sessions, and the subagents stored beside them.
fn transcript_files() -> Result<Vec<TranscriptFile>> {
    let mut files = Vec::new();
    let projects = projects_dir();
    for folder in js::read_dir_names(&projects)? {
        let dir = projects.join(&folder);
        for entry in js::read_dir_sorted(&dir) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_file() && name.ends_with(".jsonl") {
                let session = name[..name.len() - 6].to_string();
                files.push(TranscriptFile { path: dir.join(&name), folder: folder.clone(), session, subagent: false });
            } else if kind.is_dir() {
                let subagents = dir.join(&name).join("subagents");
                if !subagents.exists() {
                    continue;
                }
                for sub in js::read_dir_names(&subagents).unwrap_or_default().into_iter().filter(|file| file.ends_with(".jsonl")) {
                    files.push(TranscriptFile { path: subagents.join(sub), folder: folder.clone(), session: name.clone(), subagent: true });
                }
            }
        }
    }
    Ok(files)
}

struct Transcript<'a> {
    file: &'a TranscriptFile,
    calls: Vec<Call>,
    title: String,
    cwd: String,
}

/// The API calls in one transcript with their ids, before calls seen elsewhere are dropped.
fn read_calls<'a>(file: &'a TranscriptFile) -> Result<(Transcript<'a>, Vec<String>)> {
    let handle = std::fs::File::open(&file.path).map_err(|error| format!("{error}, open '{}'", file.path.display()))?;
    let mut calls = Vec::new();
    let mut ids = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let (mut title, mut cwd) = (String::new(), String::new());
    for line in std::io::BufReader::new(handle).split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        let line = line.strip_suffix('\r').unwrap_or(&line);
        if !Entry::may_be_call(line) && !line.contains("\"ai-title\"") {
            continue;
        }
        let Some(entry) = Entry::parse(line) else { continue };
        let kind = entry.kind();
        if kind == "ai-title" && js::truthy(entry.value().get("aiTitle")) {
            title = js::str_of(entry.value(), "aiTitle").to_string();
        }
        if kind != "assistant" {
            continue;
        }
        if js::truthy(entry.value().get("cwd")) && cwd.is_empty() {
            cwd = js::str_of(entry.value(), "cwd").to_string();
        }
        let Some(call) = entry.api_call() else { continue };
        let Some(id) = call.id.clone() else { continue };
        // A response is written as a line per block, and an early line can carry the output
        // count of the stream's start: the call counts the highest.
        if let Some(&at) = index.get(&id) {
            let first: &mut Call = &mut calls[at];
            first.call.output = first.call.output.max(call.output);
            continue;
        }
        index.insert(id.clone(), calls.len());
        ids.push(id);
        calls.push(Call { time: entry.time(), call });
    }
    Ok((Transcript { file, calls, title, cwd }, ids))
}

// ---------------------------------------------------------------- adding up

#[derive(Clone, Default)]
struct Totals {
    calls: usize,
    tokens: Parts,
    cost: Parts,
    unpriced: f64,
}

impl Totals {
    fn add(&mut self, call: &Call) {
        self.calls += 1;
        self.tokens.input += call.input;
        self.tokens.read += call.read;
        self.tokens.write += call.write();
        self.tokens.output += call.output;
        let Some(cost) = cost_of(call) else {
            self.unpriced += call.context() + call.output;
            return;
        };
        self.cost.read += cost.read;
        self.cost.write += cost.write;
        self.cost.output += cost.output;
        self.cost.input += cost.input;
    }

    fn fields(&self) -> serde_json::Map<String, Value> {
        let Value::Object(map) = json!({
            "calls": self.calls, "tokens": self.tokens.json(), "cost": self.cost.json(), "unpriced": num(self.unpriced),
        }) else { unreachable!() };
        map
    }

    fn json(&self) -> Value {
        Value::Object(self.fields())
    }

    fn json_with(&self, key: &str, name: &str) -> Value {
        let mut map = serde_json::Map::new();
        map.insert(key.into(), name.into());
        map.extend(self.fields());
        Value::Object(map)
    }
}

struct Rebuild {
    gap: f64,
    tokens: f64,
    cost: f64,
}

/// Cold cache rebuilds in a main session: a call after a pause longer than the cache lifetime
/// that writes a large context and reads less than it writes. A heuristic: a changed prompt
/// prefix also rewrites the cache without a pause.
fn rebuilds_of(calls: &[Call]) -> Vec<Rebuild> {
    let mut rebuilds = Vec::new();
    for pair in calls.windows(2) {
        let call = &pair[1];
        let gap = call.time - pair[0].time;
        let write = call.write();
        // The call writes at the lifetime the session uses; within it the cache was still warm.
        let ttl = if call.write1h > call.write5m { Ttl::OneHour } else { Ttl::FiveMinutes };
        if gap > ttl.ms() && write >= REBUILD_MIN_TOKENS && write > call.read {
            rebuilds.push(Rebuild { gap, tokens: write, cost: cost_of(call).map_or(0.0, |cost| cost.write) });
        }
    }
    rebuilds
}

struct SessionTotals {
    id: String,
    project: String,
    title: String,
    main: Totals,
    subagents: Totals,
    calls: Vec<Call>,
}

struct SessionRow {
    id: String,
    project: String,
    title: String,
    cost: f64,
    subagent_cost: f64,
    calls: usize,
    average_context: f64,
    last_context: f64,
    last_active: f64,
    rebuilds: usize,
    rebuild_cost: f64,
    per_call_now: Option<f64>,
    per_call_fresh: Option<f64>,
    cold_resume: Option<f64>,
}

impl SessionRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id, "project": self.project, "title": self.title, "cost": num(self.cost),
            "subagentCost": num(self.subagent_cost), "calls": self.calls, "averageContext": num(self.average_context),
            "lastContext": num(self.last_context), "lastActive": num(self.last_active), "rebuilds": self.rebuilds,
            "rebuildCost": num(self.rebuild_cost), "perCallNow": opt(self.per_call_now),
            "perCallFresh": opt(self.per_call_fresh), "coldResume": opt(self.cold_resume),
        })
    }
}

struct Report {
    window: String,
    transcripts: usize,
    total: Totals,
    by_model: Vec<(String, Totals)>,
    by_kind: [Totals; 2],
    by_project: Vec<(String, Totals)>,
    sessions: Vec<SessionRow>,
    rebuilds: (usize, f64, f64, usize),
    restart_candidates: Vec<usize>,
    unpriced_models: Vec<String>,
}

fn by_cost_desc(a: &Totals, b: &Totals) -> std::cmp::Ordering {
    b.cost.sum().partial_cmp(&a.cost.sum()).unwrap_or(std::cmp::Ordering::Equal)
}

fn summarize(transcripts: &[Transcript], options: &Options) -> Report {
    let days = options.days;
    let since = if js::truthy_number(days) { js::now() - days * 86400000.0 } else { 0.0 };
    let mut total = Totals::default();
    let mut by_model: IndexMap<String, Totals> = IndexMap::new();
    let mut by_kind = [Totals::default(), Totals::default()];
    let mut by_project: IndexMap<String, Totals> = IndexMap::new();
    let mut sessions: IndexMap<String, SessionTotals> = IndexMap::new();
    let mut unpriced_models: IndexSet<String> = IndexSet::new();

    for transcript in transcripts {
        let file = transcript.file;
        let project = project_of(&transcript.cwd, &file.folder);
        if !options.project.is_empty() && !format!("{project} {}", file.folder).to_lowercase().contains(&options.project) {
            continue;
        }
        let mut calls: Vec<Call> = transcript.calls.iter().filter(|call| call.time >= since).cloned().collect();
        calls.sort_by(|a, b| (a.time - b.time).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
        if calls.is_empty() {
            continue;
        }
        let session = sessions.entry(file.session.clone()).or_insert_with(|| SessionTotals {
            id: file.session.clone(), project: project.clone(), title: String::new(),
            main: Totals::default(), subagents: Totals::default(), calls: Vec::new(),
        });
        if !file.subagent {
            if !transcript.title.is_empty() {
                session.title = transcript.title.clone();
            }
            session.project = project.clone();
            session.calls = calls.clone();
        }
        let project_totals = by_project.entry(project).or_default();

        for call in &calls {
            total.add(call);
            by_kind[usize::from(file.subagent)].add(call);
            project_totals.add(call);
            if file.subagent { session.subagents.add(call) } else { session.main.add(call) }
            let model = call.model.clone().unwrap_or_else(|| "unknown".into());
            by_model.entry(model.clone()).or_default().add(call);
            if Rate::of(call.model.as_deref(), false).is_none() {
                unpriced_models.insert(model);
            }
        }
    }

    let recent = js::now() - RESTART_RECENT_DAYS * 86400000.0;
    let mut rows: Vec<SessionRow> = sessions.values().map(|session| {
        let calls = &session.calls;
        let rebuilds = rebuilds_of(calls);
        let last = calls.last();
        let rate = last.and_then(|call| Rate::of(call.model.as_deref(), call.fast));
        let last_context = last.map_or(0.0, |call| call.context());
        SessionRow {
            id: session.id.clone(),
            project: session.project.clone(),
            title: session.title.clone(),
            cost: session.main.cost.sum() + session.subagents.cost.sum(),
            subagent_cost: session.subagents.cost.sum(),
            calls: session.main.calls + session.subagents.calls,
            average_context: if calls.is_empty() { 0.0 } else { calls.iter().fold(0.0, |s, call| s + call.context()) / calls.len() as f64 },
            last_context,
            last_active: last.map_or(0.0, |call| call.time),
            rebuilds: rebuilds.len(),
            rebuild_cost: rebuilds.iter().fold(0.0, |s, r| s + r.cost),
            // What one more call costs in cache reads now, against a fresh start of about 15k tokens.
            per_call_now: rate.map(|r| r.read(last_context)),
            per_call_fresh: rate.map(|r| r.read(FRESH_CONTEXT_TOKENS)),
            // What a cold --resume writes once, at the 1-hour cache price Claude Code uses.
            cold_resume: rate.map(|r| r.write(last_context, Ttl::OneHour)),
        }
    }).collect();

    let all_rebuilds: Vec<Rebuild> = sessions.values().flat_map(|session| rebuilds_of(&session.calls)).collect();
    rows.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal));
    let mut restart_candidates: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].last_active >= recent && rows[i].last_context >= RESTART_MIN_CONTEXT && rows[i].per_call_now.is_some())
        .collect();
    restart_candidates.sort_by(|&a, &b| rows[b].per_call_now.partial_cmp(&rows[a].per_call_now).unwrap_or(std::cmp::Ordering::Equal));

    let mut by_model: Vec<(String, Totals)> = by_model.into_iter().collect();
    by_model.sort_by(|a, b| by_cost_desc(&a.1, &b.1));
    let mut by_project: Vec<(String, Totals)> = by_project.into_iter().collect();
    by_project.sort_by(|a, b| by_cost_desc(&a.1, &b.1));
    Report {
        window: if js::truthy_number(days) { format!("last {} days", js::number(days)) } else { "all time".into() },
        transcripts: transcripts.len(),
        total,
        by_model,
        by_kind,
        by_project,
        sessions: rows,
        rebuilds: (
            all_rebuilds.len(),
            all_rebuilds.iter().fold(0.0, |s, r| s + r.tokens),
            all_rebuilds.iter().fold(0.0, |s, r| s + r.cost),
            all_rebuilds.iter().filter(|r| r.gap > 3600000.0).count(),
        ),
        restart_candidates,
        unpriced_models: unpriced_models.into_iter().collect(),
    }
}

fn report_json(report: &Report) -> Value {
    let rows = |list: &[(String, Totals)], key: &str| Value::Array(list.iter().map(|(name, totals)| totals.json_with(key, name)).collect());
    json!({
        "window": report.window,
        "transcripts": report.transcripts,
        "total": report.total.json(),
        "byModel": rows(&report.by_model, "model"),
        "byKind": {"main": report.by_kind[0].json(), "subagent": report.by_kind[1].json()},
        "byProject": rows(&report.by_project, "project"),
        "sessions": report.sessions.iter().map(SessionRow::json).collect::<Vec<_>>(),
        "rebuilds": {
            "count": report.rebuilds.0, "tokens": num(report.rebuilds.1), "cost": num(report.rebuilds.2), "afterAnHour": report.rebuilds.3,
        },
        "restartCandidates": report.restart_candidates.iter().map(|&i| report.sessions[i].json()).collect::<Vec<_>>(),
        "unpricedModels": report.unpriced_models,
    })
}

// ---------------------------------------------------------------- printing

fn usd(value: f64) -> String {
    if value >= 100.0 { format!("${}", js::grouped(js::round(value))) } else { format!("${}", js::fixed(value, 2)) }
}

pub fn tokens(value: f64) -> String {
    if value >= 1e9 {
        format!("{}B", js::fixed(value / 1e9, 2))
    } else if value >= 1e6 {
        format!("{}M", js::fixed(value / 1e6, 1))
    } else if value >= 1e3 {
        format!("{}K", js::number(js::round(value / 1e3)))
    } else {
        js::number(value)
    }
}

fn share(part: f64, whole: f64) -> String {
    if js::truthy_number(whole) { format!("{}%", js::number(js::round((100.0 * part) / whole))) } else { "-".into() }
}

fn day(time: f64) -> String {
    if js::truthy_number(time) { js::iso(time)[..10].to_string() } else { String::new() }
}

use js::{lpad, pad};

fn table(title: &str, header: String, rows: Vec<String>) -> String {
    let rule = "-".repeat(js::len(&header));
    let mut lines = vec![String::new(), title.to_string(), String::new(), header, rule];
    lines.extend(rows);
    lines.join("\n")
}

fn render(report: &Report, options: &Options) -> String {
    let total_cost = report.total.cost.sum();
    let top = options.top;
    let mut out: Vec<String> = Vec::new();
    out.push(format!("sessionkit usage — {}: {} transcripts, {} API calls, {} API-equivalent", report.window,
        js::grouped(report.transcripts as f64), js::grouped(report.total.calls as f64), usd(total_cost)));
    out.push("Prices are the published per-token prices. With a subscription you pay a flat fee instead.".into());
    if !report.unpriced_models.is_empty() {
        out.push(format!("No price known for {}: {} tokens left out of the costs.",
            report.unpriced_models.join(", "), tokens(report.total.unpriced)));
    }

    let kinds: [(&str, f64, f64); 4] = [
        ("cache reads", report.total.tokens.read, report.total.cost.read),
        ("cache writes", report.total.tokens.write, report.total.cost.write),
        ("output", report.total.tokens.output, report.total.cost.output),
        ("uncached input", report.total.tokens.input, report.total.cost.input),
    ];
    out.push(table("Where it goes", format!("{}{}{}{}", pad("kind", 16), lpad("tokens", 10), lpad("cost", 11), lpad("share", 7)),
        kinds.iter().map(|(name, count, cost)| format!("{}{}{}{}", pad(name, 16), lpad(&tokens(*count), 10),
            lpad(&usd(*cost), 11), lpad(&share(*cost, total_cost), 7))).collect()));

    out.push(table("By model", format!("{}{}{}{}", pad("model", 26), lpad("calls", 9), lpad("cost", 11), lpad("share", 7)),
        report.by_model.iter().take(top).map(|(model, row)| format!("{}{}{}{}", pad(model, 26), lpad(&js::grouped(row.calls as f64), 9),
            lpad(&if Rate::of(Some(model), false).is_some() { usd(row.cost.sum()) } else { "no price".into() }, 11),
            lpad(&share(row.cost.sum(), total_cost), 7))).collect()));

    out.push(table("Main sessions and subagents",
        format!("{}{}{}{}{}", pad("", 14), lpad("calls", 9), lpad("tokens", 10), lpad("cost", 11), lpad("share", 7)),
        [("main sessions", &report.by_kind[0]), ("subagents", &report.by_kind[1])].iter().map(|(name, row)| {
            let all = row.tokens.input + row.tokens.read + row.tokens.write + row.tokens.output;
            format!("{}{}{}{}{}", pad(name, 14), lpad(&js::grouped(row.calls as f64), 9), lpad(&tokens(all), 10),
                lpad(&usd(row.cost.sum()), 11), lpad(&share(row.cost.sum(), total_cost), 7))
        }).collect()));

    out.push(table("By project", format!("{}{}{}{}", pad("project", 44), lpad("calls", 9), lpad("cost", 11), lpad("share", 7)),
        report.by_project.iter().take(top).map(|(project, row)| format!("{}{}{}{}", pad(project, 44),
            lpad(&js::grouped(row.calls as f64), 9), lpad(&usd(row.cost.sum()), 11), lpad(&share(row.cost.sum(), total_cost), 7))).collect()));

    let label = |row: &SessionRow| if row.title.is_empty() { row.project.clone() } else { row.title.clone() };
    out.push(table("Largest sessions (cost includes their subagents)",
        format!("{}{}{}{}{}  {}{}id", lpad("cost", 9), lpad("subagents", 10), lpad("calls", 8), lpad("avg ctx", 9), lpad("cold", 6),
            pad("last", 11), pad("title", 38)),
        report.sessions.iter().take(top).map(|row| format!("{}{}{}{}{}  {}{} {}", lpad(&usd(row.cost), 9),
            lpad(&share(row.subagent_cost, row.cost), 10), lpad(&js::grouped(row.calls as f64), 8), lpad(&tokens(row.average_context), 9),
            lpad(&row.rebuilds.to_string(), 6), pad(&day(row.last_active), 11), pad(&label(row), 37), js::head(&row.id, 8))).collect()));

    let (count, rebuild_tokens, rebuild_cost, after_an_hour) = report.rebuilds;
    out.push(String::new());
    out.push("Cold cache rebuilds".into());
    out.push(String::new());
    out.push(format!("{count} calls wrote the context again after a pause longer than the cache lifetime (5 minutes or 1 hour): {} tokens, {} ({} of the total). \
        {after_an_hour} came after more than an hour.", tokens(rebuild_tokens), usd(rebuild_cost), share(rebuild_cost, total_cost)));
    out.push("A heuristic: a large cache write after a pause, larger than the cache read in the same call.".into());

    if !report.restart_candidates.is_empty() {
        out.push(table(&format!("Cheaper to restart: active in the last {} days, last context above {}",
                js::number(RESTART_RECENT_DAYS), tokens(RESTART_MIN_CONTEXT)),
            format!("{}{}{}{}  {}command", lpad("context", 9), lpad("per call", 10), lpad("fresh", 8), lpad("cold resume", 13), pad("title", 38)),
            report.restart_candidates.iter().take(top).map(|&i| {
                let row = &report.sessions[i];
                format!("{}{}{}{}  {} sessionkit start --session {} \"…\"", lpad(&tokens(row.last_context), 9),
                    lpad(&format!("${}", js::fixed(row.per_call_now.unwrap_or(f64::NAN), 3)), 10),
                    lpad(&format!("${}", js::fixed(row.per_call_fresh.unwrap_or(f64::NAN), 3)), 8),
                    lpad(&usd(row.cold_resume.unwrap_or(f64::NAN)), 13), pad(&label(row), 37), js::head(&row.id, 8))
            }).collect()));
        out.push(String::new());
        out.push("Per call: the cache reads of one more call at the current context. Fresh: the same after a".into());
        out.push(format!("start with about {} of context. Cold resume: the one-time cache write of a --resume after a pause.",
            tokens(FRESH_CONTEXT_TOKENS)));
    }
    out.join("\n")
}

// ---------------------------------------------------------------- tool results

const TOOL_BUCKETS: [f64; 2] = [10000.0, 20000.0]; // characters of one result
const CHARS_PER_TOKEN: f64 = 4.0;

/// A tool by the name a person knows: an MCP tool by its server.
fn tool_label(name: &str) -> String {
    match name.strip_prefix("mcp__") {
        Some(rest) => format!("mcp: {}", rest.split("__").next().unwrap_or(rest)),
        None => name.to_string(),
    }
}

/// The characters of a tool result as the model reads it: its text, not its images.
fn result_chars(content: Option<&Value>) -> f64 {
    match content {
        Some(Value::String(text)) => js::len(text) as f64,
        Some(Value::Array(blocks)) => blocks.iter().map(|b| js::len(js::str_of(b, "text")) as f64).sum(),
        _ => 0.0,
    }
}

fn base64_decode(data: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut bytes, mut buffer, mut bits) = (Vec::with_capacity(data.len() * 3 / 4), 0u32, 0);
    for six in data.bytes().filter_map(value) {
        buffer = buffer << 6 | six as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    bytes
}

/// Width and height from a PNG or JPEG header.
fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be = |at: usize, len: usize| bytes.get(at..at + len).map(|b| b.iter().fold(0u32, |n, &x| n << 8 | x as u32));
    if bytes.starts_with(b"\x89PNG") {
        return Some((be(16, 4)?, be(20, 4)?));
    }
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    while *bytes.get(at)? == 0xFF {
        let marker = *bytes.get(at + 1)?;
        // SOF0..SOF15 hold the size; C4, C8 and CC are other segments in that range.
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            return Some((be(at + 7, 2)?, be(at + 5, 2)?));
        }
        at += 2 + be(at + 2, 2)? as usize;
    }
    None
}

/// Claude 4.7 and later read images at the high-resolution tier; other models at the standard one.
fn high_resolution(model: &str) -> bool {
    let Some(rest) = ["claude-opus-4", "claude-sonnet-4", "claude-haiku-4"].iter().find_map(|family| model.strip_prefix(family)) else {
        return !model.starts_with("claude-3");
    };
    let minor = rest.trim_start_matches('-').split('-').next().unwrap_or("");
    minor.len() <= 2 && minor.parse::<u32>().is_ok_and(|minor| minor >= 7)
}

/// The visual tokens of an image: one per 28x28 patch, after Claude resizes it to the limits of
/// its tier (https://platform.claude.com/docs/en/build-with-claude/vision-coordinates).
fn image_tokens(width: u32, height: u32, high_resolution: bool) -> f64 {
    let (max_edge, max_tokens) = if high_resolution { (2576, 4784) } else { (1568, 1568) };
    let count = |w: u32, h: u32| w.div_ceil(28) * h.div_ceil(28);
    let fits = |w: u32, h: u32| w.div_ceil(28) * 28 <= max_edge && h.div_ceil(28) * 28 <= max_edge && count(w, h) <= max_tokens;
    let (long, short) = (width.max(height), width.min(height).max(1));
    if fits(long, short) {
        return count(long, short) as f64;
    }
    let aspect = long as f64 / short as f64;
    let short_of = |long: u32| ((long as f64 / aspect).round_ties_even() as u32).max(1);
    let (mut lo, mut hi) = (1, long); // lo always fits; hi never fits
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if fits(mid, short_of(mid)) { lo = mid } else { hi = mid }
    }
    count(lo, short_of(lo)) as f64
}

/// The visual tokens of the images in a tool result; an image without a readable size counts as
/// the most its tier allows.
fn result_image_tokens(content: Option<&Value>, high_resolution: bool) -> f64 {
    let Some(Value::Array(blocks)) = content else { return 0.0 };
    blocks.iter().filter(|b| js::str_of(b, "type") == "image").map(|b| {
        let data = b.get("source").map(|source| js::str_of(source, "data")).unwrap_or("");
        match image_size(&base64_decode(data)) {
            Some((width, height)) => image_tokens(width, height, high_resolution),
            None => if high_resolution { 4784.0 } else { 1568.0 },
        }
    }).sum()
}

#[derive(Default)]
struct ToolTotals {
    results: usize,
    chars: f64,
    image_tokens: f64,
    over: [f64; 2],
    reread: f64,
}

impl ToolTotals {
    fn context_tokens(&self) -> f64 {
        self.chars / CHARS_PER_TOKEN + self.image_tokens
    }
}

struct ToolResult {
    tool: String,
    time: f64,
    chars: f64,
    image_tokens: f64,
    /// The API calls after it in the same transcript, up to the next compaction: each read it again.
    later_calls: f64,
}

/// The tool results in one transcript, with their tool_use ids, and the folder it ran in.
fn read_tool_results(file: &TranscriptFile) -> Result<(String, Vec<(String, ToolResult)>)> {
    let handle = std::fs::File::open(&file.path).map_err(|error| format!("{error}, open '{}'", file.path.display()))?;
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut seen_calls: HashSet<String> = HashSet::new();
    let mut calls = 0.0;
    let mut open: Vec<(String, ToolResult, f64)> = Vec::new(); // results since the last compaction, with the calls before them
    let mut done = Vec::new();
    let mut cwd = String::new();
    let mut model = String::new(); // of the last call: the next one reads its results
    let close = |open: &mut Vec<(String, ToolResult, f64)>, done: &mut Vec<(String, ToolResult)>, calls: f64| {
        done.extend(open.drain(..).map(|(id, result, before)| (id, ToolResult { later_calls: calls - before, ..result })));
    };
    for line in std::io::BufReader::new(handle).split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        if !line.contains("\"tool_") && !Entry::may_be_call(&line) && !Entry::may_be_compaction(&line) {
            continue;
        }
        let Some(entry) = Entry::parse(&line) else { continue };
        if entry.compaction().is_some() {
            close(&mut open, &mut done, calls);
            continue;
        }
        if cwd.is_empty() && js::truthy(entry.value().get("cwd")) {
            cwd = js::str_of(entry.value(), "cwd").to_string();
        }
        let Some(message) = entry.value().get("message") else { continue };
        if let Some(call) = entry.api_call() {
            if call.id.as_ref().is_none_or(|id| seen_calls.insert(id.clone())) {
                calls += 1.0;
            }
            model = call.model.unwrap_or_default();
        }
        let Some(Value::Array(blocks)) = message.get("content") else { continue };
        for block in blocks {
            match js::str_of(block, "type") {
                "tool_use" => {
                    names.insert(js::str_of(block, "id").to_string(), tool_label(js::str_of(block, "name")));
                }
                "tool_result" => {
                    let id = js::str_of(block, "tool_use_id").to_string();
                    let tool = names.get(&id).cloned().unwrap_or_else(|| "unknown".into());
                    let result = ToolResult { tool, time: entry.time(), chars: result_chars(block.get("content")),
                        image_tokens: result_image_tokens(block.get("content"), high_resolution(&model)), later_calls: 0.0 };
                    open.push((id, result, calls));
                }
                _ => {}
            }
        }
    }
    close(&mut open, &mut done, calls);
    Ok((cwd, done))
}

struct ToolReport {
    window: String,
    results: usize,
    total: ToolTotals,
    tools: Vec<(String, ToolTotals)>,
}

/// Every tool result once, by its tool_use id: a forked session copies earlier results into its
/// own file, and main sessions come first, as in read_all.
fn tool_report(files: &[TranscriptFile], options: &Options) -> Result<ToolReport> {
    let since = if js::truthy_number(options.days) { js::now() - options.days * 86400000.0 } else { 0.0 };
    let mut seen: HashSet<String> = HashSet::new();
    let mut by_tool: IndexMap<String, ToolTotals> = IndexMap::new();
    let mut total = ToolTotals::default();
    for subagent in [false, true] {
        let batch: Vec<&TranscriptFile> = files.iter().filter(|file| file.subagent == subagent).collect();
        for (file, read) in batch.iter().zip(js::pool(&batch, CONCURRENCY, |file, _| read_tool_results(file))) {
            let (cwd, results) = read?;
            let project = project_of(&cwd, &file.folder);
            if !options.project.is_empty() && !format!("{project} {}", file.folder).to_lowercase().contains(&options.project) {
                continue;
            }
            for (id, result) in results {
                if result.time < since || (!id.is_empty() && !seen.insert(id)) {
                    continue;
                }
                let reread = (result.chars / CHARS_PER_TOKEN + result.image_tokens) * result.later_calls;
                for totals in [by_tool.entry(result.tool).or_default(), &mut total] {
                    totals.results += 1;
                    totals.chars += result.chars;
                    totals.image_tokens += result.image_tokens;
                    totals.reread += reread;
                    for (i, floor) in TOOL_BUCKETS.iter().enumerate() {
                        if result.chars > *floor {
                            totals.over[i] += result.chars;
                        }
                    }
                }
            }
        }
    }
    let mut tools: Vec<(String, ToolTotals)> = by_tool.into_iter().collect();
    tools.sort_by(|a, b| b.1.reread.total_cmp(&a.1.reread));
    let window = if js::truthy_number(options.days) { format!("last {} days", js::number(options.days)) } else { "all time".into() };
    Ok(ToolReport { window, results: total.results, total, tools })
}

fn tool_json(name: &str, totals: &ToolTotals) -> Value {
    json!({"tool": name, "results": totals.results, "chars": num(totals.chars), "imageTokens": num(totals.image_tokens),
        "charsOver10k": num(totals.over[0]), "charsOver20k": num(totals.over[1]), "rereadTokens": num(totals.reread)})
}

fn render_tools(report: &ToolReport, options: &Options) -> String {
    let total = &report.total;
    let mut out = vec![format!("sessionkit usage --tools — {}: {} tool results, {} tokens in the context, read again {} times over",
        report.window, js::grouped(report.results as f64), tokens(total.context_tokens()),
        js::number(js::round(total.reread / total.context_tokens().max(1.0))))];
    let header = format!("{}{}{}{}{}{}{}{}", pad("tool", 24), lpad("results", 9), lpad("tokens", 9), lpad("share", 7), lpad("avg", 7),
        lpad(">10k", 6), lpad(">20k", 6), lpad("re-read", 10));
    let rows = report.tools.iter().take(options.top).map(|(name, t)| format!("{}{}{}{}{}{}{}{}", pad(name, 24),
        lpad(&js::grouped(t.results as f64), 9), lpad(&tokens(t.context_tokens()), 9), lpad(&share(t.context_tokens(), total.context_tokens()), 7),
        lpad(&tokens(js::round(t.context_tokens() / t.results.max(1) as f64)), 7), lpad(&share(t.over[0], t.chars), 6),
        lpad(&share(t.over[1], t.chars), 6), lpad(&share(t.reread, total.reread), 10))).collect();
    out.push(table("Tool results in the context, by what later calls read again", header, rows));
    out.push(String::new());
    out.push(format!("tokens: the text of the results at 4 characters a token, and their images at the visual tokens Claude counts for them ({} of all). share: of all tool results.",
        share(total.image_tokens, total.context_tokens())));
    out.push(">10k, >20k: the part of a tool's text that came in results above 10,000 or 20,000 characters.".into());
    out.push("re-read: each result times the API calls after it, up to the next compaction; each of them read it again.".into());
    out.push("Only tool names and numbers: MCP tools appear by their server's name. Nothing leaves this machine.".into());
    out.join("\n")
}

/// Read the transcripts in parallel; then keep every call once, by its message id, across all
/// files. A response with several content blocks is written as several lines with the same usage,
/// and a forked session copies earlier calls into its own file. Main sessions come first, so a
/// call copied into a subagent file keeps its main-session owner. Within each group the order of
/// the files decides; the JavaScript version let whichever file it read first win.
fn read_all(files: &[TranscriptFile]) -> Result<Vec<Transcript<'_>>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut transcripts = Vec::with_capacity(files.len());
    for subagent in [false, true] {
        let batch: Vec<&TranscriptFile> = files.iter().filter(|file| file.subagent == subagent).collect();
        for result in js::pool(&batch, CONCURRENCY, |file, _| read_calls(file)) {
            let (mut transcript, ids) = result?;
            let calls = std::mem::take(&mut transcript.calls);
            transcript.calls = calls.into_iter().zip(ids).filter_map(|(call, id)| seen.insert(id).then_some(call)).collect();
            transcripts.push(transcript);
        }
    }
    Ok(transcripts)
}

pub fn usage_main(argv: &[String]) -> Result<()> {
    let options = parse_arguments(argv);
    let files = transcript_files()?;
    if files.is_empty() {
        return Err(format!("No transcripts found in {}.", projects_dir().display()));
    }
    if options.tools {
        let report = tool_report(&files, &options)?;
        if report.results == 0 {
            return Err("No tool results found for these options.".into());
        }
        let json = json!({"window": report.window, "total": tool_json("all", &report.total),
            "tools": report.tools.iter().map(|(name, t)| tool_json(name, t)).collect::<Vec<_>>()});
        println!("{}", if options.json { js::stringify_pretty(&json) } else { render_tools(&report, &options) });
        return Ok(());
    }
    let transcripts = read_all(&files)?;
    let report = summarize(&transcripts, &options);
    if report.total.calls == 0 {
        return Err("No API calls found for these options.".into());
    }
    println!("{}", if options.json { js::stringify_pretty(&report_json(&report)) } else { render(&report, &options) });
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_counts_the_calls_after_it_up_to_the_next_compaction() {
        let call = |id: &str, content: Value| json!({"type": "assistant", "timestamp": "2026-09-30T09:00:00Z",
            "message": {"id": id, "model": "claude-opus-5-5", "usage": {"input_tokens": 1}, "content": content}});
        let result = |id: &str, text: &str| json!({"type": "user", "timestamp": "2026-09-30T09:00:01Z",
            "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
        let lines = [
            call("m1", json!([{"type": "tool_use", "id": "t1", "name": "Bash"}])),
            result("t1", "12345678"),
            call("m2", json!([{"type": "text", "text": "a"}])),
            call("m2", json!([{"type": "tool_use", "id": "t2", "name": "mcp__playwright__browser_snapshot"}])),
            result("t2", "1234"),
            call("m3", json!([])),
            json!({"type": "system", "subtype": "compact_boundary"}),
            call("m4", json!([])),
        ];
        let path = std::env::temp_dir().join(format!("sessionkit-tools-{}.jsonl", std::process::id()));
        std::fs::write(&path, lines.iter().map(|line| line.to_string() + "\n").collect::<String>()).unwrap();
        let file = TranscriptFile { path: path.clone(), folder: String::new(), session: String::new(), subagent: false };
        let (_, results) = read_tool_results(&file).unwrap();
        std::fs::remove_file(&path).unwrap();
        let seen: Vec<(&str, &str, f64, f64)> = results.iter().map(|(id, r)| (id.as_str(), r.tool.as_str(), r.chars, r.later_calls)).collect();
        // m2 is one call written as two lines; m4 comes after the compaction and reads neither.
        assert_eq!(seen, [("t1", "Bash", 8.0, 2.0), ("t2", "mcp: playwright", 4.0, 1.0)]);
    }

    #[test]
    fn a_call_written_as_several_lines_counts_its_last_output() {
        // Claude Code writes a line per block; the first can carry the output count of the stream's start.
        let line = |id: &str, output: f64| json!({"type": "assistant", "timestamp": "2026-09-30T09:00:00Z",
            "message": {"id": id, "model": "claude-opus-5-5", "usage": {"input_tokens": 1, "output_tokens": output}, "content": []}});
        let lines = [line("m1", 8.0), line("m1", 8.0), line("m1", 900.0), line("m2", 40.0)];
        let path = std::env::temp_dir().join(format!("sessionkit-output-{}.jsonl", std::process::id()));
        std::fs::write(&path, lines.iter().map(|line| line.to_string() + "\n").collect::<String>()).unwrap();
        let file = TranscriptFile { path: path.clone(), folder: String::new(), session: String::new(), subagent: true };
        let (transcript, ids) = read_calls(&file).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(ids, ["m1", "m2"]);
        assert_eq!(transcript.calls.iter().map(|call| call.output).collect::<Vec<f64>>(), [900.0, 40.0]);
    }

    #[test]
    fn a_pause_within_the_one_hour_cache_is_no_rebuild() {
        let call = |minute: f64, write5m: f64, write1h: f64| Call { time: minute * 60000.0, call: ApiCall {
            id: None, model: Some("claude-opus-5-5".into()), fast: false, input: 1.0, read: 0.0, write5m, write1h, output: 0.0 } };
        let gaps = |calls: &[Call]| rebuilds_of(calls).iter().map(|r| r.gap / 60000.0).collect::<Vec<f64>>();
        assert_eq!(gaps(&[call(0.0, 0.0, 50000.0), call(30.0, 0.0, 50000.0), call(100.0, 0.0, 50000.0)]), [70.0]);
        assert_eq!(gaps(&[call(0.0, 50000.0, 0.0), call(30.0, 50000.0, 0.0)]), [30.0]);
    }

    #[test]
    fn an_image_costs_the_visual_tokens_of_its_tier() {
        // The table under "Resolution and token cost" in the vision docs.
        assert_eq!(image_tokens(200, 200, false), 64.0);
        assert_eq!(image_tokens(1000, 1000, true), 1296.0);
        assert_eq!(image_tokens(1920, 1080, false), 1560.0);
        assert_eq!(image_tokens(1920, 1080, true), 2691.0);
        assert_eq!(image_tokens(2000, 1500, false), 1564.0);
        assert_eq!(image_tokens(1080, 1920, false), 1560.0);
        assert_eq!(image_tokens(3840, 2160, true), 4784.0);
        assert!(high_resolution("claude-opus-5-5") && high_resolution("claude-opus-4-7") && high_resolution("claude-sonnet-5"));
        assert!(!high_resolution("claude-haiku-4-5-20251001") && !high_resolution("claude-opus-4-20250514") && !high_resolution("claude-3-5-sonnet"));
    }

    #[test]
    fn the_size_of_an_image_comes_from_its_header() {
        let png = [b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".as_slice(), &1920u32.to_be_bytes(), &1080u32.to_be_bytes()].concat();
        assert_eq!(image_size(&png), Some((1920, 1080)));
        // An APP0 segment of 16 bytes, then SOF0 with height 600 and width 800.
        let jpeg = [[0xFF, 0xD8, 0xFF, 0xE0, 0, 16].as_slice(), &[0; 14], &[0xFF, 0xC0, 0, 17, 8, 0x02, 0x58, 0x03, 0x20]].concat();
        assert_eq!(image_size(&jpeg), Some((800, 600)));
        assert_eq!(base64_decode("iVBORw0K"), b"\x89PNG\r\n");
        let content = json!([{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}]);
        assert_eq!(result_image_tokens(Some(&content), false), 1568.0);
    }
}
