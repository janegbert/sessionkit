// The architect's nervous system: whether a change the coding agent made needs the architect.
// The plugin's architect.js sends each plan and each Edit or Write here as it happens, and wakes
// the architect agent (plugin/agents/architect.md) only when this says so. Ordinary work costs
// one Jev question and no wait; a repository without architecture memory costs nothing.
//
// The classifier is one Choice over what the change does to the architecture, against the memory
// in ARCHITECTURE.md: the change and the rules it must agree with sit side by side in the state.
// Known debt and deliberate exceptions have bins of their own, so a change that touches them
// without adding to them does not count as relevant. Jev by default; SESSIONKIT_ARCHITECT_CLASSIFIER
// names a command that answers the same question instead. Every check is logged, woken or not,
// in ~/.cache/sessionkit/architect-checks.jsonl, so the line can be measured.

use crate::Result;
use crate::auth::key_at_hand;
use crate::claude::{cache_dir, setting};
use crate::jev::{CHARS_PER_TOKEN, ask_choice, model, post};
use crate::js::{self, num};
use serde_json::{Map, Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The memory's file name, at the root of the repository it describes.
pub const MEMORY: &str = "ARCHITECTURE.md";
/// The line onboarding writes into the memory. Without it an ARCHITECTURE.md is someone else's
/// document, and the architect stays asleep.
const MARKER: &str = "<!-- sessionkit architect";
/// Decision records live beside the memory; the architect writes them, so they are not checked.
const DECISIONS: &str = "docs/adr";

/// Relevance at or above this wakes the architect (SESSIONKIT_ARCHITECT_WAKE). Set between the
/// ordinary cases (0.00 to 0.53) and the architectural ones (0.91 to 1.00) of eval/architect; the
/// log of every check is there to set it again on real work.
const WAKE_AT: f64 = 0.6;
/// The state: the change and the memory together, under Jev's 32k tokens for the state plus the
/// longest question, with room left for the question.
const STATE_CHARS: usize = (28_000.0 * CHARS_PER_TOKEN) as usize;

/// What the change does to the architecture. The first three are not architectural; the rest are,
/// and their probabilities add up to the relevance.
pub const CATEGORIES: [(&str, &str, bool); 7] = [
    ("local", "An implementation detail inside one module: logic, a fix, naming, tests, formatting. No new \
        dependency between modules and no change in who owns what.", false),
    ("known_debt", "It touches something `architecture` lists as known debt, without adding to it.", false),
    ("exception", "It looks suspicious, but `architecture` lists it as a deliberate exception, deliberate \
        duplication or something deliberately not abstracted.", false),
    ("boundary", "It adds or changes a dependency between modules, moves a responsibility or the ownership \
        of data, or reaches into another module's persistence or internals.", true),
    ("structure", "It adds a module, layer, service, process, external dependency or broadly reused \
        abstraction, or changes a public contract, a schema or who orchestrates what.", true),
    ("duplicate", "It adds a concept or responsibility that `architecture` gives to another module.", true),
    ("expands_debt", "It adds coupling to, or widens, something `architecture` lists as known debt.", true),
];

/// What the coding agent did: a plan it presented, or an edit or a new file with its diff.
pub struct Change {
    pub kind: String,
    pub file: String,
    pub text: String,
}

/// A decision record in docs/adr/, by its path from the repository root.
pub struct Decision {
    pub file: String,
    pub text: String,
}

/// How a change relates to one recorded decision. Each record is held against the change on its
/// own: the pair in the state, not the change against everything at once.
pub const RELATIONS_TO_DECISION: [(&str, &str); 3] = [
    ("follows", "The change keeps to the decision, or does what it prescribes."),
    ("goes_against", "The change does what the decision rules out, works around it, or undoes it."),
    ("unrelated", "The change does not touch what the decision is about."),
];

/// What the classifier read: a probability per category, and per decision record a probability
/// per relation.
#[derive(Default)]
pub struct Reading {
    pub categories: Map<String, Value>,
    pub decisions: Map<String, Value>,
}

/// Scores a change against the memory and the decision records.
pub trait Classifier {
    fn name(&self) -> String;
    fn classify(&self, change: &Change, memory: &str, decisions: &[Decision]) -> Result<Reading>;
}

/// The decision records of a repository, in the order of their numbers.
fn decisions_of(root: &Path) -> Vec<Decision> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(DECISIONS)).into_iter().flatten().flatten()
        .map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|e| e == "md")).collect();
    files.sort();
    files.into_iter().filter_map(|path| {
        let text = std::fs::read_to_string(&path).ok()?;
        Some(Decision { file: format!("{DECISIONS}/{}", path.file_name()?.to_string_lossy()), text })
    }).collect()
}

