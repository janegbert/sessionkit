// sessionkit measure — measure whether sessionkit start selects the right turns and routes to the
// right session.
//
// Labels come from code, so a run needs no hand work and can be repeated:
//   A. Turn selection. Cut a session at turn k; the real next prompt is the new task. Plant
//      five turns with a known label at random places that are not forced: the real turn after
//      the prompt (still applies), the first turn of a session in another project (does not),
//      a stated rule and a plain task (rule or not), and one conclusion with an open block and
//      a done block. Measure how often the right plant scores above the wrong one, and how
//      often each passes its threshold. The real turns give the distribution next to it. A
//      second run with a prompt from another project must lower the scores.
//      (Labels from file overlap and shared terms were tried first: most turns change no file,
//      and shared terms scored at chance, so neither was a usable label.)
//   B. Routing. A later prompt of a session, with the cut session in the pool, must route to
//      that session. The first prompt of a session, with that session out of the pool, must
//      route to a new topic.
//
// Results hold ids, turn numbers, scores and labels, never transcript text.
//
// A run costs money. It first runs dry, without network, to estimate the cost, and asks
// before it spends anything. --yes skips the question; --estimate stops after the estimate.
//
// Usage: sessionkit measure [--cuts 30] [--routes 20] [--seed 1] [--yes] [--estimate]

use crate::Result;
use crate::js::{self, num};
use crate::jev::{ask_nouls, start_estimate, stop_estimate};
use crate::start::{EXCERPT_FLOOR, Exchange, NEW_TOPIC, ROUTE_ASK, ROUTE_CANDIDATES, ROUTE_CONTINUE, SECTION_FLOOR, SESSION_TAIL, SHORTLIST_FLOOR, Scored, Session, continue_session, read_session, relevance_question, route_to_session, session_card, session_files};
use crate::usage::home;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::Arc;

const POOL_SIZE: usize = 150;
const MIN_TURNS: usize = 6;
const MIN_PROMPT_CHARS: usize = 40;
const CONCURRENCY: usize = 6;
const REQUEST_BUDGET_CHARS: usize = 100000;

/// A small seeded generator, so a run can be repeated with the same sample. The arithmetic is in
/// doubles, as in the JavaScript version, so a seed draws the same sample in both.
pub struct Random {
    pub seed: f64,
}

impl Random {
    pub fn next(&mut self) -> f64 {
        self.seed = (self.seed * 1103515245.0 + 12345.0) % 2147483648.0;
        self.seed / 2147483648.0
    }

    pub fn index(&mut self, count: usize) -> usize {
        (self.next() * count as f64).floor() as usize
    }

    fn shuffle<T: Clone>(&mut self, items: &[T]) -> Vec<T> {
        let mut keyed: Vec<(f64, T)> = items.iter().map(|item| (self.next(), item.clone())).collect();
        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        keyed.into_iter().map(|(_, item)| item).collect()
    }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted.get(sorted.len() / 2).copied().unwrap_or(f64::NAN)
}

fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[(sorted.len() - 1).min((q * sorted.len() as f64).floor() as usize)]
}

fn spread(values: &[f64]) -> Value {
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    json!({"n": values.len(), "median": num(median(values)), "p90": num(percentile(values, 0.9)), "max": num(max)})
}

fn share(flags: &[bool]) -> Value {
    if flags.is_empty() { Value::Null } else { num(flags.iter().filter(|&&flag| flag).count() as f64 / flags.len() as f64) }
}

/// A session as it was at turn k: the earlier turns only, and a summary only if it came earlier.
fn cut_session(session: &Session, k: usize) -> Session {
    let exchanges = session.exchanges[..k.min(session.exchanges.len())].to_vec();
    let mut changed_files = IndexMap::new();
    for exchange in &exchanges {
        for file in &exchange.files {
            changed_files.insert(file.clone(), exchange.timestamp.clone());
        }
    }
    let end = exchanges.last().map_or_else(|| session.start.clone(), |e| e.timestamp.clone());
    let summary = session.summary.clone().filter(|summary| summary.timestamp < end);
    Session {
        id: format!("{}@{k}", session.id), exchanges, changed_files, commands: Vec::new(), summary, end,
        cwd: String::new(), ..session.clone()
    }
}

