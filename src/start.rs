// sessionkit start — start a new agent session with the relevant parts of earlier Claude Code
// sessions. The agent is Claude Code, Codex or jcode. Jev (TypeSafe) makes the selections.
//
// Earlier sessions live as JSONL files under ~/.claude/projects. Jev reads them in steps:
//   1. one Noul per session: is this session relevant to the new prompt?
//   2. one Choice over the best sessions: does the prompt continue one of them?
//   3a. continuing: per turn, does it still apply, and does it state a rule; per block of a
//       conclusion, is it still open. Git adds what changed since the session.
//   3b. searching: one Noul per exchange in the shortlisted sessions: does it help?
// Everything kept stays verbatim. Jev selects; it never writes.
//
// The TypeSafe key comes from TYPESAFE_API_KEY, else from the macOS keychain, else once from
// explicitly configured 1Password fallback. See sessionkit auth --help.
//
// Names from before the rename to sessionkit still work: the JEV_START_* variables and the
// key stored in the keychain under "jev-start".

use crate::transcript::Entry;
use crate::Result;
use crate::jev::{ask_choice, ask_nouls, start_estimate, stop_estimate};
use crate::js::{self, clip, clip_middle, len};
use crate::next::{run_handoff, take_handoff};
use crate::usage::{home, projects_dir};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const PICK_SIZE: usize = 20;
const AGENTS: [&str; 3] = ["claude", "codex", "jcode"];
const JCODE_WAIT_MS: u64 = 15000;

const MAX_SESSIONS_SCANNED: usize = 150; // newest first; older sessions are rarely the context you want
pub const SHORTLIST_FLOOR: f64 = 0.3; // below this a session is a clear no
const SHORTLIST_SIZE: usize = 4;
pub const EXCERPT_FLOOR: f64 = 0.5;
const CONTEXT_BUDGET_CHARS: usize = 24000; // about six thousand tokens of earlier work
const SESSION_BUDGET_CHARS: usize = 60000; // one session to continue: about fifteen thousand tokens
const SESSION_SUMMARY_CHARS: usize = 16000;
pub const SESSION_TAIL: usize = 2; // the last exchanges show where the work stopped
const SESSION_GOAL_PROMPTS: usize = 3; // the last prompts of a session say what was going on
const OUTLINE_BUDGET_CHARS: usize = 40000; // the whole session as state, about ten thousand tokens
const SESSION_REQUEST_CHARS: usize = 160000; // state plus questions, inside the 64k-token request limit
const PICK_HERE: usize = 5; // sessions from this folder at the top of the list
const FILES_SHOWN: usize = 40;
const COMMANDS_SHOWN: usize = 12;
const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
const REQUEST_BUDGET_CHARS: usize = 100000; // stays well inside the 64k-token request limit
pub const ROUTE_CANDIDATES: usize = 5;
pub const ROUTE_CONTINUE: f64 = 0.7; // continue without asking; measured: 3 of 3 right above it, one wrong at 0.67
pub const ROUTE_ASK: f64 = 0.3; // between this and ROUTE_CONTINUE: ask the person
pub const SECTION_FLOOR: f64 = 0.7; // rules and open items sit at the top, so they must be clear yeses
const OPEN_BLOCKS_MAX: usize = 250; // newest blocks first
const OPEN_ITEMS_SHOWN: usize = 8; // measured: 11% of real blocks pass the floor, too many to read
const OPEN_BLOCK_CHARS: usize = 600;
const SINCE_COMMITS_SHOWN: usize = 10;
const SINCE_CHANGES_SHOWN: usize = 20;
const CONCURRENCY: usize = 6;

/// A setting from the environment, under its sessionkit name or its name from before the rename.
fn setting(name: &str) -> Option<String> {
    std::env::var(format!("SESSIONKIT_{name}")).or_else(|_| std::env::var(format!("JEV_START_{name}"))).ok()
}

pub fn confirm_above() -> f64 {
    js::number_or(setting("CONFIRM_ABOVE").as_deref(), 0.05)
}

fn cache_dir() -> PathBuf {
    home().join(".cache").join("sessionkit")
}

fn usage_text() -> String {
    format!("Usage: sessionkit start [options] \"prompt\" [-- agent options]
       sessionkit [options] \"prompt\"

Finds earlier Claude Code sessions relevant to the prompt and starts a new agent session
with their relevant exchanges as context.

Options:
  --agent <name>  the agent to start: claude, codex or jcode (default claude, or the
                  SESSIONKIT_AGENT environment variable). jcode takes no first prompt,
                  so sessionkit sends it to the new session once it is live; it is also
                  on the clipboard in case that fails
  --pick          choose one session to continue from a list of recent sessions in every
                  project; the newest ones from this folder come first
  --search        always search; do not let Jev decide to continue one session
  --session <s>   continue one session: an id or id prefix, \"last\" for the newest session
                  in this folder, or words from its title. Keeps its last compaction
                  summary, its first and last exchanges, and what Jev finds relevant.
                  The new session starts fresh, so a cold cache costs only this context
  --all           use the sessions of every project, not only this folder (the default
                  when this folder has no sessions)
  --top <n>       at most n earlier sessions in the context (default {SHORTLIST_SIZE})
  --yes           do not ask before spending; sessionkit first estimates the cost without
                  sending anything, and asks when it is above ${}
                  (SESSIONKIT_CONFIRM_ABOVE)
  --dry-run       print the context and the agent command; do not start the agent
  --verbose       print the first-pass score of every session
  -h, --help      show this help

Without --session, --pick or --search, Jev decides: when the prompt clearly continues one
earlier session, that session is continued; when it might, you are asked; else it searches.

Everything after -- goes to the agent unchanged, e.g. -- --model opus --permission-mode plan", js::number(confirm_above()))
}

// ---------------------------------------------------------------- arguments

pub struct Options {
    all: bool,
    top: usize,
    dry_run: bool,
    verbose: bool,
    pick: bool,
    search: bool,
    yes: bool,
    session: Option<String>,
    agent: String,
    prompt: String,
    agent_args: Vec<String>,
}

fn parse_arguments(argv: &[String]) -> Options {
    let mut options = Options {
        all: false, top: SHORTLIST_SIZE, dry_run: false, verbose: false, pick: false, search: false, yes: false,
        session: None, agent: setting("AGENT").filter(|a| !a.is_empty()).unwrap_or_else(|| "claude".into()),
        prompt: String::new(), agent_args: Vec::new(),
    };
    let mut words = Vec::new();
    let mut missing_session = false;
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        match arg {
            "--" => {
                options.agent_args = argv[i + 1..].to_vec();
                break;
            }
            "--all" => options.all = true,
            "--dry-run" => options.dry_run = true,
            "--verbose" => options.verbose = true,
            "--session" => {
                i += 1;
                options.session = argv.get(i).cloned();
                missing_session = options.session.is_none();
            }
            "--pick" => options.pick = true,
            "--search" => options.search = true,
            "--yes" => options.yes = true,
            "--agent" => {
                i += 1;
                options.agent = argv.get(i).cloned().unwrap_or_else(|| "undefined".into());
            }
            "--top" => {
                i += 1;
                let top = js::parse_number(argv.get(i).map(String::as_str));
                options.top = if js::truthy_number(top) { top.max(1.0) as usize } else { SHORTLIST_SIZE };
            }
            "-h" | "--help" => {
                println!("{}", usage_text());
                std::process::exit(0);
            }
            // A mistyped option must not start an agent with the option as its prompt; a prompt that
            // mentions an option is one quoted argument, which does not start with dashes.
            word if word.starts_with("--") => {
                eprintln!("sessionkit: unknown option {word}\n{}", usage_text());
                std::process::exit(2);
            }
            word => words.push(word.to_string()),
        }
        i += 1;
    }
    options.prompt = js::trim(&words.join(" ")).to_string();
    if options.prompt.is_empty() || missing_session {
        eprintln!("{}", usage_text());
        std::process::exit(2);
    }
    if !AGENTS.contains(&options.agent.as_str()) {
        eprintln!("sessionkit: unknown agent \"{}\"; use {}.", options.agent, AGENTS.join(", "));
        std::process::exit(2);
    }
    options
}

// ---------------------------------------------------------------- chains

pub struct Chain {
    parents: Vec<String>,
    context_file: String,
}

pub type Chains = IndexMap<String, Chain>;