/// Room for two texts in one budget: each gets all it needs when both fit; otherwise the longer
/// one is cut, but never below half.
fn shares(a: usize, b: usize, budget: usize) -> (usize, usize) {
    let half = budget / 2;
    match (a + b <= budget, a <= half, b <= half) {
        (true, _, _) => (a, b),
        (_, true, _) => (a, budget - a),
        (_, _, true) => (budget - b, b),
        _ => (half, half),
    }
}

fn state(change: &Change, memory: &str, decisions: &[Decision]) -> Value {
    let recorded: usize = decisions.iter().map(|d| js::len(&d.text)).sum();
    let (change_room, reference_room) = shares(js::len(&change.text), js::len(memory) + recorded, STATE_CHARS);
    let (memory_room, recorded_room) = shares(js::len(memory), recorded, reference_room);
    let each = recorded_room / decisions.len().max(1);
    json!({
        "change": {"kind": change.kind, "file": change.file, "diff": js::clip(&change.text, change_room)},
        "architecture": js::clip_middle(memory, memory_room),
        "decisions": decisions.iter().map(|d| json!({"file": d.file, "text": js::clip(&d.text, each)})).collect::<Vec<_>>(),
    })
}

fn category_criteria() -> Map<String, Value> {
    CATEGORIES.iter().map(|(name, words, _)| (name.to_string(), json!(js::collapse_space(words)))).collect()
}

fn relation_criteria() -> Map<String, Value> {
    RELATIONS_TO_DECISION.iter().map(|(name, words)| (name.to_string(), json!(words))).collect()
}

struct Jev;

impl Classifier for Jev {
    fn name(&self) -> String {
        "jev".into()
    }

    fn classify(&self, change: &Change, memory: &str, decisions: &[Decision]) -> Result<Reading> {
        let mut questions = Map::new();
        questions.insert("category".into(), json!({
            "type": "choice",
            "instructions": "What does `change` do to the architecture that `architecture` describes? `change` is \
                what a coding agent just did: a plan it presented, or an edit with its diff. Judge the change \
                itself, not the code around it.",
            "criteria": category_criteria(),
        }));
        for (i, decision) in decisions.iter().enumerate() {
            questions.insert(decision.file.clone(), json!({
                "type": "choice",
                "instructions": format!("How does `change` relate to the decision recorded in `decisions[{i}].text`? \
                    Judge the change itself, not the code around it."),
                "criteria": relation_criteria(),
            }));
        }
        let data = post(&json!({"model": model(), "state": state(change, memory, decisions), "questions": questions}))?;
        let probabilities = |key: &str| data.get("answers").and_then(|a| a.get(key)).and_then(|a| a.get("probabilities"))
            .and_then(Value::as_object).cloned().unwrap_or_default();
        Ok(Reading {
            categories: probabilities("category"),
            decisions: decisions.iter().map(|d| (d.file.clone(), Value::Object(probabilities(&d.file)))).collect(),
        })
    }
}

/// Any program that reads {change, architecture, decisions, categories, relations} as JSON on
/// stdin and prints {"probabilities": {category: p}, "decisions": {file: {relation: p}}}; the
/// second part may be left out.
struct Command(String);

impl Classifier for Command {
    fn name(&self) -> String {
        self.0.clone()
    }

    fn classify(&self, change: &Change, memory: &str, decisions: &[Decision]) -> Result<Reading> {
        let mut input = state(change, memory, decisions);
        input["categories"] = category_criteria().into();
        input["relations"] = relation_criteria().into();
        let mut child = std::process::Command::new(&self.0).stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped()).spawn().map_err(|e| format!("{}: {e}", self.0))?;
        child.stdin.take().map(|mut stdin| stdin.write_all(js::stringify(&input).as_bytes()));
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        let answer = js::parse(js::trim(&String::from_utf8_lossy(&output.stdout)))
            .ok_or_else(|| format!("{}: no JSON on stdout", self.0))?;
        let part = |key: &str| answer.get(key).and_then(Value::as_object).cloned().unwrap_or_default();
        Ok(Reading { categories: part("probabilities"), decisions: part("decisions") })
    }
}

/// The decision records the change most likely goes against: those where that is the top relation.
fn against(decisions: &Map<String, Value>) -> Vec<String> {
    decisions.iter().filter(|(_, relations)| {
        let p = |name: &str| relations.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        RELATIONS_TO_DECISION.iter().map(|(name, _)| *name).max_by(|a, b| p(a).total_cmp(&p(b))) == Some("goes_against")
            && p("goes_against") > 0.0
    }).map(|(file, _)| file.clone()).collect()
}

