// sessionkit skills — find work you repeat across sessions, where a skill would save time.
//
// Code narrows, Jev judges, the person reads the top:
//   1. Every exchange (a request and the shell commands the agent ran for it) becomes a vector
//      of its commands, pairs of commands in a row, and the words of the request. Exchanges
//      that are alike are grouped; a group counts when it recurs in several sessions on several
//      days. This is local and free.
//   2. Jev reads each group, the requests beside their commands, and answers two Choices: do
//      they repeat one task, or share only a subject or only generic commands; and which
//      installed skill already covers it.
//   3. The groups are ranked on the first answer. --draft starts Claude Code on one of them to
//      write the skill; Jev only gives probabilities, so the writing is Claude's.
//
// --measure adds groups of exchanges drawn at random from different sessions, which should
// score low, and shows every group with its scores.

use crate::Result;
use crate::js;
use crate::measure::Random;
use crate::jev::{PRICE_PER_MILLION_TOKENS, model, post, start_estimate, stop_estimate};
use crate::start::{confirm_above, read_session, session_files};
use crate::usage::home;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const USAGE: &str = "Usage: sessionkit skills [options]
       sessionkit skills --draft <n>

Finds work you repeat across your Claude Code sessions, where a skill would save time. Groups
of alike exchanges are found on this machine; Jev (TypeSafe) then judges whether each group
repeats one task, and whether an installed skill already covers it. The groups are sent to
TypeSafe: your requests, clipped, and the names of the commands that ran for them.

Options:
  --min-sessions <n>  a group must recur in at least this many sessions (default 3)
  --top <n>           groups to show (default 10)
  --draft <n>         start Claude Code to write a skill for group n of the last run
  --measure           also score groups drawn at random, and show every group with its scores
  --estimate          count what a run would cost, and stop
  --yes               do not ask before spending
  -h, --help          show this help";

const MIN_COMMANDS: usize = 1; // one command that is not trivial is enough: `osascript` sending a mail
const SIMILAR: f64 = 0.35; // cosine to a group's centre to join it; tuned by eye on real sessions
const MIN_DAYS: usize = 2;
const WORD_WEIGHT: f64 = 0.5; // the commands say what was done; the words only help to tell tasks apart
const PROMPT_WORD_CHARS: usize = 500;
const CARD_EXCHANGES: usize = 12;
const CARD_REQUEST_CHARS: usize = 300;
const CARD_COMMANDS: usize = 12;
const MERGE_GATE: f64 = 0.5; // only groups that may be a task are compared, on task score and on commands
const SAME_TASK: f64 = 0.6; // measured: halves of one group 0.49–0.85, two different groups at most 0.53
const PLANTS: usize = 10;
const PLANT_SIZE: usize = 4;
const CONCURRENCY: usize = 6;

const TASK: &str = "one recurring task";
const SUBJECT: &str = "one subject, different work";
const UNRELATED: &str = "unrelated";
const NONE: &str = "none of these";

/// Commands that show or move around, and say nothing about the task.
const TRIVIAL: &[&str] = &[
    "cd", "echo", "cat", "ls", "sed", "grep", "rg", "head", "tail", "wc", "sleep", "set", "for", "do", "done", "while",
    "if", "then", "fi", "python3", "python", "true", "false", "printf", "sort", "uniq", "cut", "tr", "awk", "xargs",
    "test", "[", "[[", "export", "source", "which", "pwd", "date", "jq",
];

static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap());
static SUBCOMMAND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z:_-]+$").unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"'[^']*'|"(?:[^"\\]|\\.)*""#).unwrap());
static SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&&|\|\||[;|\n]").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Ll}{4,}").unwrap());
static FRONT_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^name:\s*"?([^"\n]+?)"?\s*$"#).unwrap());
static FRONT_DESCRIPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^description:\s*"?(.+?)"?\s*$"#).unwrap());

struct Options {
    min_sessions: usize,
    top: usize,
    draft: Option<usize>,
    measure: bool,
    estimate: bool,
    yes: bool,
}

