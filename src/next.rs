// sessionkit next — from inside Claude Code, close this session and start a fresh one with a
// new prompt, when the prompt cache has gone cold.
//
// Run it with Claude Code's ! prefix:  ! sessionkit next "the next prompt"
//
// A ! command runs under Claude Code without the terminal, so it cannot start a new interactive
// session itself. Something that owns the terminal must do it once Claude Code has exited. next
// leaves the prompt in a handoff file for one of two owners, and stops Claude Code:
//   - the sessionkit start that launched this Claude Code (file named after Claude Code's pid);
//   - else the zsh hook that sessionkit setup installs (file named after the terminal), which
//     runs sessionkit next --run-pending before the next shell prompt.
// Either one then runs sessionkit start --session <this session> "the next prompt".

use crate::Result;
use crate::cache::cache_state;
use crate::js;
use crate::usage::{home, projects_dir};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

const HANDOFF_MAX_AGE_MS: f64 = 60.0 * 1000.0; // an older handoff is left over from a stop that failed

const USAGE: &str = "Usage: ! sessionkit next [options] \"prompt\"

Inside Claude Code: when the prompt cache of this session has gone cold, close the session and
continue in a fresh one with the prompt: sessionkit start --session <this session> \"prompt\".

Options:
  --force     also when the cache is still warm
  -h, --help  show this help";

fn handoff_dir() -> PathBuf {
    home().join(".cache").join("sessionkit").join("next")
}

struct Options {
    force: bool,
    run_pending: Option<String>,
    prompt: String,
}

fn parse_arguments(argv: &[String]) -> Options {
    let mut options = Options { force: false, run_pending: None, prompt: String::new() };
    let mut words = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--force" => options.force = true,
            "--run-pending" => {
                i += 1;
                options.run_pending = argv.get(i).cloned();
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            word => words.push(word.to_string()),
        }
        i += 1;
    }
    options.prompt = js::trim(&words.join(" ")).to_string();
    if options.prompt.is_empty() && options.run_pending.as_deref().is_none_or(str::is_empty) {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    options
}

pub fn transcript_of(session: &str) -> Option<PathBuf> {
    let projects = projects_dir();
    js::read_dir_names(&projects).ok()?.into_iter()
        .map(|folder| projects.join(folder).join(format!("{session}.jsonl")))
        .find(|path| path.exists())
}

pub fn ps_field(field: &str, pid: i64) -> Result<String> {
    let output = Command::new("ps").args(["-o", &format!("{field}="), "-p", &pid.to_string()]).output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("Command failed: ps -o {field}= -p {pid}"));
    }
    Ok(js::trim(&String::from_utf8_lossy(&output.stdout)).to_string())
}

/// The terminal a process runs in, as zsh's ${TTY:t} names it (ttys001), or None.
fn terminal_of(pid: i64) -> Result<Option<String>> {
    let name = ps_field("tty", pid)?;
    let tty = name.strip_prefix("ttys").or_else(|| name.strip_prefix("tty"));
    Ok(tty.filter(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).map(|_| name.clone()))
}

pub struct Handoff {
    pub session: String,
    pub prompt: String,
}

/// The handoff for this key, once: the file is removed, and a stale one is ignored.
pub fn take_handoff(key: &str) -> Option<Handoff> {
    let file = handoff_dir().join(format!("{key}.json"));
    if !file.exists() {
        return None;
    }
    let handoff = std::fs::read_to_string(&file).ok().and_then(|text| js::parse(&text));
    let _ = std::fs::remove_file(&file);
    let handoff = handoff?;
    let time = handoff.get("time").and_then(serde_json::Value::as_f64).unwrap_or(f64::NAN);
    (js::now() - time < HANDOFF_MAX_AGE_MS).then(|| Handoff {
        session: js::str_of(&handoff, "session").to_string(),
        prompt: js::str_of(&handoff, "prompt").to_string(),
    })
}

/// Start the session a handoff asks for: what the owner of the terminal runs.
pub fn run_handoff(handoff: Handoff) -> Result<()> {
    eprint!("\nsessionkit: continuing session {} in a fresh one\n", js::head(&handoff.session, 8));
    crate::start::start_main(&["--session".into(), handoff.session, handoff.prompt])
}

/// A pid from the environment, as Number(process.env[name]); None when it is not a number or 0.
pub fn env_pid(name: &str) -> Option<i64> {
    let value = js::parse_number(std::env::var(name).ok().as_deref());
    (js::truthy_number(value) && value.fract() == 0.0).then_some(value as i64)
}

pub fn next_main(argv: &[String]) -> Result<()> {
    let options = parse_arguments(argv);
    if let Some(key) = options.run_pending.as_deref().filter(|key| !key.is_empty()) {
        return match take_handoff(key) {
            Some(handoff) => run_handoff(handoff),
            None => Ok(()),
        };
    }

    let session = std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default();
    let claude_pid = env_pid("CLAUDE_PID");
    let (false, Some(claude_pid)) = (session.is_empty(), claude_pid) else {
        return Err("run this inside Claude Code, with the ! prefix.".into());
    };
    let restart = format!("sessionkit start --session {} {}", js::head(&session, 8), js::quote(&options.prompt));

    let state = transcript_of(&session).and_then(|transcript| cache_state(&transcript));
    if let Some(state) = state.filter(|state| state.warm && !options.force) {
        let cost = match state.per_call {
            None => "only a cache read".to_string(),
            Some(per_call) => format!("about ${}", js::fixed(per_call, 3)),
        };
        println!("The cache is still warm for {} more minutes: the next message costs {cost}. \
            Send the prompt here, or add --force to start fresh anyway.", js::number((state.left / 60000.0).ceil()));
        return Ok(());
    }

    let Some(key) = handoff_key(claude_pid)? else {
        println!("Nothing in this terminal can open the next session after this one closes. Run \
            sessionkit setup once to add the zsh hook, and open a new terminal. For now, exit Claude \
            Code and run:\n  {restart}");
        return Ok(());
    };
    println!("Closing this session; next: {restart}");
    leave_handoff(&key, &session, &options.prompt);
    terminate(claude_pid);
    Ok(())
}

/// Send SIGTERM to a process.
pub fn terminate(pid: i64) {
    // SAFETY: kill has no memory effects; a wrong pid only returns an error.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

/// Who can open the next session once this Claude Code has exited: the sessionkit start that
/// launched it (key: Claude Code's pid), else the zsh hook (key: the terminal), else nobody.
pub fn handoff_key(claude_pid: i64) -> Result<Option<String>> {
    if let Some(launcher) = env_pid("SESSIONKIT_LAUNCHER") {
        let parent = js::parse_number(Some(&ps_field("ppid", claude_pid)?));
        if parent == launcher as f64 {
            return Ok(Some(claude_pid.to_string()));
        }
    }
    if std::env::var("SESSIONKIT_SHELL").is_ok_and(|shell| !shell.is_empty()) { terminal_of(claude_pid) } else { Ok(None) }
}

/// Leave the prompt for the owner of the terminal; it acts once Claude Code has exited.
pub fn leave_handoff(key: &str, session: &str, prompt: &str) {
    let dir = handoff_dir();
    let _ = std::fs::create_dir_all(&dir);
    let record = json!({"session": session, "prompt": prompt, "time": js::num(js::now())});
    let _ = std::fs::write(dir.join(format!("{key}.json")), js::stringify(&record));
}