/// The repository root and the memory, from the nearest ARCHITECTURE.md that onboarding wrote.
fn memory_of(start: &Path) -> Option<(PathBuf, String)> {
    start.ancestors().find_map(|folder| {
        let text = std::fs::read_to_string(folder.join(MEMORY)).ok()?;
        text.contains(MARKER).then(|| (folder.to_path_buf(), text))
    })
}

/// The sum of the architectural categories, and the most likely category.
fn relevance(probabilities: &Map<String, Value>) -> (f64, String) {
    let p = |name: &str| probabilities.get(name).and_then(Value::as_f64).unwrap_or(0.0);
    let sum = CATEGORIES.iter().filter(|(_, _, counts)| *counts).map(|(name, _, _)| p(name)).sum();
    let top = CATEGORIES.iter().max_by(|a, b| p(a.0).total_cmp(&p(b.0))).map(|c| c.0).unwrap_or("local");
    (sum, top.to_string())
}

/// The text of a decision record the change goes against, as the coding agent's note carries it:
/// enough to judge by without reading the file, which stays the pointer for the rest.
const RECORD_CHARS: usize = 1500;

fn log(record: &Value) -> Result<()> {
    std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true)
        .open(cache_dir().join("architect-checks.jsonl")).map_err(|e| e.to_string())?;
    writeln!(file, "{}", js::stringify(record)).map_err(|e| e.to_string())
}

fn classifier() -> Option<Box<dyn Classifier>> {
    match std::env::var("SESSIONKIT_ARCHITECT_CLASSIFIER").ok().filter(|c| !c.is_empty() && c != "jev") {
        Some(command) => Some(Box::new(Command(command))),
        // A hook must not raise a 1Password prompt: without a key at hand Jev is not asked.
        None => key_at_hand().then(|| Box::new(Jev) as Box<dyn Classifier>),
    }
}

/// For the plugin's hooks: input {session_id, cwd, kind (plan, edit, create), file, change,
/// answers {wake, relevance, category, probabilities, root, against, against_text} or {wake: false,
/// reason}.
pub fn check(input: &Value, classifier: Option<&dyn Classifier>, wake_at: f64) -> Result<Value> {
    let file = js::str_of(input, "file");
    let start = if file.is_empty() { PathBuf::from(js::str_of(input, "cwd")) } else { PathBuf::from(file).parent().map(Path::to_path_buf).unwrap_or_default() };
    let Some((root, memory)) = memory_of(&start) else {
        return Ok(json!({"wake": false, "reason": "no architecture memory"}));
    };
    let relative = Path::new(file).strip_prefix(&root).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| file.to_string());
    if relative == MEMORY || relative.starts_with(&format!("{DECISIONS}/")) {
        return Ok(json!({"wake": false, "reason": "architecture memory"}));
    }
    let change = Change { kind: js::str_of(input, "kind").to_string(), file: relative, text: js::str_of(input, "change").to_string() };
    if js::trim(&change.text).is_empty() {
        return Ok(json!({"wake": false, "reason": "empty change"}));
    }
    let Some(classifier) = classifier else {
        return Ok(json!({"wake": false, "reason": "no TypeSafe key at hand"}));
    };
    let decisions = decisions_of(&root);
    let reading = classifier.classify(&change, &memory, &decisions)?;
    let probabilities = reading.categories;
    let (relevance, category) = relevance(&probabilities);
    let against = against(&reading.decisions);
    let wake = relevance >= wake_at || !against.is_empty();
    let against_text: Map<String, Value> = decisions.iter().filter(|d| against.contains(&d.file))
        .map(|d| (d.file.clone(), json!(js::clip(js::trim(&d.text), RECORD_CHARS)))).collect();
    Ok(json!({
        "wake": wake,
        "relevance": num(relevance),
        "category": category,
        "probabilities": probabilities,
        "decisions": reading.decisions,
        "against": against,
        "root": root.to_string_lossy(),
        "file": change.file,
        "classifier": classifier.name(),
        "against_text": against_text,
    }))
}

/// Every check that reached the classifier is logged, woken or not.
pub fn check_main(input: &Value) -> Result<Value> {
    let answer = check(input, classifier().as_deref(), setting("ARCHITECT_WAKE", WAKE_AT))?;
    if answer.get("probabilities").is_some() {
        log(&json!({
            "time": num(js::now()),
            "session_id": js::str_of(input, "session_id"),
            "root": answer["root"],
            "kind": js::str_of(input, "kind"),
            "file": answer["file"],
            "classifier": answer["classifier"],
            "probabilities": answer["probabilities"],
            "decisions": answer["decisions"],
            "relevance": answer["relevance"],
            "wake": answer["wake"],
        }))?;
    }
    Ok(answer)
}