fn parse_arguments(argv: &[String]) -> Options {
    let mut options = Options { min_sessions: 3, top: 10, draft: None, measure: false, estimate: false, yes: false };
    let number = |value: Option<&String>, flag: &str| -> usize {
        value.and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or_else(|| {
            eprintln!("sessionkit: {flag} takes a number above 0.\n{USAGE}");
            std::process::exit(2);
        })
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--min-sessions" => { i += 1; options.min_sessions = number(argv.get(i), "--min-sessions"); }
            "--top" => { i += 1; options.top = number(argv.get(i), "--top"); }
            "--draft" => { i += 1; options.draft = Some(number(argv.get(i), "--draft")); }
            "--measure" => options.measure = true,
            "--estimate" => options.estimate = true,
            "--yes" | "-y" => options.yes = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => {
                eprintln!("sessionkit: unknown option {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    options
}

// ---------------------------------------------------------------- exchanges

struct Step {
    session: String,
    path: String,
    project: String,
    day: String,
    prompt: String,
    commands: Vec<String>,
    tool_calls: u64,
}

/// The commands of a shell line that say what was done: `git push`, `gh repo`, `docker compose`.
/// Environment assignments go, a path keeps its last part, and a heredoc body or a quoted
/// argument (the script of python3 -c) is not commands.
fn command_names(command: &str) -> Vec<String> {
    let command = QUOTED.replace_all(command.split("<<").next().unwrap_or(""), "''");
    let mut names = Vec::new();
    for part in SEPARATOR.split(&command) {
        let words: Vec<&str> = part.split_whitespace().skip_while(|word| ASSIGNMENT.is_match(word)).collect();
        let Some(first) = words.first() else { continue };
        let head = first.rsplit('/').next().unwrap_or(first);
        if head.is_empty() || TRIVIAL.contains(&head) {
            continue;
        }
        let sub = words[1..].iter().take(2).find(|word| SUBCOMMAND.is_match(word));
        names.push(match sub { Some(sub) => format!("{head} {sub}"), None => head.to_string() });
    }
    names
}

fn unique(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|item| seen.insert(item.clone())).collect()
}

fn project_of(cwd: &str) -> String {
    Path::new(cwd).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| cwd.to_string())
}

fn read_steps() -> Result<(Vec<Step>, usize)> {
    let files = session_files(true, usize::MAX)?;
    let sessions = js::pool(&files, 8, |path, _| read_session(path));
    let mut steps = Vec::new();
    let mut read = 0;
    for session in sessions.into_iter().flatten() {
        read += 1;
        for exchange in &session.exchanges {
            let commands = unique(exchange.commands.iter().flat_map(|command| command_names(command)));
            if exchange.automated || commands.len() < MIN_COMMANDS {
                continue;
            }
            steps.push(Step {
                session: session.id.clone(), path: session.path.clone(), project: project_of(&session.cwd),
                day: js::head(&exchange.timestamp, 10).to_string(), prompt: exchange.prompt.clone(), commands,
                tool_calls: exchange.tool_calls,
            });
        }
    }
    steps.sort_by(|a, b| a.day.cmp(&b.day));
    Ok((steps, read))
}

// ---------------------------------------------------------------- grouping

type Vector = HashMap<String, f64>;

fn features(step: &Step) -> HashMap<String, f64> {
    let mut features: HashMap<String, f64> = HashMap::new();
    for command in &step.commands {
        *features.entry(format!("c:{command}")).or_default() += 1.0;
    }
    for pair in step.commands.windows(2) {
        *features.entry(format!("p:{}>{}", pair[0], pair[1])).or_default() += 1.0;
    }
    for word in WORD.find_iter(&js::head(&step.prompt, PROMPT_WORD_CHARS).to_lowercase()) {
        *features.entry(format!("w:{}", word.as_str())).or_default() += WORD_WEIGHT;
    }
    features
}

fn normalized(mut vector: Vector) -> Vector {
    let norm = vector.values().map(|w| w * w).sum::<f64>().sqrt();
    if norm > 0.0 {
        vector.values_mut().for_each(|w| *w /= norm);
    }
    vector
}

fn cosine(a: &Vector, b: &Vector) -> f64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    small.iter().map(|(key, w)| w * large.get(key).unwrap_or(&0.0)).sum()
}

