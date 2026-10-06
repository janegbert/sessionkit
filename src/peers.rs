// sessionkit peers — the Claude Code sessions running on this machine, and whether one of them
// already has the context a task needs.
//
// Claude Code keeps one file per running session in ~/.claude/sessions: its name, folder and
// whether it is busy or idle. With the cache state of its transcript that shows which sessions
// are idle while their prompt cache is still warm. Given a task, Jev judges per warm, idle
// session whether its work so far covers what the task needs. A session that fits can take the
// task through SendMessage, instead of a fresh subagent that first has to read its way in.
//
// The same judgment runs in shadow for every subagent (see claude.rs, agent_route): it is logged,
// nothing changes the spawn.

use crate::cache::{CacheState, cache_state};
use crate::claude::{minutes, tokens, usd};
use crate::js;
use crate::next::transcript_of;
use crate::auth::key_at_hand;
use crate::jev::{PRICE_PER_MILLION_TOKENS, ask_nouls};
use crate::start::{read_session, session_card};
use crate::usage::home;
use serde_json::{Value, json};
use std::path::PathBuf;

const USAGE: &str = "Usage: sessionkit peers [options] [\"task\"]

Lists the Claude Code sessions running on this machine: status, context, what one call costs in
cache reads, and how long the cache stays warm. Given a task, Jev judges per idle, warm session
whether its work so far covers what the task needs (costs money).

Options:
  --all        sessions in every folder, not only this one
  -h, --help   show this help";

/// Unmeasured: the same line as the route of sessionkit start, until the shadow log says more.
pub const FIT: f64 = 0.70;
const REQUEST_BUDGET_CHARS: usize = 100000;

pub struct Peer {
    pub name: String,
    pub session_id: String,
    pub cwd: String,
    pub status: String,
    pub transcript: Option<PathBuf>,
    pub cache: Option<CacheState>,
}

impl Peer {
    pub fn available(&self) -> bool {
        self.status == "idle" && self.cache.as_ref().is_some_and(|c| c.warm)
    }
}

fn sessions_dir() -> PathBuf {
    home().join(".claude").join("sessions")
}

fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
}

/// One entry of ~/.claude/sessions, when it describes a session.
fn entry_of(entry: &Value) -> Option<(i64, String, String, String, String)> {
    let id = js::str_of(entry, "sessionId");
    if id.is_empty() {
        return None;
    }
    let name = js::str_of(entry, "name");
    Some((entry.get("pid").and_then(Value::as_i64).unwrap_or(0), id.to_string(),
        (if name.is_empty() { js::head(id, 8) } else { name }).to_string(), js::str_of(entry, "cwd").to_string(),
        js::str_of(entry, "status").to_string()))
}

/// Whether two session folders are the same project: equal, or one inside the other.
pub fn same_folder(a: &str, b: &str) -> bool {
    let inside = |outer: &str, inner: &str| inner.strip_prefix(outer).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
    !a.is_empty() && !b.is_empty() && (inside(a, b) || inside(b, a))
}

/// The running sessions, without `own`, busy ones last.
pub fn running(own: &str) -> Vec<Peer> {
    let dir = sessions_dir();
    let mut peers: Vec<Peer> = js::read_dir_sorted(&dir).into_iter()
        .filter(|file| file.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|file| js::parse(&std::fs::read_to_string(file.path()).ok()?))
        .filter_map(|entry| entry_of(&entry))
        .filter(|(pid, id, ..)| id != own && alive(*pid))
        .map(|(_, session_id, name, cwd, status)| {
            let transcript = transcript_of(&session_id);
            let cache = transcript.as_deref().and_then(cache_state);
            Peer { name, session_id, cwd, status, transcript, cache }
        })
        .collect();
    peers.sort_by_key(|peer| (!peer.available(), peer.status != "idle"));
    peers
}