/// Where sessionkit remembers the repositories whose person declined the architect.
fn declined_file() -> PathBuf {
    crate::usage::home().join(".config").join("sessionkit").join("architect.json")
}

fn declined(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file).ok().and_then(|text| js::parse(&text))
        .and_then(|v| v.get("declined").and_then(Value::as_array).cloned()).unwrap_or_default()
        .iter().filter_map(Value::as_str).map(String::from).collect()
}

fn git_root(cwd: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("git").args(["-C", cwd, "rev-parse", "--show-toplevel"]).output().ok()?;
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !root.is_empty()).then(|| PathBuf::from(root))
}

/// For the plugin, once per session: input {cwd}; answers {ask, root, question}. Asked in a git
/// repository without an onboarded memory, unless the person declined it for that repository.
pub fn offer(input: &Value, declined_in: &Path) -> Value {
    let Some(root) = git_root(js::str_of(input, "cwd")) else { return json!({"ask": false, "reason": "not a git repository"}) };
    if memory_of(&root).is_some_and(|(found, _)| found == root) {
        return json!({"ask": false, "reason": "onboarded"});
    }
    let path = root.to_string_lossy().to_string();
    if declined(declined_in).contains(&path) {
        return json!({"ask": false, "reason": "declined"});
    }
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone());
    let question = if root.join(MEMORY).exists() {
        format!("{name} has an ARCHITECTURE.md, but no architect yet. Onboard one? It takes the document as input, asks you a few questions and then watches plans and edits for architectural decisions.")
    } else {
        format!("{name} has no architect yet. Onboard one? It surveys the repository, asks you a few questions and then watches plans and edits for architectural decisions.")
    };
    json!({"ask": true, "root": path, "question": question})
}