/// TF-IDF vectors; a feature that only one exchange has cannot make a group.
fn vectors(steps: &[Step]) -> Vec<Vector> {
    let all: Vec<HashMap<String, f64>> = steps.iter().map(features).collect();
    let mut df: HashMap<&str, f64> = HashMap::new();
    for key in all.iter().flat_map(|features| features.keys()) {
        *df.entry(key).or_default() += 1.0;
    }
    let n = steps.len() as f64;
    all.iter().map(|features| {
        normalized(features.iter().filter(|(key, _)| df[key.as_str()] > 1.0)
            .map(|(key, count)| (key.clone(), (1.0 + count.ln().max(0.0)) * (n / df[key.as_str()]).ln()))
            .collect())
    }).collect()
}

struct Group {
    members: Vec<usize>,
    centre: Vector,
}

/// Each exchange joins the group whose centre it is closest to, above SIMILAR, or starts one.
fn group(vectors: &[Vector]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (i, vector) in vectors.iter().enumerate() {
        let best = groups.iter().enumerate().map(|(g, group)| (g, cosine(vector, &group.centre)))
            .filter(|(_, score)| *score > SIMILAR).max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((g, _)) => {
                let group = &mut groups[g];
                group.members.push(i);
                for (key, w) in vector {
                    *group.centre.entry(key.clone()).or_default() += w;
                }
                group.centre = normalized(std::mem::take(&mut group.centre));
            }
            None => groups.push(Group { members: vec![i], centre: vector.clone() }),
        }
    }
    groups
}

fn distinct<'a>(steps: &'a [Step], members: &[usize], field: impl Fn(&'a Step) -> &'a str) -> usize {
    members.iter().map(|&i| field(&steps[i])).collect::<HashSet<_>>().len()
}

// ---------------------------------------------------------------- judging

struct Skill {
    name: String,
    description: String,
}

fn front_matter(path: &Path) -> Option<Skill> {
    let text = std::fs::read_to_string(path).ok()?;
    let head = js::head(&text, 4000);
    let folder = path.parent()?.file_name()?.to_string_lossy().into_owned();
    let name = FRONT_NAME.captures(head).map_or(folder, |found| found[1].trim().to_string());
    let description = FRONT_DESCRIPTION.captures(head).map_or(String::new(), |found| found[1].trim().to_string());
    Some(Skill { name, description: js::clip(&description, 300) })
}

/// The skills Claude Code loads: the person's own, and those of the plugins that are enabled.
fn installed_skills() -> Vec<Skill> {
    let claude = home().join(".claude");
    let mut folders = vec![claude.join("skills")];
    let read_json = |path: PathBuf| std::fs::read_to_string(path).ok().and_then(|text| js::parse(&text));
    let enabled = read_json(claude.join("settings.json")).and_then(|s| s.get("enabledPlugins").cloned()).unwrap_or(Value::Null);
    if let Some(Value::Object(plugins)) = read_json(claude.join("plugins").join("installed_plugins.json")).and_then(|p| p.get("plugins").cloned()) {
        for (name, installs) in plugins {
            if enabled.get(&name).and_then(Value::as_bool) != Some(true) {
                continue;
            }
            if let Some(path) = installs.get(0).and_then(|install| install.get("installPath")).and_then(Value::as_str) {
                folders.push(Path::new(path).join("skills"));
            }
        }
    }
    let mut skills: Vec<Skill> = Vec::new();
    for folder in folders {
        for name in js::read_dir_names(&folder).unwrap_or_default() {
            if let Some(skill) = front_matter(&folder.join(name).join("SKILL.md")) {
                if !skills.iter().any(|known| known.name == skill.name) {
                    skills.push(skill);
                }
            }
        }
    }
    skills
}

fn card(steps: &[Step], members: &[usize]) -> Value {
    let shown = members.iter().rev().take(CARD_EXCHANGES).rev(); // the newest, in order
    Value::Array(shown.map(|&i| {
        let step = &steps[i];
        json!({"project": step.project, "date": step.day, "request": js::clip(&step.prompt, CARD_REQUEST_CHARS),
            "commands": step.commands.iter().take(CARD_COMMANDS).collect::<Vec<_>>()})
    }).collect())
}