/// The folder of a running session, from its entry in ~/.claude/sessions.
pub fn folder_of(session: &str) -> Option<String> {
    js::read_dir_sorted(&sessions_dir()).into_iter()
        .filter_map(|file| js::parse(&std::fs::read_to_string(file.path()).ok()?))
        .find(|entry| js::str_of(entry, "sessionId") == session)
        .map(|entry| js::str_of(&entry, "cwd").to_string())
}

fn fit_question(card: Value) -> Value {
    json!({
        "instructions": {
            "question": "Does `session` already hold the context that `task` needs: the same feature, \
                files, bug or decisions? Then it could do the task without first reading its way in. \
                Sharing only the project, the tools or the language is not enough.",
            "session": card,
        },
        "criteria": {
            "true": "The session worked on what the task is about, so its context covers what the task needs.",
            "false": "The session worked on something else; the task would need context it does not have.",
        },
    })
}

/// Jev's fit per peer that can take work now: idle, warm, with a transcript. Others get None.
pub fn fits(task: &str, peers: &[Peer]) -> crate::Result<(Vec<Option<f64>>, f64)> {
    let asked: Vec<(usize, Value)> = peers.iter().enumerate()
        .filter(|(_, peer)| peer.available())
        .filter_map(|(i, peer)| {
            let session = read_session(peer.transcript.as_deref()?).ok()?;
            (!session.exchanges.is_empty()).then(|| (i, fit_question(session_card(&session))))
        })
        .collect();
    let mut result = vec![None; peers.len()];
    if asked.is_empty() {
        return Ok((result, 0.0));
    }
    let questions: Vec<Value> = asked.iter().map(|(_, q)| q.clone()).collect();
    let (probabilities, used) = ask_nouls(&json!({"task": js::clip(task, 6000)}), &questions, REQUEST_BUDGET_CHARS)?;
    for ((i, _), p) in asked.iter().zip(probabilities) {
        result[*i] = Some(p);
    }
    Ok((result, used))
}

/// For the shadow log of a subagent: the peers in the parent's folder that could have taken it.
pub fn shadow(parent: &str, task: &str) -> Value {
    let folder = folder_of(parent).unwrap_or_default();
    let peers: Vec<Peer> = running(parent).into_iter().filter(|p| p.available() && same_folder(&p.cwd, &folder)).collect();
    match fits(task, &peers) {
        Ok((fits, used)) => json!({
            "peers": peers.iter().zip(fits).map(|(peer, fit)| json!({
                "name": peer.name, "session_id": peer.session_id,
                "context": js::opt(peer.cache.as_ref().map(|c| c.context)),
                "left_ms": js::opt(peer.cache.as_ref().map(|c| c.left)),
                "fit": js::opt(fit),
            })).collect::<Vec<_>>(),
            "tokens": js::num(used),
        }),
        Err(error) => json!({"error": error}),
    }
}

fn line(peer: &Peer, fit: Option<f64>, here: &str) -> String {
    let cache = peer.cache.as_ref();
    let warm = match cache {
        Some(c) if c.warm => format!("warm {}", minutes(c.left)),
        Some(_) => "cold".into(),
        None => "—".into(),
    };
    let folder = if same_folder(&peer.cwd, here) { String::new() } else { format!("  {}", peer.cwd) };
    format!("  {}{}{}{}{}  {}{}",
        js::pad(&peer.name, 16), js::pad(&peer.status, 6),
        js::lpad(&cache.map_or("—".into(), |c| tokens(c.context)), 6),
        js::lpad(&cache.and_then(|c| c.per_call).map_or("—".into(), |v| format!("{}/call", usd(v))), 13),
        js::lpad(&warm, 10),
        fit.map_or(String::new(), |p| format!("fit {}", js::fixed(p, 2))), folder)
}