fn plant(prompt: &str, answer: &str, timestamp: &str) -> Exchange {
    Exchange { prompt: prompt.into(), answer: answer.into(), conclusion: answer.into(), timestamp: timestamp.into(), files: Vec::new(), tool_calls: 0, commands: Vec::new(), automated: false, plant: None }
}

const RULE_PLANT: (&str, &str) = (
    "Vanaf nu: schrijf alle commitberichten in het Engels en gebruik nooit emoji.",
    "Begrepen. Ik schrijf commitberichten voortaan in het Engels, zonder emoji.",
);
const TASK_PLANT: (&str, &str) = ("Draai de tests nog een keer.", "Ik heb de tests gedraaid. Ze slagen allemaal.");
const OPEN_BLOCK: &str = "Nog open: de migratie voor de nieuwe tabel is nog niet gedraaid. Dat moet jij eerst bevestigen.";
const DONE_BLOCK: &str = "De vertaalbestanden zijn bijgewerkt en de build slaagt.";
const PLANTS: [&str; 5] = ["related", "unrelated", "rule", "task", "open"];

/// A cut with five planted turns at random free places: after the first turn and before the
/// last SESSION_TAIL turns, which are forced. `draws` holds one random number per plant. Returns
/// the session and where each plant landed.
fn planted_cut(session: &Session, k: usize, stranger: &Session, draws: &[f64; 5]) -> (Session, IndexMap<&'static str, usize>) {
    let mut exchanges = session.exchanges[..k].to_vec();
    let timestamp = exchanges.last().map(|e| e.timestamp.clone()).unwrap_or_default();
    let related = Exchange { files: Vec::new(), tool_calls: 0, ..session.exchanges[k + 1].clone() };
    let unrelated = Exchange { timestamp: timestamp.clone(), files: Vec::new(), tool_calls: 0, ..stranger.exchanges[0].clone() };
    let plants = [
        related,
        unrelated,
        plant(RULE_PLANT.0, RULE_PLANT.1, &timestamp),
        plant(TASK_PLANT.0, TASK_PLANT.1, &timestamp),
        plant("Hoe staat het ervoor?", &format!("{DONE_BLOCK}\n\n{OPEN_BLOCK}"), &timestamp),
    ];
    for ((name, turn), draw) in PLANTS.iter().zip(plants).zip(draws) {
        let position = 1 + (draw * (exchanges.len() - SESSION_TAIL) as f64).floor() as usize;
        exchanges.insert(position, Exchange { plant: Some(name), ..turn });
    }
    let count = exchanges.len();
    let cut = cut_session(&Session { exchanges, ..session.clone() }, count);
    let at = cut.exchanges.iter().enumerate().filter_map(|(i, e)| e.plant.map(|name| (name, i))).collect();
    (cut, at)
}

struct Cut {
    session: Arc<Session>,
    k: usize,
    stranger: Arc<Session>,
    draws: [f64; 5],
    foreign: String,
}