fn questions(skills: &[Skill]) -> Value {
    let mut covered = Map::new();
    for skill in skills {
        covered.insert(skill.name.clone(), Value::String(if skill.description.is_empty() { skill.name.clone() } else { skill.description.clone() }));
    }
    covered.insert(NONE.into(), "No installed skill describes how to do this task.".into());
    json!({
        "kind": {
            "type": "choice",
            "instructions": {"question": "What do the exchanges in `exchanges` have in common? Each exchange is a request of \
                the developer, from a different session, with the shell commands that the agent ran for it."},
            "criteria": {
                TASK: "The exchanges do the same kind of job again, with much the same steps, so a written procedure \
                    would make the next time faster.",
                SUBJECT: "The exchanges return to the same subject, project or tool, but each does different work.",
                UNRELATED: "The exchanges share only generic commands; their requests are about different things.",
            },
        },
        "covered": {
            "type": "choice",
            "instructions": {"question": "Which installed skill already describes how to do the task that the exchanges repeat?"},
            "criteria": covered,
        },
    })
}

#[derive(Clone)]
struct Judged {
    task: f64,
    subject: f64,
    unrelated: f64,
    covered: Option<(String, f64)>,
    tokens: f64,
}

fn judge(card: &Value, questions: &Value) -> Result<Judged> {
    let data = post(&json!({"model": model(), "state": {"exchanges": card}, "questions": questions}))?;
    let answers = data.get("answers").ok_or_else(|| format!("TypeSafe: no answers in {}", js::head(&js::stringify(&data), 200)))?;
    let p = |option: &str| answers["kind"]["probabilities"].get(option).and_then(Value::as_f64).unwrap_or(0.0);
    let covered = answers.get("covered").and_then(|answer| {
        let choice = answer.get("choice").and_then(Value::as_str)?;
        (choice != NONE).then(|| (choice.to_string(), answer.get("confidence").and_then(Value::as_f64).unwrap_or(0.0)))
    });
    let tokens = data.get("usage").and_then(|u| u.get("input_tokens")).and_then(Value::as_f64).unwrap_or(0.0);
    Ok(Judged { task: p(TASK), subject: p(SUBJECT), unrelated: p(UNRELATED), covered, tokens })
}

// ---------------------------------------------------------------- the run

#[derive(Clone)]
struct Candidate {
    members: Vec<usize>,
    plant: bool,
    judged: Option<Judged>,
}

/// What a group ran, weighted as in the vectors: two groups can do the same task with requests
/// worded apart, so only the commands are compared.
fn command_vector(vectors: &[Vector], members: &[usize]) -> Vector {
    let mut sum = Vector::new();
    for (key, w) in members.iter().flat_map(|&i| &vectors[i]).filter(|(key, _)| key.starts_with("c:")) {
        *sum.entry(key.clone()).or_default() += w;
    }
    normalized(sum)
}

fn same_task(first: &Value, second: &Value) -> Result<(f64, f64)> {
    let question = json!({
        "type": "noul",
        "instructions": {"question": "Do the exchanges in `first` and the exchanges in `second` repeat the same task, \
            so that one skill would serve both?"},
        "criteria": {
            "true": "Both groups do the same kind of job, with much the same steps.",
            "false": "The groups do different jobs, even where they use some of the same commands.",
        },
    });
    let data = post(&json!({"model": model(), "state": {"first": first, "second": second}, "questions": {"q": question}}))?;
    let p = data.get("answers").and_then(|a| a.get("q")).and_then(|a| a.get("noul")).and_then(Value::as_f64).unwrap_or(0.0);
    Ok((p, data.get("usage").and_then(|u| u.get("input_tokens")).and_then(Value::as_f64).unwrap_or(0.0)))
}