/// For the plugin: input {root}; the person does not want the architect in that repository.
pub fn decline(input: &Value, declined_in: &Path) -> Result<Value> {
    let root = js::str_of(input, "root").to_string();
    let mut roots = declined(declined_in);
    if !root.is_empty() && !roots.contains(&root) {
        roots.push(root);
    }
    if let Some(folder) = declined_in.parent() {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    std::fs::write(declined_in, js::stringify(&json!({"declined": roots})) + "\n").map_err(|e| e.to_string())?;
    Ok(json!({"saved": declined_in.to_string_lossy()}))
}

pub fn offer_main(input: &Value) -> Value {
    offer(input, &declined_file())
}

pub fn decline_main(input: &Value) -> Result<Value> {
    decline(input, &declined_file())
}

/// One calibration question from onboarding, as the architect writes it (plugin/skills/architecture/ONBOARD.md).
#[derive(Debug)]
pub struct Calibration {
    pub text: String,
    pub question: String,
    pub recommended: String,
    pub evidence: Vec<(String, usize)>,
}

/// The numbered questions in the architect's answer; text before the first is not a question.
pub fn calibrations(answer: &str) -> Vec<Calibration> {
    let mut found: Vec<Calibration> = Vec::new();
    for line in answer.lines() {
        let trimmed = line.trim();
        let numbered = trimmed.split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if let Some((_, question)) = numbered.filter(|_| !line.starts_with(' ')) {
            found.push(Calibration { text: String::new(), question: question.to_string(), recommended: String::new(), evidence: Vec::new() });
        }
        let Some(current) = found.last_mut() else { continue };
        current.text.push_str(line);
        current.text.push('\n');
        if let Some(answer) = trimmed.strip_prefix("Recommended:") {
            current.recommended = answer.rsplit_once("(confidence").map_or(answer, |(a, _)| a).trim().to_string();
        }
        if let Some(cited) = trimmed.strip_prefix("Evidence:") {
            current.evidence = cited.split(',').filter_map(|c| {
                let (path, line) = c.trim().trim_matches('`').rsplit_once(':')?;
                Some((path.to_string(), line.split('-').next()?.trim().parse().ok()?))
            }).collect();
        }
    }
    for calibration in &mut found {
        calibration.text = calibration.text.trim_end().to_string();
    }
    found
}

/// The code a citation points to, numbered, around the cited line when the file is larger than
/// its share of the state. None when the file or the line does not exist.
fn cited(root: &Path, path: &str, line: usize, room: usize) -> Option<String> {
    let text = std::fs::read_to_string(root.join(path)).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    if line == 0 || line > lines.len() {
        return None;
    }
    let numbered: Vec<String> = lines.iter().enumerate().map(|(i, l)| format!("{:>5} {l}", i + 1)).collect();
    let (mut from, mut to) = (line - 1, line);
    let mut size = js::len(&numbered[from]);
    while from > 0 || to < numbered.len() {
        let mut grew = false;
        for take_before in [true, false] {
            let next = if take_before && from > 0 { Some(from - 1) } else if !take_before && to < numbered.len() { Some(to) } else { None };
            if let Some(i) = next.filter(|i| size + js::len(&numbered[*i]) < room) {
                size += js::len(&numbered[i]) + 1;
                if take_before { from = i } else { to = i + 1 }
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    Some(numbered[from..to].join("\n"))
}

/// How the cited code relates to the recommended answer, the citation check of TypeSafe's
/// cookbook: the recommendation is the claim, the evidence its citations.
/// Structured, because Jev took code that fits an answer about intent for code that shows it.
pub const RELATIONS: [(&str, &str, &str); 3] = [
    ("supports", "The code shows that the whole answer holds, including any part about what is intended or \
        should be.", "Code that is merely consistent with an answer about intent, rules or the future."),
    ("contradicts", "The code shows that the answer is false about how the system works now.",
        "A rule the answer proposes that the code does not follow yet: that is a question of intent."),
    ("says_nothing", "Whether the answer holds turns on intent, a rule or a plan that the code does not \
        state, even when the current code fits it.", "An answer about how the code works now."),
];

/// The order in which the person sees the questions: where the draft and its own evidence
/// disagree first, then what only the person can settle, then what the code already shows.
/// Within each group the architect's order, which is by consequence.
fn group(relation: &str) -> usize {
    match relation {
        "missing" | "contradicts" => 0,
        "says_nothing" | "unchecked" => 1,
        _ => 2,
    }
}

/// For each question: its relation to the code and that answer's probabilities.
pub fn check_calibration(root: &Path, calibration: &Calibration, ask: &dyn Fn(&Value) -> Result<Map<String, Value>>) -> Result<(String, Map<String, Value>)> {
    if calibration.evidence.is_empty() || calibration.recommended.is_empty() {
        return Ok(("unchecked".into(), Map::new()));
    }
    let room = STATE_CHARS / calibration.evidence.len();
    let mut evidence = Vec::new();
    for (path, line) in &calibration.evidence {
        let Some(code) = cited(root, path, *line, room) else { return Ok(("missing".into(), Map::new())) };
        evidence.push(json!({"file": path, "cited_line": line, "code": code}));
    }
    let probabilities = ask(&json!({"claim": {"question": calibration.question, "answer": calibration.recommended}, "evidence": evidence}))?;
    let p = |name: &str| probabilities.get(name).and_then(Value::as_f64).unwrap_or(0.0);
    let top = RELATIONS.iter().map(|(name, ..)| *name).max_by(|a, b| p(a).total_cmp(&p(b))).unwrap_or("says_nothing");
    Ok((top.to_string(), probabilities))
}

/// The questions in the order the person should see them, each with a line saying what the code
/// shows about the recommended answer.
pub fn rank(root: &Path, answer: &str, ask: &dyn Fn(&Value) -> Result<Map<String, Value>>) -> Result<String> {
    let mut checked = Vec::new();
    for (i, calibration) in calibrations(answer).into_iter().enumerate() {
        let (relation, probabilities) = check_calibration(root, &calibration, ask)?;
        checked.push((group(&relation), i, calibration, relation, probabilities));
    }
    checked.sort_by_key(|(group, i, ..)| (*group, *i));
    let blocks: Vec<String> = checked.into_iter().map(|(_, _, calibration, relation, probabilities)| {
        let p = probabilities.get(relation.as_str()).and_then(Value::as_f64);
        let note = match relation.as_str() {
            "missing" => "a cited file or line does not exist".to_string(),
            "unchecked" => "not checked: no recommended answer or no evidence".to_string(),
            name => format!("{} ({:.2})", name.replace('_', " "), p.unwrap_or(0.0)),
        };
        format!("{}\n   Code: {note}", calibration.text)
    }).collect();
    Ok(blocks.join("\n\n"))
}

fn ask_relation(state: &Value) -> Result<Map<String, Value>> {
    let criteria: Map<String, Value> = RELATIONS.iter()
        .map(|(name, what, not_for)| (name.to_string(), json!({"what": js::collapse_space(what), "not_for": js::collapse_space(not_for)})))
        .collect();
    let (answer, _) = ask_choice(state, json!({
        "instructions": "How does the code in `evidence` relate to `claim.answer` as the answer to `claim.question`? \
            Judge only what the code shows; a statement about what was intended is not shown by code that merely fits it.",
        "criteria": criteria,
    }))?;
    Ok(answer.and_then(|a| a.get("probabilities").and_then(Value::as_object).cloned()).unwrap_or_default())
}

/// `sessionkit architect rank [root]`: the architect's calibration questions on stdin, in the
/// order the person should see them on stdout.
pub fn architect_main(argv: &[String]) -> Result<()> {
    let usage = "usage: sessionkit architect rank [root] < questions";
    if argv.first().map(String::as_str) != Some("rank") {
        return Err(usage.into());
    }
    let root = PathBuf::from(argv.get(1).map(String::as_str).unwrap_or("."));
    let mut answer = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut answer).map_err(|e| e.to_string())?;
    if calibrations(&answer).is_empty() {
        return Err(format!("no numbered questions on stdin; {usage}"));
    }
    println!("{}", rank(&root, &answer, &ask_relation)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A classifier that answers what the test says and remembers what it was shown.
    struct Fake {
        answer: Map<String, Value>,
        relations: Map<String, Value>,
        seen: RefCell<Vec<(String, String)>>,
    }

    impl Fake {
        fn new(answer: Value) -> Fake {
            Fake { answer: answer.as_object().cloned().unwrap(), relations: Map::new(), seen: RefCell::new(Vec::new()) }
        }
    }

    impl Classifier for Fake {
        fn name(&self) -> String {
            "fake".into()
        }
        fn classify(&self, change: &Change, memory: &str, decisions: &[Decision]) -> Result<Reading> {
            self.seen.borrow_mut().push((change.file.clone(), memory.to_string()));
            let decisions = decisions.iter().map(|d| (d.file.clone(), self.relations.get(&d.file).cloned()
                .unwrap_or(json!({"unrelated": 1.0})))).collect();
            Ok(Reading { categories: self.answer.clone(), decisions })
        }
    }

    const MEMORY_TEXT: &str = "<!-- sessionkit architect: baseline abc123 2026-10-03 -->\n# Architecture\n\n\
        ## Ownership\n- Billing owns invoices and their persistence. [confirmed]\n\n\
        ## Known debt\n- AD-001 Catalog and Pricing import each other. Policy: warn only when a change adds to it.\n";

    /// A repository with the memory at its root.
    fn repository(memory: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sessionkit-architect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src/checkout")).unwrap();
        std::fs::write(root.join(MEMORY), memory).unwrap();
        root
    }

    fn edit(root: &Path, file: &str, change: &str) -> Value {
        json!({"session_id": "s1", "cwd": root, "kind": "edit", "file": root.join(file), "change": change})
    }

    #[test]
    fn a_rename_inside_one_module_does_not_wake_the_architect() {
        let root = repository(MEMORY_TEXT);
        let fake = Fake::new(json!({"local": 0.93, "boundary": 0.03, "structure": 0.02, "known_debt": 0.02}));
        let answer = check(&edit(&root, "src/checkout/total.rs", "-    let t = sum(items);\n+    let total = sum(items);"), Some(&fake), WAKE_AT).unwrap();
        assert_eq!(answer["wake"], false);
        assert_eq!(answer["category"], "local");
        assert_eq!(answer["against_text"], json!({}));
    }

    #[test]
    fn a_new_dependency_on_billing_persistence_wakes_it_with_the_change_and_the_memory() {
        let root = repository(MEMORY_TEXT);
        let fake = Fake::new(json!({"local": 0.1, "boundary": 0.72, "structure": 0.08, "duplicate": 0.1}));
        let change = "+use crate::billing::persistence::InvoiceRepository;";
        let answer = check(&edit(&root, "src/checkout/complete.rs", change), Some(&fake), WAKE_AT).unwrap();
        assert_eq!(answer["wake"], true);
        assert_eq!(answer["category"], "boundary");
        let seen = fake.seen.borrow();
        assert_eq!(seen[0].0, "src/checkout/complete.rs");
        assert!(seen[0].1.contains("Billing owns invoices"));
        assert_eq!(answer["file"], "src/checkout/complete.rs");
    }

    #[test]
    fn known_debt_and_deliberate_exceptions_do_not_count_as_relevance() {
        let (sum, top) = relevance(json!({"known_debt": 0.6, "exception": 0.3, "local": 0.1}).as_object().unwrap());
        assert!(sum < 0.01);
        assert_eq!(top, "known_debt");
        let (sum, top) = relevance(json!({"known_debt": 0.3, "expands_debt": 0.65, "local": 0.05}).as_object().unwrap());
        assert!((sum - 0.65).abs() < 1e-9);
        assert_eq!(top, "expands_debt");
    }

    #[test]
    fn without_onboarding_nothing_is_asked() {
        let root = repository("# Architecture\n\nWritten by hand, not by onboarding.\n");
        let fake = Fake::new(json!({"boundary": 1.0}));
        let answer = check(&edit(&root, "src/checkout/complete.rs", "+use billing;"), Some(&fake), WAKE_AT).unwrap();
        assert_eq!(answer["reason"], "no architecture memory");
        assert!(fake.seen.borrow().is_empty());
    }

    #[test]
    fn edits_of_the_memory_and_the_decision_records_are_not_checked() {
        let root = repository(MEMORY_TEXT);
        let fake = Fake::new(json!({"boundary": 1.0}));
        for file in [MEMORY, "docs/adr/0007-billing-owns-invoices.md"] {
            let answer = check(&edit(&root, file, "+ new decision"), Some(&fake), WAKE_AT).unwrap();
            assert_eq!(answer["reason"], "architecture memory", "{file}");
        }
        assert!(fake.seen.borrow().is_empty());
    }

    #[test]
    fn a_plan_is_found_from_the_working_folder() {
        let root = repository(MEMORY_TEXT);
        let fake = Fake::new(json!({"structure": 0.8, "local": 0.2}));
        let input = json!({"session_id": "s1", "cwd": root.join("src/checkout"), "kind": "plan", "file": "",
            "change": "Add a SavedAddress table owned by Checkout."});
        let answer = check(&input, Some(&fake), WAKE_AT).unwrap();
        assert_eq!(answer["wake"], true);
        assert_eq!(fake.seen.borrow()[0].0, "");
    }

    #[test]
    fn the_change_and_the_memory_share_the_state() {
        assert_eq!(shares(100, 200, 1000), (100, 200));
        assert_eq!(shares(100, 5000, 1000), (100, 900));
        assert_eq!(shares(5000, 300, 1000), (700, 300));
        assert_eq!(shares(5000, 5000, 1000), (500, 500));
    }

    #[test]
    fn a_change_against_a_recorded_decision_carries_that_record_s_text() {
        let root = repository(MEMORY_TEXT);
        std::fs::create_dir_all(root.join(DECISIONS)).unwrap();
        std::fs::write(root.join("docs/adr/0003-invoice-numbers.md"), "# Invoice numbers are gapless\n\nOnly billing assigns them.\n").unwrap();
        std::fs::write(root.join("docs/adr/0001-one-crate.md"), "# One crate\n").unwrap();
        let mut fake = Fake::new(json!({"local": 0.8, "boundary": 0.2}));
        fake.relations.insert("docs/adr/0003-invoice-numbers.md".into(), json!({"goes_against": 0.7, "unrelated": 0.3}));
        let answer = check(&edit(&root, "src/checkout/complete.rs", "+let number = format!(\"INV-{}\", order.id);"), Some(&fake), WAKE_AT).unwrap();
        assert_eq!(answer["wake"], true);
        assert_eq!(answer["against"], json!(["docs/adr/0003-invoice-numbers.md"]));
        assert_eq!(answer["decisions"]["docs/adr/0001-one-crate.md"]["unrelated"], 1.0);
        assert_eq!(answer["against_text"], json!({"docs/adr/0003-invoice-numbers.md": "# Invoice numbers are gapless\n\nOnly billing assigns them."}));
    }

    #[test]
    fn decision_records_share_the_state_with_the_memory() {
        let decisions = [Decision { file: "docs/adr/0001.md".into(), text: "d".repeat(100) }];
        let change = Change { kind: "edit".into(), file: "a.rs".into(), text: "+x".into() };
        let state = state(&change, "memory", &decisions);
        assert_eq!(state["decisions"][0]["file"], "docs/adr/0001.md");
        assert_eq!(state["decisions"][0]["text"].as_str().unwrap().len(), 100);
    }

    #[test]
    fn the_threshold_decides() {
        let root = repository(MEMORY_TEXT);
        let fake = Fake::new(json!({"boundary": 0.5, "local": 0.5}));
        let input = edit(&root, "src/checkout/complete.rs", "+use billing;");
        assert_eq!(check(&input, Some(&fake), 0.6).unwrap()["wake"], false);
        assert_eq!(check(&input, Some(&fake), 0.4).unwrap()["wake"], true);
    }

    #[cfg(unix)]
    #[test]
    fn a_command_can_take_jevs_place() {
        use std::os::unix::fs::PermissionsExt;
        let root = repository(MEMORY_TEXT);
        let script = root.join("classify.sh");
        std::fs::write(&script, "#!/bin/sh\ncat > /dev/null\necho '{\"probabilities\": {\"structure\": 0.9, \"local\": 0.1}}'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let command = Command(script.to_string_lossy().to_string());
        let answer = check(&edit(&root, "src/checkout/complete.rs", "+mod saved_addresses;"), Some(&command), WAKE_AT).unwrap();
        assert_eq!(answer["wake"], true);
        assert_eq!(answer["category"], "structure");
    }

    const QUESTIONS: &str = "Draft written.\n\n\
1. Who owns invoices?\n   Recommended: billing  (confidence 0.8)\n   Evidence: `src/billing.rs:2`\n   Options: billing; checkout\n\n\
2. Are the two Customer types separate on purpose?\n   Recommended: yes  (confidence 0.6)\n   Evidence: `src/billing.rs:1`, `src/accounts.rs:1`\n   Options: yes; no\n\n\
3. Is checkout the orchestrator?\n   Recommended: yes  (confidence 0.6)\n   Evidence: `src/gone.rs:9`\n   Options: yes; no\n";

    fn shop() -> PathBuf {
        let root = std::env::temp_dir().join(format!("sessionkit-calibration-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/billing.rs"), "pub struct Customer;\npub struct InvoiceRepository;\n").unwrap();
        std::fs::write(root.join("src/accounts.rs"), "pub struct Customer;\n").unwrap();
        root
    }

    #[test]
    fn calibration_questions_are_read_from_the_architects_answer() {
        let found = calibrations(QUESTIONS);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].question, "Who owns invoices?");
        assert_eq!(found[0].recommended, "billing");
        assert_eq!(found[1].evidence, [("src/billing.rs".to_string(), 1), ("src/accounts.rs".to_string(), 1)]);
        assert!(found[2].text.ends_with("Options: yes; no"));
    }

    #[test]
    fn what_only_the_person_can_settle_comes_before_what_the_code_shows_and_missing_evidence_first() {
        let root = shop();
        let seen = RefCell::new(Vec::new());
        let ask = |state: &Value| -> Result<Map<String, Value>> {
            seen.borrow_mut().push(state.clone());
            let answer = if state["claim"]["question"] == "Who owns invoices?" { json!({"supports": 0.95, "says_nothing": 0.05}) } else { json!({"says_nothing": 0.8, "supports": 0.2}) };
            Ok(answer.as_object().cloned().unwrap())
        };
        let ranked = rank(&root, QUESTIONS, &ask).unwrap();
        let order: Vec<&str> = ranked.split("\n\n").map(|b| b.lines().next().unwrap()).collect();
        assert_eq!(order, ["3. Is checkout the orchestrator?", "2. Are the two Customer types separate on purpose?", "1. Who owns invoices?"]);
        assert!(ranked.contains("Code: a cited file or line does not exist"));
        assert!(ranked.contains("Code: says nothing (0.80)"));
        assert!(ranked.contains("Code: supports (0.95)"));
        let states = seen.borrow();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0]["evidence"][0]["code"], "    1 pub struct Customer;\n    2 pub struct InvoiceRepository;");
    }

    #[test]
    fn a_large_file_is_cited_around_the_line() {
        let root = shop();
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("src/big.rs"), text).unwrap();
        let code = cited(&root, "src/big.rs", 50, 60).unwrap();
        assert!(code.contains("   50 line 50"));
        assert!(!code.contains("    1 line 1\n"));
        assert!(js::len(&code) <= 60);
        assert_eq!(cited(&root, "src/big.rs", 101, 60), None);
    }

    fn git_repository() -> PathBuf {
        let root = std::env::temp_dir().join(format!("sessionkit-offer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::process::Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn a_repository_without_an_architect_is_offered_one_until_the_person_declines() {
        let root = git_repository();
        let declined_in = root.join("declined.json");
        let input = json!({"cwd": root.join("src")});
        let answer = offer(&input, &declined_in);
        assert_eq!(answer["ask"], true);
        assert!(answer["question"].as_str().unwrap().contains("has no architect yet"));
        decline(&json!({"root": answer["root"]}), &declined_in).unwrap();
        decline(&json!({"root": answer["root"]}), &declined_in).unwrap();
        assert_eq!(declined(&declined_in).len(), 1);
        assert_eq!(offer(&input, &declined_in)["reason"], "declined");
    }

    #[test]
    fn an_onboarded_repository_or_a_folder_outside_git_is_not_asked() {
        let root = git_repository();
        let declined_in = root.join("declined.json");
        std::fs::write(root.join(MEMORY), "# Architecture, by hand\n").unwrap();
        assert!(offer(&json!({"cwd": root}), &declined_in)["question"].as_str().unwrap().contains("has an ARCHITECTURE.md"));
        std::fs::write(root.join(MEMORY), MEMORY_TEXT).unwrap();
        assert_eq!(offer(&json!({"cwd": root}), &declined_in)["reason"], "onboarded");
        let outside = std::env::temp_dir().join(format!("sessionkit-nogit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(offer(&json!({"cwd": outside}), &declined_in)["ask"], false);
    }
}