fn render(peers: &[Peer], fits: &[Option<f64>], task: &str, here: &str, used: f64) -> String {
    if peers.is_empty() {
        return "No other Claude Code sessions are running here.".into();
    }
    let mut out = vec!["Running sessions: status, context, cache read per call, cache left, fit.".to_string()];
    out.extend(peers.iter().zip(fits).map(|(peer, fit)| line(peer, *fit, here)));
    if task.is_empty() {
        return out.join("\n");
    }
    let best = peers.iter().zip(fits).filter_map(|(peer, fit)| Some((peer, (*fit)?))).filter(|(_, p)| *p >= FIT)
        .max_by(|a, b| a.1.total_cmp(&b.1));
    out.push(String::new());
    match best {
        Some((peer, _)) => {
            out.push(format!("{} fits. Send it the task with SendMessage to \"{}\"; it works in its own terminal, \
                in parallel with you.", peer.name, peer.name));
            if let Some(per_call) = peer.cache.as_ref().and_then(|c| c.per_call) {
                out.push(format!("Every call it makes reads its whole context: {} per call. For a long task, ask it \
                    for a briefing and give that to a subagent.", usd(per_call)));
            }
        }
        None => out.push("No idle, warm session fits this task.".into()),
    }
    out.push(format!("Jev: ${}", js::fixed(used / 1e6 * PRICE_PER_MILLION_TOKENS, 4)));
    out.join("\n")
}

pub fn peers_main(argv: &[String]) -> crate::Result<()> {
    let (mut all, mut words) = (false, Vec::new());
    for arg in argv {
        match arg.as_str() {
            "--all" => all = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => words.push(arg.as_str()),
        }
    }
    let task = js::trim(&words.join(" ")).to_string();
    let here = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let own = std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default();
    let peers: Vec<Peer> = running(&own).into_iter().filter(|p| all || same_folder(&p.cwd, &here)).collect();
    let (fits, used) = if task.is_empty() || !key_at_hand() { (vec![None; peers.len()], 0.0) } else { fits(&task, &peers)? };
    println!("{}", render(&peers, &fits, &task, &here, used));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_matches_itself_and_what_lies_inside_it() {
        assert!(same_folder("/p/lmnl", "/p/lmnl"));
        assert!(same_folder("/p/lmnl", "/p/lmnl/web"));
        assert!(same_folder("/p/lmnl/web", "/p/lmnl"));
        assert!(!same_folder("/p/lmnl", "/p/lmnl-old"));
        assert!(!same_folder("", "/p/lmnl"));
    }

    #[test]
    fn an_entry_needs_a_session_and_falls_back_to_its_id_for_a_name() {
        assert!(entry_of(&json!({"pid": 1})).is_none());
        let (pid, id, name, cwd, status) = entry_of(&json!({"pid": 42, "sessionId": "674b62a1-dd91", "cwd": "/p", "status": "idle"})).unwrap();
        assert_eq!((pid, id.as_str(), name.as_str(), cwd.as_str(), status.as_str()), (42, "674b62a1-dd91", "674b62a1", "/p", "idle"));
    }

    fn peer(status: &str, warm: bool) -> Peer {
        let cache = CacheState { context: 300_000.0, model: None, last_call_time: 0.0, last_call_cold: false, ttl: None,
            warm, left: if warm { 600_000.0 } else { 0.0 }, per_call: Some(0.06), cold_call: None };
        Peer { name: format!("{status}-{warm}"), session_id: String::new(), cwd: "/p".into(), status: status.into(), transcript: None, cache: Some(cache) }
    }

    #[test]
    fn only_an_idle_warm_session_can_take_work() {
        assert!(peer("idle", true).available());
        assert!(!peer("idle", false).available());
        assert!(!peer("busy", true).available());
    }

    #[test]
    fn a_fit_above_the_line_names_the_session() {
        let peers = [peer("idle", true), peer("idle", false)];
        let text = render(&peers, &[Some(0.9), None], "fix the deploy", "/p", 1000.0);
        assert!(text.contains("idle-true fits. Send it the task with SendMessage to \"idle-true\""));
        let text = render(&peers, &[Some(0.4), None], "fix the deploy", "/p", 1000.0);
        assert!(text.contains("No idle, warm session fits this task."));
    }
}