/// Judge every group, then join the groups Jev holds to be one task. Grouping on the requests
/// splits a task whose requests are worded apart ("deploy", "merge and deploy").
fn evaluate(steps: &[Step], vectors: &[Vector], mut candidates: Vec<Candidate>, questions: &Value) -> Result<(Vec<Candidate>, f64)> {
    let cards: Vec<Value> = candidates.iter().map(|c| card(steps, &c.members)).collect();
    let results = js::pool(&cards, CONCURRENCY, |card, _| judge(card, questions));
    let mut tokens = 0.0;
    for (candidate, result) in candidates.iter_mut().zip(results) {
        let judged = result?;
        tokens += judged.tokens;
        candidate.judged = Some(judged);
    }
    let task = |c: &Candidate| c.judged.as_ref().map_or(0.0, |j| j.task);
    candidates.sort_by(|a, b| task(b).total_cmp(&task(a)));

    let open: Vec<usize> = (0..candidates.len()).filter(|&i| !candidates[i].plant && task(&candidates[i]) >= MERGE_GATE).collect();
    let commands: Vec<Vector> = candidates.iter().map(|c| command_vector(vectors, &c.members)).collect();
    let pairs: Vec<(usize, usize)> = open.iter().enumerate()
        .flat_map(|(n, &i)| open[n + 1..].iter().map(move |&j| (i, j)))
        .filter(|&(i, j)| cosine(&commands[i], &commands[j]) >= MERGE_GATE).collect();
    let answers = js::pool(&pairs, CONCURRENCY, |&(i, j), _| same_task(&cards[i], &cards[j]));
    let mut into: Vec<usize> = (0..candidates.len()).collect(); // each group points at the higher one it joins
    let root = |into: &[usize], mut i: usize| { while into[i] != i { i = into[i]; } i };
    for (&(i, j), answer) in pairs.iter().zip(answers) {
        let (p, used) = answer?;
        tokens += used;
        let (a, b) = (root(&into, i), root(&into, j));
        if p >= SAME_TASK && a != b {
            into[b.max(a)] = a.min(b);
        }
    }
    let mut merged: Vec<Candidate> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for i in 0..candidates.len() {
        let r = root(&into, i);
        match slot.get(&r) {
            Some(&at) => {
                merged[at].members.extend(candidates[i].members.iter().copied());
                merged[at].members.sort_unstable();
            }
            None => {
                slot.insert(r, merged.len());
                merged.push(candidates[i].clone());
            }
        }
    }
    Ok((merged, tokens))
}

fn last_run_file() -> PathBuf {
    home().join(".cache").join("sessionkit").join("skills.json")
}