fn measure_turns(sessions: &[Arc<Session>], cuts_wanted: usize, random: &mut Random) -> Result<(Vec<Value>, f64, Value)> {
    let eligible: Vec<Arc<Session>> = sessions.iter().filter(|s| s.exchanges.len() >= MIN_TURNS).cloned().collect();
    let mut cuts = Vec::new();
    for session in random.shuffle(&eligible) {
        let k = (session.exchanges.len() as f64 * (0.5 + random.next() * 0.3)).floor() as usize;
        let (Some(next), Some(_)) = (session.exchanges.get(k), session.exchanges.get(k + 1)) else { continue };
        if k < 4 || js::len(&next.prompt) < MIN_PROMPT_CHARS || next.prompt.starts_with('<') {
            continue;
        }
        let strangers: Vec<&Arc<Session>> = sessions.iter().filter(|o| o.cwd != session.cwd && !o.exchanges[0].answer.is_empty()).collect();
        let pick = random.index(strangers.len());
        let Some(stranger) = strangers.get(pick).map(|s| (*s).clone()) else { return Err("No session from another project to plant.".into()) };
        cuts.push((session, k, stranger));
        if cuts.len() >= cuts_wanted {
            break;
        }
    }
    eprint!("turns: {} sessions eligible, {} cuts\n", eligible.len(), cuts.len());
    if cuts.is_empty() {
        return Err("No cuts: nothing to measure.".into());
    }
    // The JavaScript version drew these numbers as each cut started, in an order that depended on
    // which request finished first; here they are drawn in the order of the cuts.
    let cuts: Vec<Cut> = cuts.into_iter().map(|(session, k, stranger)| {
        let draws = [random.next(), random.next(), random.next(), random.next(), random.next()];
        let others: Vec<&Arc<Session>> = sessions.iter().filter(|o| o.cwd != session.cwd).collect();
        let foreign = others.get(random.index(others.len())).map(|o| o.exchanges[0].prompt.clone()).unwrap_or_default();
        Cut { session, k, stranger, draws, foreign }
    }).collect();

    let rows: Vec<Result<(Value, f64, Rows)>> = js::pool(&cuts, CONCURRENCY, |cut, _| {
        let (session_cut, at) = planted_cut(&cut.session, cut.k, &cut.stranger, &cut.draws);
        let session_cut = Arc::new(session_cut);
        let prompt = &cut.session.exchanges[cut.k].prompt;
        let real = continue_session(prompt, session_cut.clone(), false, None)?;
        let away = continue_session(&cut.foreign, session_cut.clone(), false, None)?;
        let (applies, rules, open, blocks) = (&real.scores.applies, &real.scores.rules, &real.scores.open, &real.scores.blocks);
        let block_score = |text: &str| blocks.iter().position(|b| b.turn == at["open"] + 1 && b.text == text).map(|i| open[i]);
        let plant_indexes: Vec<usize> = at.values().copied().collect();
        let count = session_cut.exchanges.len();
        let real_turns: Vec<usize> = (0..count).filter(|i| !plant_indexes.contains(i)).collect();
        let free = |i: &&usize| **i > 0 && **i + SESSION_TAIL < count;
        let rows = Rows {
            applies: (applies[at["related"]], applies[at["unrelated"]]),
            applies_away: away.scores.applies[at["related"]],
            rule: (rules[at["rule"]], rules[at["task"]]),
            open: (block_score(OPEN_BLOCK), block_score(DONE_BLOCK)),
            real_applies: real_turns.iter().filter(free).map(|&i| applies[i]).collect(),
            real_away: real_turns.iter().filter(free).map(|&i| away.scores.applies[i]).collect(),
            real_rules: real_turns.iter().map(|&i| rules[i]).collect(),
            real_open: blocks.iter().enumerate().filter(|(_, b)| b.turn - 1 != at["open"]).map(|(i, _)| open[i]).collect(),
        };
        let row = json!({
            "session": cut.session.id, "cut": cut.k, "turns": count,
            "applies": {"related": num(rows.applies.0), "unrelated": num(rows.applies.1)},
            "applies_away": {"related": num(rows.applies_away), "unrelated": num(away.scores.applies[at["unrelated"]])},
            "rule": {"rule": num(rows.rule.0), "task": num(rows.rule.1)},
            "open": {"open": js::opt(rows.open.0), "done": js::opt(rows.open.1)},
            "real": {"applies": rows.real_applies, "applies_away": rows.real_away, "rules": rows.real_rules, "open": rows.real_open},
        });
        Ok((row, real.tokens + away.tokens, rows))
    });
    let mut tokens = 0.0;
    let (mut json_rows, mut all) = (Vec::new(), Vec::new());
    for row in rows {
        let (value, used, parts) = row?;
        tokens += used;
        json_rows.push(value);
        all.push(parts);
    }

    let wins = |pairs: &[(f64, f64)]| share(&pairs.iter().map(|(g, b)| g > b).collect::<Vec<_>>());
    let applies_pairs: Vec<(f64, f64)> = all.iter().map(|r| r.applies).collect();
    let rule_pairs: Vec<(f64, f64)> = all.iter().map(|r| r.rule).collect();
    let open_pairs: Vec<(f64, f64)> = all.iter().filter_map(|r| Some((r.open.0?, r.open.1?))).collect();
    let real_applies: Vec<f64> = all.iter().flat_map(|r| r.real_applies.clone()).collect();
    let real_away: Vec<f64> = all.iter().flat_map(|r| r.real_away.clone()).collect();
    let real_rules: Vec<f64> = all.iter().flat_map(|r| r.real_rules.clone()).collect();
    let real_open: Vec<f64> = all.iter().flat_map(|r| r.real_open.clone()).collect();
    let at_floor = |values: &[f64], floor: f64| share(&values.iter().map(|&v| v >= floor).collect::<Vec<_>>());
    let column = |pick: fn(&Rows) -> f64| all.iter().map(pick).collect::<Vec<f64>>();
    let summary = json!({
        "cuts": all.len(),
        "applies": {
            "related_beats_unrelated": wins(&applies_pairs),
            "related_at_floor": at_floor(&column(|r| r.applies.0), EXCERPT_FLOOR),
            "unrelated_at_floor": at_floor(&column(|r| r.applies.1), EXCERPT_FLOOR),
            "related": spread(&column(|r| r.applies.0)),
            "unrelated": spread(&column(|r| r.applies.1)),
            "real_turns": spread(&real_applies),
            "real_kept_at_floor": at_floor(&real_applies, EXCERPT_FLOOR),
            "foreign_prompt_median_move": num(median(&real_away.iter().zip(&real_applies).map(|(a, r)| a - r).collect::<Vec<_>>())),
            "related_with_foreign_prompt": spread(&column(|r| r.applies_away)),
        },
        "rules": {
            "rule_beats_task": wins(&rule_pairs),
            "rule_at_floor": at_floor(&column(|r| r.rule.0), SECTION_FLOOR),
            "task_at_floor": at_floor(&column(|r| r.rule.1), SECTION_FLOOR),
            "rule": spread(&column(|r| r.rule.0)),
            "task": spread(&column(|r| r.rule.1)),
            "real_turns": spread(&real_rules),
            "real_at_floor": at_floor(&real_rules, SECTION_FLOOR),
        },
        "open": {
            "open_beats_done": wins(&open_pairs),
            "open_at_floor": at_floor(&open_pairs.iter().map(|p| p.0).collect::<Vec<_>>(), SECTION_FLOOR),
            "done_at_floor": at_floor(&open_pairs.iter().map(|p| p.1).collect::<Vec<_>>(), SECTION_FLOOR),
            "real_blocks": spread(&real_open),
            "real_at_floor": at_floor(&real_open, SECTION_FLOOR),
        },
    });
    Ok((json_rows, tokens, summary))
}