/// Sessions that sessionkit started, by their id: which sessions they came from and the context
/// file they began with. Only Claude Code sessions, because only those are searched.
fn read_chains() -> Chains {
    let mut chains = Chains::new();
    let files = [home().join(".cache").join("jev-start").join("chains.jsonl"), cache_dir().join("chains.jsonl")];
    for text in files.iter().filter_map(|file| std::fs::read_to_string(file).ok()) {
        for line in text.split('\n') {
            // A torn line from an interrupted write is skipped.
            let Some(record) = js::parse(line) else { continue };
            let child = js::str_of(&record, "child");
            if child.is_empty() {
                continue;
            }
            let parents = record["parents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            chains.insert(child.to_string(), Chain { parents, context_file: js::str_of(&record, "contextFile").to_string() });
        }
    }
    chains
}

fn record_chain(child: &str, parents: &[String], context_file: &str) -> Result<()> {
    std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    let record = json!({"child": child, "parents": parents, "contextFile": context_file, "at": js::iso(js::now())});
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(cache_dir().join("chains.jsonl")).map_err(|e| e.to_string())?;
    writeln!(file, "{}", js::stringify(&record)).map_err(|e| e.to_string())
}

#[derive(Clone)]
pub struct Scored {
    pub session: Arc<Session>,
    pub relevance: f64,
    /// The earlier sessions this one continues, when a later sessionkit session carried them on.
    pub continues: Option<Vec<String>>,
}

/// Replace a session that a later sessionkit session continued by that newer session, which
/// carries the work on. The newer one takes the higher relevance of the two: its own card may
/// be about the latest step only, while the prompt names the older subject. Changes the
/// relevance of the items in place, as the JavaScript version does, and returns the rest ranked.
fn follow_chains(scored: &mut [Scored], chains: &Chains) -> Vec<Scored> {
    let by_id: HashMap<String, usize> = scored.iter().enumerate().map(|(i, item)| (item.session.id.clone(), i)).collect();
    let mut continued: HashSet<String> = HashSet::new();
    for item in scored.iter_mut() {
        item.continues = None;
    }
    for (child_id, record) in chains {
        let Some(&child) = by_id.get(child_id) else { continue };
        for parent in record.parents.iter().filter_map(|id| by_id.get(id).copied()) {
            let (parent_relevance, parent_session) = (scored[parent].relevance, scored[parent].session.clone());
            let item = &mut scored[child];
            item.relevance = item.relevance.max(parent_relevance);
            let name = if parent_session.title.is_empty() { parent_session.id.clone() } else { parent_session.title.clone() };
            item.continues.get_or_insert_with(Vec::new).push(name);
            continued.insert(parent_session.id.clone());
        }
    }
    let mut kept: Vec<Scored> = scored.iter().filter(|item| !continued.contains(&item.session.id)).cloned().collect();
    kept.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
    kept
}

// ---------------------------------------------------------------- sessions

#[derive(Clone)]
pub struct Exchange {
    pub prompt: String,
    /// Everything the assistant put to the person in the turn: all its texts, and its questions and plans.
    pub answer: String,
    /// The last text of the turn, usually its conclusion. The session cards show it, as they did
    /// when the routing thresholds were measured.
    pub conclusion: String,
    pub timestamp: String,
    /// The files the exchange edited, in the order of their first edit.
    pub files: Vec<String>,
    pub tool_calls: u64,
    /// The shell commands the exchange ran, collapsed to one line each.
    pub commands: Vec<String>,
    /// Sent by a program, not typed: an SDK run or a message from another agent.
    pub automated: bool,
    /// Which planted turn this is, in a measurement.
    pub plant: Option<&'static str>,
}

#[derive(Clone)]
pub struct Summary {
    pub text: String,
    pub timestamp: String,
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub path: String,
    pub title: String,
    pub cwd: String,
    pub branch: String,
    pub start: String,
    pub end: String,
    pub summary: Option<Summary>,
    pub exchanges: Vec<Exchange>,
    /// Edited files, in order of their last change, with its time.
    pub changed_files: IndexMap<String, String>,
    pub commands: Vec<String>,
}

/// Claude Code names a project folder after its path, with every non-alphanumeric as a dash.
fn project_slug(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_string() } else { "-".repeat(c.len_utf16()) }).collect()
}