fn command_summary(steps: &[Step], members: &[usize]) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for command in members.iter().flat_map(|&i| &steps[i].commands) {
        match counts.iter_mut().find(|(known, _)| known == command) {
            Some((_, count)) => *count += 1,
            None => counts.push((command.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts.into_iter().take(6).map(|(command, _)| command).collect()
}

fn describe(steps: &[Step], candidate: &Candidate, rank: usize) -> String {
    let members = &candidate.members;
    let judged = candidate.judged.as_ref();
    let tool_calls: u64 = members.iter().map(|&i| steps[i].tool_calls).sum();
    let mut lines = vec![format!("{rank:>2}. {}  {} sessions · {} projects · {} days · {} tool calls{}{}",
        judged.map_or("    ".into(), |j| js::fixed(j.task, 2)), distinct(steps, members, |s| &s.session),
        distinct(steps, members, |s| &s.project), distinct(steps, members, |s| &s.day), tool_calls,
        judged.and_then(|j| j.covered.as_ref()).map_or(String::new(), |(name, confidence)| format!("  covered by {name} ({})", js::fixed(*confidence, 2))),
        if candidate.plant { "  [random]" } else { "" })];
    lines.push(format!("    commands: {}", command_summary(steps, members).join(", ")));
    for &i in members.iter().rev().take(3) {
        lines.push(format!("    > {}  {}", steps[i].project, js::clip(&js::collapse_space(&steps[i].prompt), 110)));
    }
    lines.join("\n")
}

fn save_run(steps: &[Step], shown: &[&Candidate]) -> Result<()> {
    let groups: Vec<Value> = shown.iter().map(|candidate| {
        let exchanges: Vec<Value> = candidate.members.iter().map(|&i| {
            let step = &steps[i];
            json!({"session": step.session, "transcript": step.path, "project": step.project, "date": step.day,
                "request": js::clip(&step.prompt, 1200), "commands": step.commands})
        }).collect();
        json!({"exchanges": exchanges, "covered": candidate.judged.as_ref().and_then(|j| j.covered.as_ref()).map(|(name, _)| name)})
    }).collect();
    let file = last_run_file();
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&file, js::stringify_pretty(&json!({"time": js::num(js::now()), "groups": groups}))).map_err(|e| e.to_string())
}

fn draft_prompt(group: &Value) -> String {
    let mut lines = vec![
        "Across earlier Claude Code sessions I repeated the task below. Help me turn it into a skill.".to_string(),
        String::new(),
        "The exchanges, each a request of mine with the commands the agent ran for it:".to_string(),
    ];
    for exchange in group["exchanges"].as_array().into_iter().flatten() {
        lines.push(format!("- {} {} ({}): {}", js::str_of(exchange, "date"), js::str_of(exchange, "project"),
            js::str_of(exchange, "transcript"), js::collapse_space(js::str_of(exchange, "request"))));
        let commands: Vec<&str> = exchange["commands"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        lines.push(format!("  commands: {}", commands.join(", ")));
    }
    if let Some(covered) = group["covered"].as_str() {
        lines.push(String::new());
        lines.push(format!("The installed skill {covered} may already cover part of this. Check it first, and say whether \
            to extend it instead."));
    }
    lines.extend([
        String::new(),
        "Read those parts of the transcripts: find each request and follow what the agent did after it. Learn the \
         steps that worked, the mistakes that cost time, and what differed from one time to the next. Then propose \
         a skill: its name, a description that says when to use it, and the steps. Ask me before you write it to \
         ~/.claude/skills/<name>/SKILL.md."
            .to_string(),
    ]);
    lines.join("\n")
}

fn draft(n: usize) -> Result<()> {
    let text = std::fs::read_to_string(last_run_file()).map_err(|_| "no earlier run: run sessionkit skills first.".to_string())?;
    let run = js::parse(&text).ok_or("the last run could not be read: run sessionkit skills again.")?;
    let groups = run["groups"].as_array().cloned().unwrap_or_default();
    let group = groups.get(n - 1).ok_or_else(|| format!("the last run showed {} groups.", groups.len()))?;
    let status = std::process::Command::new("claude").arg(draft_prompt(group)).status().map_err(|e| format!("spawn claude: {e}"))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn plants(steps: &[Step], random: &mut Random) -> Vec<Vec<usize>> {
    let mut plants = Vec::new();
    while plants.len() < PLANTS && steps.len() >= PLANT_SIZE * 4 {
        let members: Vec<usize> = (0..PLANT_SIZE).map(|_| random.index(steps.len())).collect();
        if distinct(steps, &members, |s| &s.session) == PLANT_SIZE {
            plants.push(members);
        }
    }
    plants
}

pub fn skills_main(argv: &[String]) -> Result<()> {
    let options = parse_arguments(argv);
    if let Some(n) = options.draft {
        return draft(n);
    }
    let started = js::now();
    let (steps, read) = read_steps()?;
    if steps.is_empty() {
        return Err(format!("{read} sessions read, but no exchange ran {MIN_COMMANDS} or more commands."));
    }
    let vectors = vectors(&steps);
    let groups = group(&vectors);
    let mut candidates: Vec<Candidate> = groups.into_iter()
        .filter(|g| distinct(&steps, &g.members, |s| &s.session) >= options.min_sessions && distinct(&steps, &g.members, |s| &s.day) >= MIN_DAYS)
        .map(|g| Candidate { members: g.members, plant: false, judged: None }).collect();
    let recurring = candidates.len();
    if options.measure {
        let mut random = Random { seed: 1.0 };
        candidates.extend(plants(&steps, &mut random).into_iter().map(|members| Candidate { members, plant: true, judged: None }));
    }
    eprint!("sessionkit: {read} sessions, {} exchanges with commands, {recurring} recurring groups ({} s)\n",
        steps.len(), js::fixed((js::now() - started) / 1000.0, 1));
    if candidates.is_empty() {
        println!("No work recurs in {} or more sessions. Try --min-sessions 2.", options.min_sessions);
        return Ok(());
    }

    let skills = installed_skills();
    let questions = questions(&skills);
    start_estimate();
    let counted = evaluate(&steps, &vectors, candidates.clone(), &questions);
    let estimated = stop_estimate();
    counted?;
    eprint!("sessionkit: at most ${} ({} requests, {} skills to compare with)\n",
        js::fixed(estimated.cost_usd, if estimated.cost_usd < 0.01 { 4 } else { 2 }), estimated.requests, skills.len());
    if options.estimate {
        return Ok(());
    }
    if estimated.cost_usd > confirm_above() && !options.yes {
        let spend = js::stdin_is_tty()
            && js::ask(&format!("Spend at most ${} on Jev? [y/N] ", js::fixed(estimated.cost_usd, 2))).to_lowercase().starts_with('y');
        if !spend {
            eprint!("sessionkit: stopped before spending anything. Add --yes to skip this question.\n");
            std::process::exit(2);
        }
    }

    let judged = candidates.len();
    let (candidates, tokens) = evaluate(&steps, &vectors, candidates, &questions)?;
    eprint!("sessionkit: {} Jev tokens, about ${}; {} groups joined as one task\n", js::grouped(tokens),
        js::fixed((tokens / 1e6) * PRICE_PER_MILLION_TOKENS, 4), judged - candidates.len());
    if options.measure {
        // Two halves of one group are the same task; two groups at the top are, mostly, not.
        let top: Vec<&Candidate> = candidates.iter().filter(|c| !c.plant && c.judged.as_ref().is_some_and(|j| j.task >= MERGE_GATE)).collect();
        let mut pairs: Vec<(&str, Value, Value)> = Vec::new();
        for c in top.iter().filter(|c| c.members.len() >= 4) {
            let (even, odd): (Vec<usize>, Vec<usize>) = (c.members.iter().step_by(2).copied().collect(), c.members.iter().skip(1).step_by(2).copied().collect());
            pairs.push(("halves", card(&steps, &even), card(&steps, &odd)));
        }
        for (n, a) in top.iter().enumerate() {
            for b in &top[n + 1..] {
                pairs.push(("groups", card(&steps, &a.members), card(&steps, &b.members)));
            }
        }
        let answers = js::pool(&pairs, CONCURRENCY, |(_, a, b), _| same_task(a, b));
        for ((kind, a, _), answer) in pairs.iter().zip(answers) {
            println!("same task  {kind}  {}  {}", js::fixed(answer?.0, 2), js::clip(&js::collapse_space(js::str_of(&a[0], "request")), 60));
        }
        for candidate in &candidates {
            let j = candidate.judged.as_ref().unwrap();
            println!("{}  task {} subject {} unrelated {}{}  {}", if candidate.plant { "random" } else { "group " },
                js::fixed(j.task, 2), js::fixed(j.subject, 2), js::fixed(j.unrelated, 2),
                j.covered.as_ref().map_or(String::new(), |(name, c)| format!("  covered {name} {}", js::fixed(*c, 2))),
                js::clip(&js::collapse_space(&steps[candidate.members[0]].prompt), 70));
        }
        return Ok(());
    }

    let shown: Vec<&Candidate> = candidates.iter().filter(|c| !c.plant).take(options.top).collect();
    save_run(&steps, &shown)?;
    println!("Work you repeat, most likely one task first (the score is Jev's):\n");
    for (rank, candidate) in shown.iter().enumerate() {
        println!("{}\n", describe(&steps, candidate, rank + 1));
    }
    println!("Draft a skill for one of them: sessionkit skills --draft <n>");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_keep_what_was_done() {
        assert_eq!(command_names("cd /x && git push origin main | tail -3"), vec!["git push"]);
        assert_eq!(command_names("DB=1 APP_ENV=testing php artisan test --filter X"), vec!["php artisan"]);
        assert_eq!(command_names("/opt/homebrew/bin/gh repo create -y"), vec!["gh repo"]);
        assert_eq!(command_names("cat > f.py <<'EOF'\nimport os; os.remove('x')\nEOF"), Vec::<String>::new());
        assert_eq!(command_names("python3 -c \"import os\nprint(1)\" && gh pr view"), vec!["gh pr"]);
    }

    #[test]
    fn alike_exchanges_group_and_others_do_not() {
        let step = |prompt: &str, commands: &[&str]| Step {
            session: prompt.into(), path: String::new(), project: String::new(), day: String::new(),
            prompt: prompt.into(), commands: commands.iter().map(|c| c.to_string()).collect(), tool_calls: 0,
        };
        let steps = vec![
            step("make a new repo on github", &["gh repo", "git remote", "git push"]),
            step("push this to a new github repo", &["gh repo", "git remote", "git push"]),
            step("clean up docker, the disk is full", &["docker system", "df", "docker volume"]),
            step("free some space in docker", &["docker system", "df", "docker volume"]),
        ];
        let groups = group(&vectors(&steps));
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].members, vec![0, 1]);
    }
}