struct Rows {
    applies: (f64, f64),
    applies_away: f64,
    rule: (f64, f64),
    open: (Option<f64>, Option<f64>),
    real_applies: Vec<f64>,
    real_away: Vec<f64>,
    real_rules: Vec<f64>,
    real_open: Vec<f64>,
}

fn first_pass(prompt: &str, pooled: &[Arc<Session>]) -> Result<(Vec<Scored>, f64)> {
    let questions: Vec<Value> = pooled.iter().map(|session| relevance_question(session_card(session))).collect();
    let (probabilities, tokens) = ask_nouls(&json!({"new_task": prompt}), &questions, REQUEST_BUDGET_CHARS)?;
    let mut scored: Vec<Scored> = pooled.iter().zip(probabilities)
        .map(|(session, relevance)| Scored { session: session.clone(), relevance, continues: None }).collect();
    scored.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
    Ok((scored, tokens))
}

struct RouteItem {
    kind: &'static str,
    session: Arc<Session>,
    prompt: String,
    pool: Vec<Arc<Session>>,
    expected: String,
}

fn measure_routes(sessions: &[Arc<Session>], routes: usize, random: &mut Random) -> Result<(Vec<Value>, f64, Value)> {
    let eligible = random.shuffle(&sessions.iter().filter(|s| s.exchanges.len() >= 4).cloned().collect::<Vec<_>>());
    let mut items = Vec::new();
    let usable = |prompt: &str| js::len(prompt) >= MIN_PROMPT_CHARS && !prompt.starts_with('<');
    for session in eligible {
        if items.len() >= routes * 2 {
            break;
        }
        let k = 2 + random.index(session.exchanges.len() - 2);
        let later = session.exchanges[k].prompt.clone();
        let first = session.exchanges[0].prompt.clone();
        let others: Vec<Arc<Session>> = sessions.iter().filter(|s| !Arc::ptr_eq(s, &session)).cloned().collect();
        if usable(&later) {
            let mut pool = others.clone();
            pool.push(Arc::new(cut_session(&session, k)));
            items.push(RouteItem { kind: "continue", session: session.clone(), prompt: later, pool, expected: format!("{}@{k}", session.id) });
        }
        if usable(&first) {
            items.push(RouteItem { kind: "new", session: session.clone(), prompt: first, pool: others, expected: NEW_TOPIC.into() });
        }
    }
    eprint!("routes: {} items\n", items.len());
    if items.is_empty() {
        return Err("No routing items: nothing to measure.".into());
    }

    let results = js::pool(&items, CONCURRENCY, |item, _| -> Result<(Value, f64)> {
        let (scored, first_tokens) = first_pass(&item.prompt, &item.pool)?;
        let candidates: Vec<Scored> = scored.into_iter().filter(|c| c.relevance >= SHORTLIST_FLOOR).take(ROUTE_CANDIDATES).collect();
        let (session, probability, route_tokens) = if candidates.is_empty() {
            (None, 1.0, 0.0)
        } else {
            let route = route_to_session(&item.prompt, &candidates, false)?;
            (route.session, route.probability, route.tokens)
        };
        let answer = session.as_ref().map_or_else(|| NEW_TOPIC.to_string(), |s| s.id.clone());
        let action = match &session {
            None => "search",
            Some(_) if probability >= ROUTE_CONTINUE => "continue",
            Some(_) if probability >= ROUTE_ASK => "ask",
            Some(_) => "search",
        };
        let row = json!({
            "kind": item.kind, "session": item.session.id, "expected": item.expected, "answer": answer,
            "probability": num(probability), "action": action,
            "expected_in_candidates": item.kind == "new" || candidates.iter().any(|c| c.session.id == item.expected),
        });
        Ok((row, first_tokens + route_tokens))
    });
    let mut tokens = 0.0;
    let mut rows = Vec::new();
    for result in results {
        let (row, used) = result?;
        tokens += used;
        rows.push(row);
    }

    let summarize = |kind: &str| {
        let set: Vec<&Value> = rows.iter().filter(|row| row["kind"] == kind).collect();
        let right = |row: &&&Value| row["answer"] == row["expected"];
        let count_action = |action: &str| set.iter().filter(|row| row["action"] == action).count();
        json!({
            "items": set.len(),
            "right_answer": set.iter().filter(right).count(),
            "actions": {"continue": count_action("continue"), "ask": count_action("ask"), "search": count_action("search")},
            "auto_continue_right": set.iter().filter(|row| row["action"] == "continue" && row["answer"] == row["expected"]).count(),
            "auto_continue_wrong": set.iter().filter(|row| row["action"] == "continue" && row["answer"] != row["expected"]).count(),
            "expected_in_candidates": set.iter().filter(|row| row["expected_in_candidates"] == true).count(),
        })
    };
    let summary = json!({"continue": summarize("continue"), "new": summarize("new")});
    Ok((rows, tokens, summary))
}