fn cwd() -> String {
    std::env::current_dir().map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Session files, newest first: this folder and its worktrees, or every project.
pub fn session_files(all: bool, limit: usize) -> Result<Vec<PathBuf>> {
    let slug = project_slug(&cwd());
    let projects = projects_dir();
    let folders: Vec<String> = js::read_dir_names(&projects)?.into_iter()
        .filter(|name| all || *name == slug || name.starts_with(&format!("{slug}--"))).collect();
    if !all && folders.is_empty() {
        return session_files(true, limit);
    }
    let mut files: Vec<(PathBuf, f64)> = Vec::new();
    for folder in folders {
        for name in js::read_dir_names(&projects.join(&folder)).unwrap_or_default().into_iter().filter(|name| name.ends_with(".jsonl")) {
            let path = projects.join(&folder).join(name);
            if let Some(mtime) = js::mtime_ms(&path) {
                files.push((path, mtime));
            }
        }
    }
    files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(files.into_iter().take(limit).map(|(path, _)| path).collect())
}

const TITLE_WINDOW_BYTES: u64 = 256 * 1024;
static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""aiTitle":"((?:[^"\\]|\\.)*)""#).unwrap());

/// The title of a session without reading all of it. Claude Code writes the title again and
/// again, so the end of the file holds the newest one; a short session fits whole.
fn read_title(path: &Path) -> String {
    let Ok(text) = crate::cache::read_tail(path, TITLE_WINDOW_BYTES) else { return String::new() };
    TITLE.captures_iter(&text).last()
        .and_then(|found| serde_json::from_str::<String>(&format!("\"{}\"", &found[1])).ok())
        .unwrap_or_default()
}

fn megabytes(path: &str) -> String {
    js::fixed(std::fs::metadata(path).map_or(0.0, |m| m.len() as f64) / 1e6, 1)
}

fn basename(path: &str) -> String {
    Path::new(path).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Let the person choose one session from a numbered list.
fn pick_session(sessions: Vec<Arc<Session>>) -> Result<Arc<Session>> {
    if sessions.is_empty() {
        return Err("No sessions found.".into());
    }
    if !js::stdin_is_tty() {
        return Err("Choosing a session needs a terminal; use --session <id>.".into());
    }
    for (i, session) in sessions.iter().enumerate() {
        let last = session.exchanges.last().map(|e| js::collapse_space(&e.prompt)).unwrap_or_default();
        eprint!("{}. {}  {}\n      {} prompts, {} MB, {} @ {}{}\n      last: {}\n",
            js::lpad(&(i + 1).to_string(), 3), day(&session.end), or(&session.title, &session.id),
            session.exchanges.len(), megabytes(&session.path), basename(&session.cwd), or(&session.branch, "?"),
            if session.summary.is_some() { ", compacted" } else { "" }, clip(&last, 100));
    }
    let answer = js::ask("Session number: ");
    let number = js::parse_number(Some(&answer));
    let index = if number.fract() == 0.0 && number >= 1.0 { Some(number as usize - 1) } else { None };
    index.and_then(|i| sessions.get(i).cloned()).ok_or_else(|| format!("No session number {answer}."))
}

fn read_sessions(files: &[PathBuf]) -> Result<Vec<Arc<Session>>> {
    let sessions: Result<Vec<Arc<Session>>> = js::pool(files, CONCURRENCY, |path, _| read_session(path)).into_iter().collect();
    Ok(sessions?.into_iter().filter(|session| !session.exchanges.is_empty()).collect())
}

/// The session to continue: from a list with --pick, the newest here with "last", else an id
/// prefix, else words from the title. Several title matches go to the list.
fn choose_session(options: &Options) -> Result<Arc<Session>> {
    if options.pick {
        let here = session_files(false, PICK_HERE)?;
        let seen: HashSet<&PathBuf> = here.iter().collect();
        let elsewhere: Vec<PathBuf> = session_files(true, PICK_SIZE + PICK_HERE)?.into_iter().filter(|file| !seen.contains(file)).collect();
        let mut all = here.clone();
        all.extend(elsewhere);
        return pick_session(read_sessions(&all)?.into_iter().take(PICK_SIZE).collect());
    }

    let reference = options.session.clone().unwrap_or_default();
    if reference == "last" {
        for file in session_files(false, MAX_SESSIONS_SCANNED)? {
            let session = read_session(&file)?;
            if !session.exchanges.is_empty() {
                return Ok(session);
            }
        }
        return Err(format!("No sessions for {}.", cwd()));
    }
    let projects = projects_dir();
    let mut by_id = Vec::new();
    for folder in js::read_dir_names(&projects)? {
        for name in js::read_dir_names(&projects.join(&folder)).unwrap_or_default() {
            if name.ends_with(".jsonl") && name.starts_with(&reference) {
                by_id.push(projects.join(&folder).join(name));
            }
        }
    }
    if by_id.len() == 1 {
        return read_session(&by_id[0]);
    }

    let lowered = reference.to_lowercase();
    let words: Vec<&str> = lowered.split_whitespace().collect();
    let titled: Vec<PathBuf> = session_files(true, usize::MAX)?.into_iter()
        .filter(|file| {
            let title = read_title(file).to_lowercase();
            words.iter().all(|word| title.contains(word))
        })
        .collect();
    let by_title = read_sessions(&titled[..titled.len().min(PICK_SIZE)])?;
    if by_title.len() == 1 {
        return Ok(by_title[0].clone());
    }
    if by_title.len() > 1 {
        return pick_session(by_title.into_iter().take(PICK_SIZE).collect());
    }
    if by_id.len() > 1 {
        return Err(format!("\"{reference}\" matches {} session ids; give more of the id.", by_id.len()));
    }
    Err(format!("No session id or title matches \"{reference}\". Try --pick."))
}

static SYSTEM_REMINDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").unwrap());
static COMMAND_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(command|local-command)-[a-z]+>").unwrap());
static COMMAND_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</command-[a-z]+>").unwrap());
static LOCAL_COMMAND_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</local-command-[a-z]+>").unwrap());
static BASH_INPUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)^\s*<bash-input>(.*?)</bash-input>").unwrap());
static BASH_OUTPUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*<bash-(stdout|stderr)>").unwrap());

/// Remove <command-…>…</command-…> and <local-command-…>…</local-command-…> blocks: JavaScript's
/// /<(command|local-command)-[a-z]+>[\s\S]*?<\/\1-[a-z]+>/g, whose back reference Rust's regex
/// has not.
fn without_command_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut copied, mut at) = (0, 0);
    while let Some(open) = COMMAND_OPEN.captures_at(text, at) {
        let whole = open.get(0).unwrap();
        let close = if &open[1] == "command" { &COMMAND_CLOSE } else { &LOCAL_COMMAND_CLOSE };
        match close.find_at(text, whole.end()) {
            Some(end) => {
                out.push_str(&text[copied..whole.start()]);
                copied = end.end();
                at = end.end();
            }
            None => at = whole.start() + 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Remove harness wrappers so only what a person typed or read remains.
fn clean_text(text: &str) -> String {
    let text = SYSTEM_REMINDER.replace_all(text, "");
    js::trim(&without_command_blocks(&text)).to_string()
}

fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => {
            if blocks.iter().any(|block| js::str_of(block, "type") == "tool_result") {
                return String::new();
            }
            blocks.iter().filter(|block| js::str_of(block, "type") == "text")
                .map(|block| js::str_of(block, "text")).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

fn content_of(entry: &Value) -> Option<&Value> {
    entry.get("message").and_then(|message| message.get("content"))
}

/// A prompt a person typed: not a tool result, not a meta message, not a sub-agent.
fn is_human_prompt(entry: &Value) -> bool {
    if js::str_of(entry, "type") != "user" || ["isMeta", "isSidechain", "isCompactSummary"].iter().any(|key| js::truthy(entry.get(*key))) {
        return false;
    }
    if let Some(origin) = entry.get("origin").filter(|o| js::truthy(Some(o))) {
        if origin.get("kind").and_then(Value::as_str) != Some("human") {
            return false;
        }
    }
    !text_of(content_of(entry)).is_empty()
}

static SESSIONS: LazyLock<Mutex<HashMap<PathBuf, Arc<Session>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Read a session once per run: the estimate and the real run use the same sessions.
pub fn read_session(path: &Path) -> Result<Arc<Session>> {
    if let Some(session) = SESSIONS.lock().unwrap().get(path) {
        return Ok(session.clone());
    }
    let session = Arc::new(parse_session(path)?);
    SESSIONS.lock().unwrap().insert(path.to_path_buf(), session.clone());
    Ok(session)
}

/// Keep what a tool call changed or ran: edited files per exchange and per session, and commands.
fn record_tool_call(session: &mut Session, exchange: usize, call: &Value, timestamp: &str) {
    session.exchanges[exchange].tool_calls += 1;
    let input = call.get("input").filter(|i| !i.is_null());
    let field = |key: &str| input.and_then(|i| i.get(key)).filter(|v| !v.is_null());
    let file = field("file_path").or_else(|| field("notebook_path")).and_then(Value::as_str).unwrap_or("");
    let name = js::str_of(call, "name");
    if EDIT_TOOLS.contains(&name) && !file.is_empty() {
        let files = &mut session.exchanges[exchange].files;
        if !files.iter().any(|known| known == file) {
            files.push(file.to_string());
        }
        session.changed_files.shift_remove(file); // re-insert, so the map stays in order of last change
        session.changed_files.insert(file.to_string(), timestamp.to_string());
    } else if name == "Bash" {
        if let Some(command) = field("command").and_then(Value::as_str).filter(|c| !c.is_empty()) {
            session.exchanges[exchange].commands.push(command.to_string());
            session.commands.push(js::trim(&js::collapse_space(command)).to_string());
        }
    }
}

/// What a proposal made through a tool looks like as text: a question with its options, or a plan.
/// The person answered it in a tool dialog, so no assistant text carries it.
fn proposal_text(call: &Value) -> String {
    let input = call.get("input");
    match js::str_of(call, "name") {
        "AskUserQuestion" => input.and_then(|i| i.get("questions")).and_then(Value::as_array).into_iter().flatten().map(|question| {
            let options = question.get("options").and_then(Value::as_array).into_iter().flatten()
                .map(|option| js::str_of(option, "label")).filter(|label| !label.is_empty()).collect::<Vec<_>>();
            let options = if options.is_empty() { String::new() } else { format!(" Options: {}", options.join(" / ")) };
            format!("Asked the person: {}{options}", js::str_of(question, "question"))
        }).collect::<Vec<_>>().join("\n"),
        "ExitPlanMode" => match input.map_or("", |i| js::str_of(i, "plan")) {
            "" => String::new(),
            plan => format!("Proposed plan:\n{plan}"),
        },
        _ => String::new(),
    }
}

/// Read one session into exchanges: a human prompt and everything the assistant wrote before the
/// next prompt: all its texts, and the questions and plans it put to the person through tools. A
/// proposal is often not in the closing text.
fn parse_session(path: &Path) -> Result<Session> {
    let file = std::fs::File::open(path).map_err(|error| format!("{error}, open '{}'", path.display()))?;
    let id = basename(&path.to_string_lossy());
    let mut session = Session {
        id: id.strip_suffix(".jsonl").unwrap_or(&id).to_string(), path: path.to_string_lossy().into_owned(),
        title: String::new(), cwd: String::new(), branch: String::new(), start: String::new(), end: String::new(),
        summary: None, exchanges: Vec::new(), changed_files: IndexMap::new(), commands: Vec::new(),
    };
    let mut current: Option<usize> = None;
    // A compaction without a summary (sessionkit compact) writes the kept messages again after
    // the boundary, with their own timestamps; they are copies of what was read already.
    let mut boundary = String::new();
    // Some transcripts repeat a message with the same id and time; its text counts once.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for line in std::io::BufReader::new(file).split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        let Some(parsed) = Entry::parse(line.strip_suffix('\r').unwrap_or(&line)) else { continue };
        let kind = parsed.kind();
        if kind == "system" && parsed.compaction().is_some() && !parsed.is_sidechain() {
            boundary = parsed.timestamp().to_string();
        } else if !boundary.is_empty() && ["user", "assistant"].contains(&kind) && parsed.timestamp() < boundary.as_str() {
            continue;
        }
        let entry = parsed.value();
        if kind == "ai-title" && js::truthy(entry.get("aiTitle")) {
            session.title = js::str_of(entry, "aiTitle").to_string();
        }
        if kind == "summary" && js::truthy(entry.get("summary")) && session.title.is_empty() {
            session.title = js::str_of(&entry, "summary").to_string();
        }
        let timestamp = js::str_of(&entry, "timestamp").to_string();
        if !timestamp.is_empty() {
            if session.start.is_empty() {
                session.start = timestamp.clone();
            }
            session.end = timestamp.clone();
        }
        if js::truthy(entry.get("cwd")) {
            session.cwd = js::str_of(&entry, "cwd").to_string();
        }
        if js::truthy(entry.get("gitBranch")) {
            session.branch = js::str_of(&entry, "gitBranch").to_string();
        }

        if kind == "user" && js::truthy(entry.get("isCompactSummary")) && !parsed.is_sidechain() {
            session.summary = Some(Summary { text: clean_text(&text_of(content_of(&entry))), timestamp });
        } else if is_human_prompt(&entry) {
            let raw = text_of(content_of(&entry));
            // A shell command the person ran with "!" is activity, not a prompt; its output is skipped.
            if let Some(shell) = BASH_INPUT.captures(&raw) {
                session.commands.push(js::trim(&js::collapse_space(&shell[1])).to_string());
                continue;
            }
            if BASH_OUTPUT.is_match(&raw) {
                continue;
            }
            let prompt = clean_text(&raw);
            if prompt.is_empty() {
                continue;
            }
            let automated = js::str_of(&entry, "promptSource") == "sdk" || prompt.contains("<teammate-message");
            session.exchanges.push(Exchange {
                prompt, answer: String::new(), conclusion: String::new(), timestamp, files: Vec::new(), tool_calls: 0, commands: Vec::new(), automated, plant: None,
            });
            current = Some(session.exchanges.len() - 1);
        } else if let Some(exchange) = current.filter(|_| kind == "assistant" && !parsed.is_sidechain()) {
            let message_id = entry.get("message").map_or("", |message| js::str_of(message, "id"));
            let mut add = |session: &mut Session, text: String, key: &str| {
                if text.is_empty() || (!message_id.is_empty() && !seen.insert((message_id.to_string(), key.to_string()))) {
                    return false;
                }
                let answer = &mut session.exchanges[exchange].answer;
                if !answer.is_empty() {
                    answer.push_str("\n\n");
                }
                answer.push_str(&text);
                true
            };
            let text = clean_text(&text_of(content_of(&entry)));
            if add(&mut session, text.clone(), &text) {
                session.exchanges[exchange].conclusion = text;
            }
            if let Some(Value::Array(blocks)) = content_of(&entry) {
                for call in blocks.iter().filter(|block| js::str_of(block, "type") == "tool_use") {
                    record_tool_call(&mut session, exchange, call, &timestamp);
                    let proposal = proposal_text(call);
                    add(&mut session, proposal.clone(), &proposal);
                }
            }
        }
    }
    Ok(session)
}

fn day(timestamp: &str) -> &str {
    js::head(timestamp, 10)
}

/// `a || b` for strings.
fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

const CARD_PROMPT_CHARS: usize = 240;
const CARD_OUTCOME_CHARS: usize = 240;
const CARD_FINAL_CHARS: usize = 1200;
const CARD_BUDGET_CHARS: usize = 10000;

/// A card of a session for the first pass: title, place, every request with the start of its
/// outcome, and the final outcome. A long session keeps its first and last requests.
pub fn session_card(session: &Session) -> Value {
    let turns: Vec<Value> = session.exchanges.iter()
        .map(|e| json!({"request": clip(&e.prompt, CARD_PROMPT_CHARS), "outcome": clip(&e.conclusion, CARD_OUTCOME_CHARS)}))
        .collect();
    let fit = 2.max(CARD_BUDGET_CHARS / (CARD_PROMPT_CHARS + CARD_OUTCOME_CHARS + 40));
    let shown = if turns.len() > fit {
        let mut shown = turns[..fit.div_ceil(2)].to_vec();
        shown.push("[…]".into());
        shown.extend_from_slice(&turns[turns.len() - fit / 2..]);
        shown
    } else {
        turns
    };
    json!({
        "title": session.title,
        "folder": session.cwd,
        "branch": session.branch,
        "date": day(&session.end),
        "turns": shown,
        "final_outcome": clip(session.exchanges.last().map_or("", |e| &e.conclusion), CARD_FINAL_CHARS),
    })
}

// ---------------------------------------------------------------- the two passes

pub fn relevance_question(card: Value) -> Value {
    json!({
        "instructions": {
            "question": "Did one or more turns in `earlier_session` work on the subject of `new_task`: \
                the same feature, document, files, bug or decision? One matching turn is enough. \
                Sharing only the project, the tools or the language is not.",
            "earlier_session": card,
        },
        "criteria": {
            "true": "At least one turn in the earlier session works on the subject that the new task names.",
            "false": "No turn in the earlier session touches the subject of the new task.",
        },
    })
}

fn exchange_question(session: &Session, exchange: &Exchange) -> Value {
    json!({
        "instructions": {
            "question": "Does `exchange` contain a decision, result, constraint or open issue that the \
                developer needs for `new_task`? Small talk, retries and unrelated steps are no.",
            "session_title": session.title,
            "exchange": {"request": clip(&exchange.prompt, 1200), "conclusion": clip_middle(&exchange.answer, 2500)},
        },
        "criteria": {
            "true": "The exchange states something that applies directly to the new task.",
            "false": "The exchange is unrelated to the new task, or adds nothing the developer needs.",
        },
    })
}

/// A file path relative to the session folder, when it lies inside it.
fn relative<'a>(file: &'a str, folder: &str) -> &'a str {
    if folder.is_empty() { file } else { file.strip_prefix(folder).and_then(|rest| rest.strip_prefix('/')).unwrap_or(file) }
}

/// The whole session as one outline: every turn with its request, its conclusion, cut in the middle when long, and
/// the files it changed. It is fitted into the budget in stages: shorter texts first, then turns
/// left out from the middle. The first and the last turns always stay.
fn session_outline(session: &Session) -> Vec<Value> {
    let turn = |exchange: &Exchange, n: usize, chars: usize| {
        let mut turn = Map::new();
        turn.insert("turn".into(), (n + 1).into());
        turn.insert("request".into(), clip(&exchange.prompt, chars).into());
        turn.insert("conclusion".into(), clip_middle(&exchange.answer, chars).into());
        if !exchange.files.is_empty() {
            turn.insert("changed".into(), exchange.files.iter().map(|file| relative(file, &session.cwd)).collect::<Vec<_>>().into());
        }
        if exchange.tool_calls > 0 {
            turn.insert("tool_calls".into(), exchange.tool_calls.into());
        }
        Value::Object(turn)
    };
    let exchanges = &session.exchanges;
    let at = |chars: usize| exchanges.iter().enumerate().map(|(n, e)| turn(e, n, chars)).collect::<Vec<_>>();
    for chars in [600, 300, 150, 80] {
        let turns = at(chars);
        if len(&js::stringify(&Value::Array(turns.clone()))) <= OUTLINE_BUDGET_CHARS {
            return turns;
        }
    }
    let turns = at(80);
    let per_turn = len(&js::stringify(&Value::Array(turns.clone()))).div_ceil(turns.len());
    let fit = 2.max(OUTLINE_BUDGET_CHARS / per_turn);
    let head = fit.div_ceil(2);
    let tail = fit - head;
    let mut shown = turns[..head.min(turns.len())].to_vec();
    shown.push(format!("[… {} turns left out …]", turns.len() as i64 - fit as i64).into());
    shown.extend_from_slice(&turns[turns.len().saturating_sub(tail)..]);
    shown
}

fn continuation_state(prompt: &str, session: &Session) -> Value {
    let recent = &session.exchanges[session.exchanges.len().saturating_sub(SESSION_GOAL_PROMPTS)..];
    json!({
        "new_task": prompt,
        "recent_requests": recent.iter().map(|e| clip(&e.prompt, 600)).collect::<Vec<_>>(),
        "session": {
            "title": session.title,
            "folder": session.cwd,
            "has_compaction_summary": session.summary.is_some(),
            "turns": session_outline(session),
        },
    })
}

fn continuation_question(exchange: &Exchange, n: usize) -> Value {
    json!({
        "instructions": {
            "question": format!("The developer continues `session` in a fresh conversation to do `new_task`. \
                `recent_requests` shows what the session was working on last. Does the developer still \
                need to read turn {}, shown in full as `turn`? Yes when it holds a decision, \
                preference, rule, result or open issue that still applies. A preference or rule the \
                person stated still applies after the work is done. No when a later turn explicitly \
                replaced it, or when the turn only did a one-off step (a login, a rerun, a question \
                that was answered) with no lasting effect on the work.", n + 1),
            "turn": {"number": n + 1, "request": clip(&exchange.prompt, 1200), "conclusion": clip_middle(&exchange.answer, 2500)},
        },
        "criteria": {
            "true": "The turn holds something that still shapes the work: a decision, preference, rule, result or open issue.",
            "false": "A later turn replaced it, or the turn was a one-off step with no lasting effect.",
        },
    })
}

fn rule_question(exchange: &Exchange, n: usize) -> Value {
    json!({
        "instructions": {
            "question": format!("Does turn {}, typed by the person and shown as `request`, state a \
                preference, rule or constraint for how the work must be done: style, naming, wording, \
                tools, what not to do, where things go? A plain task request is no. No as well when a \
                later turn in `session.turns` replaced it.", n + 1),
            "request": clip(&exchange.prompt, 1500),
        },
        "criteria": {
            "true": "The request states a lasting preference, rule or constraint for the work.",
            "false": "The request only asks for a task, or its rule was replaced later.",
        },
    })
}

#[derive(Clone)]
pub struct Block {
    pub heading: String,
    pub text: String,
    pub turn: usize,
}

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*```").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}\s").unwrap());
static HEADING_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#+\s*").unwrap());
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([-*+]|[0-9]+[.)])\s").unwrap());
static LIST_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([-*+]|[0-9]+[.)])\s+").unwrap());

/// Cut a conclusion into blocks: paragraphs, and list items one by one. A fenced code block stays
/// inside the block around it. Each block keeps the heading it falls under.
fn blocks_of(text: &str, turn: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut heading = String::new();
    let mut current: Option<String> = None;
    let mut fenced = false;
    let close = |current: &mut Option<String>, heading: &str, blocks: &mut Vec<Block>| {
        if let Some(text) = current.take() {
            let trimmed = js::trim(&text);
            if len(trimmed) >= 20 {
                blocks.push(Block { heading: heading.to_string(), text: trimmed.to_string(), turn });
            }
        }
    };
    for line in text.split('\n') {
        if FENCE.is_match(line) {
            fenced = !fenced;
        }
        if !fenced && HEADING.is_match(line) {
            close(&mut current, &heading, &mut blocks);
            heading = HEADING_MARK.replace(line, "").into_owned();
            continue;
        }
        if !fenced && js::trim(line).is_empty() {
            close(&mut current, &heading, &mut blocks);
            continue;
        }
        if !fenced && LIST_ITEM.is_match(line) {
            close(&mut current, &heading, &mut blocks);
        }
        current = Some(match current {
            Some(text) => format!("{text}\n{line}"),
            None => line.to_string(),
        });
    }
    close(&mut current, &heading, &mut blocks);
    blocks
}

fn open_item_question(block: &Block) -> Value {
    json!({
        "instructions": {
            "question": format!("Is `block`, from the conclusion of turn {}, still open when the \
                developer continues `session`: work announced or left undone, a question or decision \
                waiting for the person, or a known problem without a fix? No when it reports finished \
                work or a fact, or when a later turn in `session.turns` shows it was done.", block.turn),
            "block": {"turn": block.turn, "section": block.heading, "text": clip(&block.text, OPEN_BLOCK_CHARS)},
        },
        "criteria": {
            "true": "The block names something still to do, to decide or to fix.",
            "false": "The block reports done work or a fact, or a later turn finished it.",
        },
    })
}

/// Run git in the session folder; an empty string when it fails or is not a repository.
fn git(folder: &str, args: &[&str]) -> String {
    Command::new("git").arg("-C").arg(folder).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()
        .filter(|output| output.status.success())
        .map(|output| js::trim_end(&String::from_utf8_lossy(&output.stdout)).to_string())
        .unwrap_or_default()
}

pub struct Since {
    branch: String,
    commits: Vec<String>,
    changes: Vec<String>,
    files: Vec<(String, &'static str)>,
}

fn lines(text: &str) -> Vec<String> {
    text.split('\n').filter(|line| !line.is_empty()).map(str::to_string).collect()
}

/// What changed after the session ended, from git and the file system: the branch now, the
/// commits since, uncommitted changes, and which files of the session changed afterwards.
fn since_session(session: &Session) -> Option<Since> {
    let folder = &session.cwd;
    if folder.is_empty() || !Path::new(folder).exists() || git(folder, &["rev-parse", "--git-dir"]).is_empty() {
        return None;
    }
    let ended = js::parse_date(&session.end);
    let branch = Some(git(folder, &["branch", "--show-current"])).filter(|b| !b.is_empty())
        .unwrap_or_else(|| git(folder, &["rev-parse", "--short", "HEAD"]));
    let commits = lines(&git(folder, &["log", &format!("--since={}", session.end), "--format=%h %s"]));
    let changes = lines(&git(folder, &["status", "--porcelain"]));
    let files = session.changed_files.keys().filter_map(|file| {
        if !Path::new(file).exists() {
            return Some((file.clone(), "removed"));
        }
        (js::mtime_ms(Path::new(file)).unwrap_or(f64::NAN) > ended + 1000.0).then(|| (file.clone(), "changed after the session"))
    }).collect();
    Some(Since { branch, commits, changes, files })
}

/// A short card of a session to choose from: what it was about and where it stopped.
fn route_card(item: &Scored) -> Value {
    let session = &item.session;
    let recent = &session.exchanges[session.exchanges.len().saturating_sub(3)..];
    let mut card = json!({
        "title": session.title,
        "folder": session.cwd,
        "date": day(&session.end),
        "last_requests": recent.iter().map(|e| clip(&e.prompt, 250)).collect::<Vec<_>>(),
        "final_outcome": clip(session.exchanges.last().map_or("", |e| &e.conclusion), 500),
    });
    if let Some(continues) = &item.continues {
        card["continues_earlier_session"] = json!(continues);
    }
    card
}

pub const NEW_TOPIC: &str = "new_topic";

pub struct Route {
    pub session: Option<Arc<Session>>,
    pub probability: f64,
    pub tokens: f64,
}

/// Decide whether the prompt continues one session. A Choice over the best candidates plus a way
/// out; its probability for the winner decides: continue, ask, or search.
pub fn route_to_session(prompt: &str, candidates: &[Scored], verbose: bool) -> Result<Route> {
    let mut criteria = Map::new();
    for (i, item) in candidates.iter().enumerate() {
        criteria.insert(format!("session_{}", i + 1), route_card(item));
    }
    criteria.insert(NEW_TOPIC.into(), "The task is new work, a question across sessions, or not a clear continuation of one session.".into());
    let question = json!({
        "instructions": format!("Does `new_task` ask to continue the work of one earlier session? Choose that \
            session. Choose {NEW_TOPIC} when the task is new work, a question that spans several \
            sessions, or does not clearly continue one of them."),
        "criteria": criteria,
    });
    let (answer, tokens) = ask_choice(&json!({"new_task": prompt}), question)?;
    let choice = answer.as_ref().map(|a| js::str_of(a, "choice").to_string()).unwrap_or_default();
    let probabilities = answer.as_ref().and_then(|a| a.get("probabilities")).and_then(Value::as_object);
    let probability = probabilities.and_then(|p| p.get(&choice)).and_then(Value::as_f64).unwrap_or(0.0);
    let index_of = |option: &str| option.split('_').nth(1).and_then(|n| n.parse::<usize>().ok()).and_then(|n| n.checked_sub(1));
    if verbose {
        for (option, p) in probabilities.into_iter().flatten() {
            let title = index_of(option).and_then(|i| candidates.get(i)).map(|c| c.session.title.as_str()).unwrap_or("");
            eprint!("  route {}  {}\n", js::fixed(p.as_f64().unwrap_or(f64::NAN), 2), or(title, option));
        }
    }
    let index = if answer.is_some() && choice != NEW_TOPIC { index_of(&choice) } else { None };
    Ok(Route { session: index.and_then(|i| candidates.get(i)).map(|c| c.session.clone()), probability, tokens })
}

struct Scoring {
    scored: Vec<Scored>,
    tokens: f64,
    scanned: usize,
}

/// First pass: score every session for relevance to the prompt.
fn score_sessions(prompt: &str, all: bool, verbose: bool) -> Result<Scoring> {
    let sessions = read_sessions(&session_files(all, MAX_SESSIONS_SCANNED)?)?;
    if sessions.is_empty() {
        return Ok(Scoring { scored: Vec::new(), tokens: 0.0, scanned: 0 });
    }
    let questions: Vec<Value> = sessions.iter().map(|session| relevance_question(session_card(session))).collect();
    let (probabilities, tokens) = ask_nouls(&json!({"new_task": prompt}), &questions, REQUEST_BUDGET_CHARS)?;
    let scanned = sessions.len();
    let mut scored: Vec<Scored> = sessions.into_iter().zip(probabilities)
        .map(|(session, relevance)| Scored { session, relevance, continues: None }).collect();
    scored.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
    if verbose {
        for item in &scored {
            eprint!("  first pass {}  {}  ({})\n", js::fixed(item.relevance, 2), or(&item.session.title, &item.session.id), day(&item.session.end));
        }
    }
    Ok(Scoring { scored, tokens, scanned })
}

#[derive(Clone)]
pub struct Kept {
    pub exchange: Exchange,
    pub relevance: f64,
    pub turn: Option<usize>,
    pub forced: bool,
}

pub struct OpenItem {
    block: Block,
    p: f64,
    order: usize,
}

pub struct Rule {
    turn: usize,
    timestamp: String,
    text: String,
}

pub struct Structure {
    rules: Vec<Rule>,
    open_items: Vec<OpenItem>,
    since: Option<Since>,
    started_from: Option<(Vec<String>, String)>,
}

pub struct Chosen {
    pub session: Arc<Session>,
    pub relevance: f64,
    pub exchanges: Vec<Kept>,
    pub total: Option<usize>,
    pub structure: Option<Structure>,
}

/// Searching: the relevant exchanges of the shortlisted sessions.
fn gather_exchanges(prompt: &str, scored: &[Scored], top: usize) -> Result<(Vec<Chosen>, f64)> {
    let state = json!({"new_task": prompt});
    let mut tokens = 0.0;
    let mut chosen = Vec::new();
    for item in scored.iter().filter(|item| item.relevance >= SHORTLIST_FLOOR).take(top) {
        let exchanges = &item.session.exchanges;
        let questions: Vec<Value> = exchanges.iter().map(|e| exchange_question(&item.session, e)).collect();
        let (probabilities, used) = ask_nouls(&state, &questions, REQUEST_BUDGET_CHARS)?;
        tokens += used;
        let kept: Vec<Kept> = exchanges.iter().zip(probabilities)
            .filter(|(_, relevance)| *relevance >= EXCERPT_FLOOR)
            .map(|(exchange, relevance)| Kept { exchange: exchange.clone(), relevance, turn: None, forced: false })
            .collect();
        if !kept.is_empty() {
            chosen.push(Chosen { session: item.session.clone(), relevance: item.relevance, exchanges: kept, total: None, structure: None });
        }
    }
    Ok((chosen, tokens))
}

pub struct Scores {
    pub applies: Vec<f64>,
    pub rules: Vec<f64>,
    pub open: Vec<f64>,
    pub blocks: Vec<Block>,
}

pub struct Continued {
    pub chosen: Chosen,
    pub tokens: f64,
    pub scores: Scores,
}

/// Continue one session. Against the whole session as state, Jev answers per turn whether it
/// still applies and whether it states a rule, and per block of a conclusion whether it is still
/// open. The first and the last turns stay regardless: the goal and where work stopped.
pub fn continue_session(prompt: &str, session: Arc<Session>, verbose: bool, chains: Option<&Chains>) -> Result<Continued> {
    if session.exchanges.is_empty() {
        return Err(format!("Session {} has no typed prompts.", session.id));
    }
    let exchanges = &session.exchanges;
    let count = exchanges.len();
    let state = continuation_state(prompt, &session);
    let question_budget = 20000.max(SESSION_REQUEST_CHARS as i64 - len(&js::stringify(&state)) as i64) as usize;

    // Open items come from the closing texts only: earlier texts narrate steps the turn itself
    // finished, and read as open. A proposal in them still reaches the context with its turn.
    let all_blocks: Vec<Block> = exchanges.iter().enumerate().flat_map(|(n, e)| blocks_of(&e.conclusion, n + 1)).collect();
    let blocks = all_blocks[all_blocks.len().saturating_sub(OPEN_BLOCKS_MAX)..].to_vec();
    let mut questions: Vec<Value> = exchanges.iter().enumerate().map(|(n, e)| continuation_question(e, n)).collect();
    questions.extend(exchanges.iter().enumerate().map(|(n, e)| rule_question(e, n)));
    questions.extend(blocks.iter().map(open_item_question));
    let (probabilities, tokens) = ask_nouls(&state, &questions, question_budget)?;
    let applies = probabilities[..count].to_vec();
    let rules = probabilities[count..2 * count].to_vec();
    let open = probabilities[2 * count..].to_vec();

    if verbose {
        for (i, exchange) in exchanges.iter().enumerate() {
            eprint!("  turn {}  applies {}  rule {}  {}\n", js::lpad(&(i + 1).to_string(), 2), js::fixed(applies[i], 2),
                js::fixed(rules[i], 2), clip(&js::collapse_space(&exchange.prompt), 70));
        }
    }
    let is_forced = |i: usize| i == 0 || i + SESSION_TAIL >= count;
    let kept: Vec<Kept> = exchanges.iter().enumerate()
        .map(|(i, exchange)| Kept { exchange: exchange.clone(), relevance: applies[i], turn: Some(i + 1), forced: is_forced(i) })
        .filter(|kept| kept.forced || kept.relevance >= EXCERPT_FLOOR)
        .collect();
    let mut open_items: Vec<OpenItem> = blocks.iter().enumerate()
        .map(|(i, block)| OpenItem { block: block.clone(), p: open[i], order: i })
        .filter(|item| item.p >= SECTION_FLOOR)
        .collect();
    open_items.sort_by(|a, b| b.p.partial_cmp(&a.p).unwrap_or(std::cmp::Ordering::Equal));
    open_items.truncate(OPEN_ITEMS_SHOWN);
    open_items.sort_by_key(|item| item.order);
    let structure = Structure {
        rules: exchanges.iter().enumerate().filter(|(i, _)| rules[*i] >= SECTION_FLOOR)
            .map(|(i, e)| Rule { turn: i + 1, timestamp: e.timestamp.clone(), text: e.prompt.clone() }).collect(),
        open_items,
        since: since_session(&session),
        started_from: chains.and_then(|chains| chains.get(&session.id)).map(|c| (c.parents.clone(), c.context_file.clone())),
    };
    Ok(Continued {
        chosen: Chosen { session: session.clone(), relevance: 1.0, exchanges: kept, total: Some(count), structure: Some(structure) },
        tokens,
        scores: Scores { applies, rules, open, blocks },
    })
}

// ---------------------------------------------------------------- context and launch

/// Rules the person stated, open items, and what changed since: the top of a continuation.
fn structure_sections(session: &Session, structure: &Structure) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| lines.extend(items.iter().map(|s| s.to_string()));
    if let Some((parents, context_file)) = &structure.started_from {
        push(&["", "### Where this session came from", ""]);
        push(&[&format!("This session was itself started by sessionkit, with context from {} earlier session(s): {}. \
            That context is in {context_file}. Read it when you need the history before this session.", parents.len(), parents.join(", "))]);
    }
    if let Some(since) = &structure.since {
        push(&["", "### Since the session", ""]);
        let moved = !session.branch.is_empty() && !since.branch.is_empty() && since.branch != session.branch;
        push(&[&format!("- Branch now: {}{}", or(&since.branch, "unknown"),
            if moved { format!(" (the session was on {})", session.branch) } else { String::new() })]);
        push(&[&if since.commits.is_empty() { "- No commits after the session.".to_string() } else {
            format!("- {} commit(s) after the session:", since.commits.len())
        }]);
        for commit in since.commits.iter().take(SINCE_COMMITS_SHOWN) {
            push(&[&format!("  - {commit}")]);
        }
        if since.commits.len() > SINCE_COMMITS_SHOWN {
            push(&[&format!("  - … and {} more", since.commits.len() - SINCE_COMMITS_SHOWN)]);
        }
        push(&[&if since.changes.is_empty() { "- No uncommitted changes now.".to_string() } else {
            format!("- {} uncommitted change(s) now:", since.changes.len())
        }]);
        for change in since.changes.iter().take(SINCE_CHANGES_SHOWN) {
            push(&[&format!("  - `{change}`")]);
        }
        if !since.files.is_empty() {
            push(&["- Files from the session that changed afterwards. Read them again before you rely on the conclusions:"]);
            for (file, state) in &since.files {
                push(&[&format!("  - {} ({state})", relative(file, &session.cwd))]);
            }
        }
    }
    if !structure.rules.is_empty() {
        push(&["", "### Rules and preferences the person stated", "", "Verbatim requests in which Jev found a lasting rule or preference.", ""]);
        for rule in &structure.rules {
            push(&[&format!("- Turn {} ({}): {}", rule.turn, day(&rule.timestamp), clip(&js::collapse_space(&rule.text), 500))]);
        }
    }
    if !structure.open_items.is_empty() {
        push(&["", "### Open items", "", "Verbatim blocks from the conclusions that Jev found still open.", ""]);
        for item in &structure.open_items {
            let text = clip(&item.block.text, OPEN_BLOCK_CHARS).split('\n').enumerate()
                .map(|(i, line)| if i > 0 { format!("  {line}") } else { line.to_string() }).collect::<Vec<_>>().join("\n");
            let heading = if item.block.heading.is_empty() { String::new() } else { format!(", {}", item.block.heading) };
            push(&[&format!("- Turn {}{heading}: {}", item.block.turn, LIST_MARK.replace(&text, ""))]);
        }
    }
    lines
}

/// Files the session changed, last changed first, and the last commands it ran.
fn activity(session: &Session) -> Vec<String> {
    let mut lines = Vec::new();
    let files: Vec<&String> = session.changed_files.keys().rev().collect();
    if !files.is_empty() {
        lines.extend([String::new(), format!("### Files changed in the session ({}, last changed first)", files.len()), String::new()]);
        lines.extend(files.iter().take(FILES_SHOWN).map(|file| format!("- {}", relative(file, &session.cwd))));
        if files.len() > FILES_SHOWN {
            lines.push(format!("- … and {} more", files.len() - FILES_SHOWN));
        }
    }
    let commands = &session.commands[session.commands.len().saturating_sub(COMMANDS_SHOWN)..];
    if !commands.is_empty() {
        lines.extend([String::new(), format!("### Last commands run ({} of {})", commands.len(), session.commands.len()), String::new()]);
        lines.extend(commands.iter().map(|command| format!("- `{}`", clip(command, 200))));
    }
    lines
}

/// Fill the budget with the forced exchanges first, then the strongest, and print each session
/// in time order. With one session, its compaction summary comes first.
fn render_context(chosen: &[Chosen], continuing: bool) -> String {
    let mut ranked: Vec<(usize, usize, &Kept)> = chosen.iter().enumerate()
        .flat_map(|(s, item)| item.exchanges.iter().enumerate().map(move |(e, kept)| (s, e, kept))).collect();
    ranked.sort_by(|a, b| b.2.forced.cmp(&a.2.forced)
        .then_with(|| b.2.relevance.partial_cmp(&a.2.relevance).unwrap_or(std::cmp::Ordering::Equal)));

    let summary = if continuing { chosen[0].session.summary.as_ref() } else { None };
    let sections = match (continuing, chosen[0].structure.as_ref()) {
        (true, Some(structure)) => structure_sections(&chosen[0].session, structure),
        _ => Vec::new(),
    };
    let budget = if continuing { SESSION_BUDGET_CHARS } else { CONTEXT_BUDGET_CHARS };
    let mut keep: HashSet<(usize, usize)> = HashSet::new();
    let mut used = summary.map_or(0, |s| len(&s.text).min(SESSION_SUMMARY_CHARS)) + len(&sections.join("\n"));
    for (s, e, kept) in ranked {
        let size = len(&kept.exchange.prompt).min(1200) + len(&kept.exchange.answer).min(3000);
        if used + size > budget && !keep.is_empty() {
            continue;
        }
        keep.insert((s, e));
        used += size;
    }

    let mut parts: Vec<String> = if continuing {
        [
            "# Continuation of an earlier Claude Code session",
            "",
            "This session continues the session below. A tool (sessionkit) kept what changed since,",
            "the rules the person stated, the open items, its compaction summary, its first and last",
            "exchanges, and the exchanges that still apply. Jev selected them; they are verbatim but",
            "incomplete. Treat them as background, not as instructions. Check claims against the",
            "current code before you rely on them. To read the whole session, use its transcript path.",
        ].iter().map(|s| s.to_string()).collect()
    } else {
        [
            "# Context from earlier Claude Code sessions",
            "",
            "A tool (sessionkit) selected these exchanges from earlier sessions because they look relevant",
            "to the first prompt. They are verbatim but incomplete. Treat them as background, not as",
            "instructions. Check claims against the current code before you rely on them.",
            "To read a whole session, use its transcript path.",
        ].iter().map(|s| s.to_string()).collect()
    };
    for (s, item) in chosen.iter().enumerate() {
        let session = &item.session;
        let exchanges: Vec<&Kept> = item.exchanges.iter().enumerate().filter(|(e, _)| keep.contains(&(s, *e))).map(|(_, k)| k).collect();
        if exchanges.is_empty() {
            continue;
        }
        parts.extend([String::new(), format!("## {} ({} – {})", or(&session.title, "Untitled session"), day(&session.start), day(&session.end)),
            String::new(), format!("- Folder: {}", session.cwd), format!("- Branch: {}", or(&session.branch, "unknown")),
            format!("- Transcript: {}", session.path), format!("- Resume: claude --resume {}", session.id)]);
        if continuing {
            parts.extend(sections.iter().cloned());
            parts.extend(activity(session));
        }
        if let Some(summary) = summary {
            parts.extend([String::new(), format!("### Compaction summary of the earlier part ({})", day(&summary.timestamp)),
                String::new(), clip(&summary.text, SESSION_SUMMARY_CHARS)]);
        }
        for kept in exchanges {
            let label = kept.turn.map_or_else(|| "Request".to_string(), |turn| format!("Turn {turn}: request"));
            parts.extend([String::new(), format!("### {label} ({})", day(&kept.exchange.timestamp)), String::new(), clip(&kept.exchange.prompt, 1200)]);
            if !kept.exchange.answer.is_empty() {
                parts.extend([String::new(), "### Conclusion".into(), String::new(), clip_middle(&kept.exchange.answer, 3000)]);
            }
        }
    }
    parts.join("\n")
}

fn jcode_active_dir() -> PathBuf {
    home().join(".jcode").join("active_pids")
}

fn jcode_sessions() -> Vec<String> {
    js::read_dir_names(&jcode_active_dir()).unwrap_or_default()
}

static JCODE_SESSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^session_[a-z]+_([0-9]+)_").unwrap());

/// Send the first message to the jcode session this process started. jcode lists every live
/// session in ~/.jcode/active_pids as session_<name>_<start in ms>_<id>. The new one is the entry
/// that was not there before and started after the launch. Sending by session id means the
/// message cannot land in another jcode window. Nothing is written to the terminal here: jcode
/// owns it by now, and the message is on the clipboard if this fails.
fn send_to_new_jcode_session(message: String, before: HashSet<String>, launched_at: f64) {
    let deadline = std::time::Instant::now() + Duration::from_millis(JCODE_WAIT_MS);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        let session = jcode_sessions().into_iter().find(|name| {
            let started = JCODE_SESSION.captures(name).map_or(f64::NAN, |found| js::parse_number(Some(&found[1])));
            !before.contains(name) && started >= launched_at - 1000.0
        });
        let Some(session) = session else { continue };
        std::thread::sleep(Duration::from_millis(500)); // let the client finish attaching before it gets input
        // The clipboard holds the message if this fails.
        let _ = Command::new("jcode").args(["transcript", "-S", &session, "--mode", "send", &message])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        return;
    }
}

struct LaunchPlan {
    command: &'static str,
    args: Vec<String>,
    first_message: Option<String>,
}

/// How to start each agent with the context:
/// - claude appends the context file to its system prompt;
/// - codex takes it as developer instructions, a TOML string (JSON escapes are valid TOML);
/// - jcode has no first-prompt argument, so the first message is sent once the session is live.
fn launch_plan(agent: &str, prompt: &str, context_file: &str, context: &str, extra_args: &[String]) -> LaunchPlan {
    let mut args = extra_args.to_vec();
    match agent {
        "claude" => {
            if !context_file.is_empty() {
                args.extend(["--append-system-prompt-file".into(), context_file.into()]);
            }
            args.push(prompt.into());
            LaunchPlan { command: "claude", args, first_message: None }
        }
        "codex" => {
            if !context_file.is_empty() {
                args.extend(["-c".into(), format!("developer_instructions={}", js::quote(context))]);
            }
            args.push(prompt.into());
            LaunchPlan { command: "codex", args, first_message: None }
        }
        _ => {
            let first_message = if context_file.is_empty() {
                prompt.to_string()
            } else {
                format!("Read {context_file} before you start: it holds context from earlier sessions. Then: {prompt}")
            };
            LaunchPlan { command: "jcode", args, first_message: Some(first_message) }
        }
    }
}

struct Collected {
    sessions: Vec<Chosen>,
    tokens: f64,
    scanned: usize,
    continuing: bool,
}

/// Select the context: continue the chosen session, or score all sessions, route, and then
/// continue one or search. While estimating, both routes are taken, so the count covers
/// whichever the real run picks.
fn collect(options: &Options, chains: &Chains, chosen: Option<&Arc<Session>>, estimating: bool) -> Result<Collected> {
    let verbose = options.verbose && !estimating;
    if let Some(session) = chosen {
        let result = continue_session(&options.prompt, session.clone(), verbose, Some(chains))?;
        return Ok(Collected { sessions: vec![result.chosen], tokens: result.tokens, scanned: 1, continuing: true });
    }
    let mut first = score_sessions(&options.prompt, options.all, verbose)?;
    let mut tokens = first.tokens;
    let candidates: Vec<Scored> = follow_chains(&mut first.scored, chains).into_iter()
        .filter(|item| item.relevance >= SHORTLIST_FLOOR).take(ROUTE_CANDIDATES).collect();

    if estimating {
        // Every score is neutral here, so rank by size: the largest sessions are the worst case
        // for whichever sessions the real run picks.
        let size = |item: &Scored| item.session.exchanges.iter().map(|e| len(&e.prompt).min(1200) + len(&e.answer).min(2500)).sum::<usize>();
        let mut largest = first.scored.clone();
        largest.sort_by_key(|item| std::cmp::Reverse(size(item)));
        if !options.search && !candidates.is_empty() {
            let top = &largest[..largest.len().min(ROUTE_CANDIDATES)];
            tokens += route_to_session(&options.prompt, top, false)?.tokens;
            tokens += continue_session(&options.prompt, largest[0].session.clone(), false, Some(chains))?.tokens;
        }
        tokens += gather_exchanges(&options.prompt, &largest, options.top)?.1;
        return Ok(Collected { sessions: Vec::new(), tokens, scanned: first.scanned, continuing: false });
    }

    let mut target = None;
    if !options.search && !candidates.is_empty() {
        let route = route_to_session(&options.prompt, &candidates, options.verbose)?;
        tokens += route.tokens;
        if let Some(session) = route.session {
            if route.probability >= ROUTE_CONTINUE {
                target = Some(session);
            } else if route.probability >= ROUTE_ASK && js::stdin_is_tty() {
                let answer = js::ask(&format!("Continue \"{}\" ({}, {})? [Y/n] ", or(&session.title, &session.id),
                    day(&session.end), basename(&session.cwd)));
                if !answer.to_lowercase().starts_with('n') {
                    target = Some(session);
                }
            }
        }
    }
    if let Some(target) = target {
        eprint!("sessionkit: continuing \"{}\"\n", or(&target.title, &target.id));
        let result = continue_session(&options.prompt, target, options.verbose, Some(chains))?;
        return Ok(Collected { sessions: vec![result.chosen], tokens: tokens + result.tokens, scanned: first.scanned, continuing: true });
    }
    let (sessions, used) = gather_exchanges(&options.prompt, &first.scored, options.top)?;
    Ok(Collected { sessions, tokens: tokens + used, scanned: first.scanned, continuing: false })
}

static OWN_SESSION_FLAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^--(session-id|resume|continue)$|^-[rc]$").unwrap());

pub fn start_main(argv: &[String]) -> Result<()> {
    let options = parse_arguments(argv);
    let chains = read_chains();
    let chosen = if options.session.is_some() || options.pick { Some(choose_session(&options)?) } else { None };
    eprint!("{}", if chosen.is_some() { "sessionkit: reading the session…\n" } else { "sessionkit: reading earlier sessions…\n" });

    // Count what the run would send before sending anything, and ask when it is not trivial.
    start_estimate();
    let counted = collect(&options, &chains, chosen.as_ref(), true);
    let estimated = stop_estimate();
    counted?;
    eprint!("sessionkit: at most ${} ({} requests, at most {} tokens)\n",
        js::fixed(estimated.cost_usd, if estimated.cost_usd < 0.01 { 4 } else { 2 }), estimated.requests, js::grouped(estimated.tokens));
    if estimated.cost_usd > confirm_above() && !options.yes {
        let spend = js::stdin_is_tty()
            && js::ask(&format!("Spend at most ${} on Jev? [y/N] ", js::fixed(estimated.cost_usd, 2))).to_lowercase().starts_with('y');
        if !spend {
            eprint!("sessionkit: stopped before spending anything. Add --yes to skip this question.\n");
            std::process::exit(2);
        }
    }

    let Collected { sessions, tokens, scanned, continuing } = collect(&options, &chains, chosen.as_ref(), false)?;

    let cost = (tokens / 1e6) * 0.042;
    eprint!("sessionkit: {scanned} sessions read, {} relevant ({} Jev tokens, about ${})\n", sessions.len(), js::grouped(tokens), js::fixed(cost, 4));
    for item in &sessions {
        eprint!("  {}  {}  ({}{} exchanges{}, {})\n", js::fixed(item.relevance, 2), or(&item.session.title, &item.session.id),
            item.exchanges.len(), item.total.map_or(String::new(), |total| format!(" of {total}")),
            if continuing && item.session.summary.is_some() { ", with compaction summary" } else { "" }, day(&item.session.end));
    }

    let (mut context, mut context_file) = (String::new(), String::new());
    if !sessions.is_empty() {
        context = render_context(&sessions, continuing);
        std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
        context_file = cache_dir().join(format!("{}.md", js::file_stamp())).to_string_lossy().into_owned();
        std::fs::write(&context_file, &context).map_err(|e| e.to_string())?;
        eprint!("sessionkit: context in {context_file}\n");
        if options.dry_run {
            println!("{context}");
        }
    }
    let mut plan = launch_plan(&options.agent, &options.prompt, &context_file, &context, &options.agent_args);
    // A known id lets a later run see that this new session continues the ones it came from.
    let owns_session_id = !options.agent_args.iter().any(|arg| OWN_SESSION_FLAG.is_match(arg));
    let child_id = (options.agent == "claude" && !sessions.is_empty() && owns_session_id).then(|| uuid::Uuid::new_v4().to_string());
    if let Some(id) = &child_id {
        plan.args.splice(0..0, ["--session-id".to_string(), id.clone()]);
    }

    if options.dry_run {
        let shown: Vec<String> = plan.args.iter()
            .map(|arg| js::quote(&if len(arg) > 200 { format!("{}…", js::head(arg, 200)) } else { arg.clone() })).collect();
        println!("\n{} {}", plan.command, shown.join(" "));
        if let Some(message) = &plan.first_message {
            println!("first message: {message}");
        }
        return Ok(());
    }

    if let Some(message) = &plan.first_message {
        let copied = Command::new("pbcopy").stdin(Stdio::piped()).spawn().and_then(|mut child| {
            child.stdin.take().expect("piped stdin").write_all(message.as_bytes())?;
            child.wait()
        });
        if !copied.is_ok_and(|status| status.success()) {
            return Err("Command failed: pbcopy".into());
        }
        eprint!("sessionkit: jcode gets the first message once it is live. It is also on the clipboard; paste it with Cmd+V if it does not arrive.\n");
    }

    // A continued session starts in its own folder, so its paths and project settings apply.
    let folder = if continuing { sessions[0].session.cwd.clone() } else { String::new() };
    let here = cwd();
    let dir = if !folder.is_empty() && Path::new(&folder).exists() { folder } else { here.clone() };
    if dir != here {
        eprint!("sessionkit: starting {} in {dir}\n", plan.command);
    }
    if let Some(id) = &child_id {
        record_chain(id, &sessions.iter().map(|item| item.session.id.clone()).collect::<Vec<_>>(), &context_file)?;
    }
    let before: HashSet<String> = jcode_sessions().into_iter().collect();
    let launched_at = js::now();
    // SESSIONKIT_LAUNCHER tells sessionkit next inside the agent who can open the next session.
    let mut child = Command::new(plan.command).args(&plan.args).current_dir(&dir)
        .env("SESSIONKIT_LAUNCHER", std::process::id().to_string()).spawn()
        .map_err(|error| format!("spawn {}: {error}", plan.command))?;
    if let Some(message) = plan.first_message {
        std::thread::spawn(move || send_to_new_jcode_session(message, before, launched_at));
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    match take_handoff(&child.id().to_string()) {
        Some(handoff) => run_handoff(handoff),
        None => {
            use std::os::unix::process::ExitStatusExt;
            std::process::exit(if status.signal().is_some() { 1 } else { status.code().unwrap_or(0) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_blocks_go_with_their_own_closing_tag() {
        assert_eq!(without_command_blocks("a<command-name>/x</command-name>b"), "ab");
        assert_eq!(without_command_blocks("a<local-command-stdout>x</local-command-stdout>b"), "ab");
        assert_eq!(without_command_blocks("a<command-name>x</local-command-name>b"), "a<command-name>x</local-command-name>b");
        assert_eq!(without_command_blocks("<command-a>x</command-b> y"), " y");
    }

    #[test]
    fn blocks_split_on_paragraphs_and_list_items() {
        let text = "## Done\nThe build passes on every platform.\n\n- first item that is long enough\n- second item that is long enough\n```\n- not an item\n```";
        let blocks = blocks_of(text, 3);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].heading, "Done");
        assert!(blocks[2].text.contains("- not an item"));
    }

    fn parse_lines(name: &str, lines: &[Value]) -> Session {
        let path = std::env::temp_dir().join(format!("sessionkit-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&path, lines.iter().map(|line| line.to_string() + "\n").collect::<String>()).unwrap();
        let session = parse_session(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        session
    }

    fn prompt(text: &str, second: u32) -> Value {
        json!({"type": "user", "timestamp": format!("2026-01-01T00:00:{second:02}Z"), "message": {"content": text}})
    }

    fn assistant(id: &str, second: u32, content: Value) -> Value {
        json!({"type": "assistant", "timestamp": format!("2026-01-01T00:00:{second:02}Z"), "message": {"id": id, "content": content}})
    }

    #[test]
    fn a_turn_keeps_every_assistant_text() {
        let session = parse_lines("texts", &[
            prompt("what now?", 1),
            assistant("m1", 2, json!([{"type": "text", "text": "I propose to split the module."}])),
            assistant("m2", 3, json!([{"type": "tool_use", "name": "Read", "input": {"file_path": "/a.rs"}}])),
            assistant("m3", 4, json!([{"type": "text", "text": "Done reading."}])),
            // the same message again, as some transcripts repeat it
            assistant("m1", 2, json!([{"type": "text", "text": "I propose to split the module."}])),
        ]);
        assert_eq!(session.exchanges[0].answer, "I propose to split the module.\n\nDone reading.");
        assert_eq!(session.exchanges[0].conclusion, "Done reading.");
        assert_eq!(session.exchanges[0].tool_calls, 1);
    }

    #[test]
    fn a_question_to_the_person_reaches_the_answer() {
        let session = parse_lines("question", &[
            prompt("scope?", 1),
            assistant("m1", 2, json!([{"type": "tool_use", "name": "AskUserQuestion", "input": {"questions": [
                {"question": "Which points go in?", "header": "Scope", "multiSelect": false,
                 "options": [{"label": "The eight clear ones", "description": "x"}, {"label": "Config only", "description": "y"}]},
            ]}}])),
            assistant("m2", 3, json!([{"type": "tool_use", "name": "ExitPlanMode", "input": {"plan": "# Plan\nStep one."}}])),
            assistant("m3", 4, json!([{"type": "text", "text": "Started."}])),
        ]);
        assert_eq!(
            session.exchanges[0].answer,
            "Asked the person: Which points go in? Options: The eight clear ones / Config only\n\nProposed plan:\n# Plan\nStep one.\n\nStarted."
        );
    }

    #[test]
    fn a_clipped_conclusion_keeps_its_proposal() {
        let answer = format!("{} Shall I split the module?", "Long account of the work. ".repeat(200));
        let clipped = clip_middle(&answer, 3000);
        assert!(len(&clipped) <= 3000 + " […] ".len());
        assert!(clipped.starts_with("Long account"));
        assert!(clipped.ends_with("Shall I split the module?"));
    }

    #[test]
    fn slug_matches_claude_code() {
        assert_eq!(project_slug("/Users/a/my.project"), "-Users-a-my-project");
    }
}