struct Measured {
    turns: (Vec<Value>, f64, Value),
    routes: (Vec<Value>, f64, Value),
}

fn measure_all(sessions: &[Arc<Session>], cuts: usize, routes: usize, random: &mut Random) -> Result<Measured> {
    let turns = measure_turns(sessions, cuts, random)?;
    let routes = if routes > 0 { measure_routes(sessions, routes, random)? } else { (Vec::new(), 0.0, Value::Null) };
    Ok(Measured { turns, routes })
}

pub fn measure_main(argv: &[String]) -> Result<()> {
    let argument = |name: &str, fallback: f64| {
        argv.iter().position(|arg| *arg == format!("--{name}"))
            .map_or(fallback, |i| js::parse_number(argv.get(i + 1).map(String::as_str)))
    };
    let flag = |name: &str| argv.iter().any(|arg| *arg == format!("--{name}"));
    let count = |value: f64| if value.is_nan() || value < 0.0 { 0 } else { value as usize };
    let (cuts, routes, seed) = (count(argument("cuts", 30.0)), count(argument("routes", 20.0)), argument("seed", 1.0));

    let files = session_files(true, POOL_SIZE)?;
    let loaded: Result<Vec<Arc<Session>>> = js::pool(&files, CONCURRENCY, |path, _| read_session(path)).into_iter().collect();
    let sessions: Vec<Arc<Session>> = loaded?.into_iter().filter(|s| !s.exchanges.is_empty()).collect();
    eprint!("loaded {} sessions from {} files\n", sessions.len(), files.len());
    if sessions.is_empty() {
        return Err("No sessions loaded.".into());
    }

    // The dry run draws the same sample as the real one: it starts from the same seed.
    start_estimate();
    let counted = measure_all(&sessions, cuts, routes, &mut Random { seed });
    let cost = stop_estimate();
    counted?;
    eprint!("estimate: {} requests, {} characters, at most {} tokens, at most ${}\n",
        cost.requests, js::grouped(cost.chars as f64), js::grouped(cost.tokens), js::fixed(cost.cost_usd, 2));
    if flag("estimate") {
        return Ok(());
    }
    let confirmed = flag("yes") || (js::stdin_is_tty()
        && js::ask(&format!("Spend at most ${} on this run? [y/N] ", js::fixed(cost.cost_usd, 2))).to_lowercase().starts_with('y'));
    if !confirmed {
        eprint!("sessionkit measure: stopped before spending anything.\n");
        std::process::exit(2);
    }

    let Measured { turns, routes } = measure_all(&sessions, cuts, routes, &mut Random { seed })?;
    let tokens = turns.1 + routes.1;
    let run = js::iso(js::now());
    let cost_usd = (tokens / 1e6) * 0.042;
    let result = json!({
        "run": run,
        "seed": num(seed),
        "thresholds": {"EXCERPT_FLOOR": EXCERPT_FLOOR, "ROUTE_ASK": ROUTE_ASK, "ROUTE_CONTINUE": ROUTE_CONTINUE,
            "SECTION_FLOOR": SECTION_FLOOR, "SHORTLIST_FLOOR": SHORTLIST_FLOOR},
        "tokens": num(tokens),
        "cost_usd": num(cost_usd),
        "turns": turns.2,
        "routes": routes.2,
        "rows": {"turns": turns.0, "routes": routes.0},
    });
    // The JavaScript version wrote next to its script; an installed binary has no such place.
    let dir = home().join(".cache").join("sessionkit").join("measure");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}.json", run.replace([':', '.'], "-")));
    std::fs::write(&file, js::stringify_pretty(&result)).map_err(|e| e.to_string())?;
    println!("{}", js::stringify_pretty(&json!({
        "turns": result["turns"], "routes": result["routes"], "tokens": num(tokens), "cost_usd": num(cost_usd),
        "file": file.to_string_lossy(),
    })));
    Ok(())
}
