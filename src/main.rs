// sessionkit — start, measure and analyse agent sessions.
//
//   sessionkit start    start Claude Code, Codex or jcode with what matters from earlier sessions
//   sessionkit next     inside Claude Code: close a cold session and continue in a fresh one
//   sessionkit peers    the running sessions, and whether a warm, idle one fits a task
//   sessionkit usage    where the tokens and the money go in your Claude Code sessions
//   sessionkit measure  measure whether start selects the right context
//   sessionkit statusline, sessionkit hook   show in Claude Code what the prompt cache costs
//   sessionkit setup    put the status line and hooks in Claude Code's settings
//   sessionkit grep, sessionkit ask   find code, or a fact about a file, by asking Jev
//   sessionkit compact  compact a conversation without a summary, from Claude Code's /compact
//   sessionkit trim     measure cutting large tool results down to the task, before the context
//   sessionkit skills   find work you repeat across sessions, where a skill would save time
//
// A prompt without a command means start.

mod architect;
mod ask;
mod auth;
mod awareness;
mod cache;
mod claude;
mod compact;
mod grep;
mod jev;
mod js;
mod measure;
mod next;
mod peers;
mod pricing;
mod setup;
mod skills;
mod start;
mod transcript;
mod trim;
mod usage;

const HELP: &str = "sessionkit — start, measure and analyse agent sessions

Usage:
  sessionkit start [options] \"prompt\"   start an agent with what matters from earlier sessions
  ! sessionkit next \"prompt\"            inside Claude Code: close a cold session, continue fresh
  sessionkit peers [--all] [\"task\"]     running sessions; which warm, idle one fits a task (Jev)
  sessionkit usage [options]            where the tokens and the money go (local, free)
  sessionkit measure [options]          measure the selection of start (costs money)
  sessionkit statusline                 Claude Code status line: context, cost per call, cache
  sessionkit hook session-start|prompt  Claude Code hooks: warn on a costly resume or cold message
  sessionkit auth login|logout|status   configure the TypeSafe API key for Jev
  sessionkit setup [--remove]           install the status line and hooks in Claude Code
  sessionkit grep \"question\" [root]     find code by asking what it does (Jev, about $0.0005 a file)
  sessionkit ask <file> \"question\"      ask about a file instead of reading it (Jev, about $0.0005 a file)
  sessionkit architect rank [root]      order onboarding's calibration questions by what the code shows (Jev)
  sessionkit compact <session>          what a compaction without summary would keep (Jev)
  sessionkit trim --measure [n]         measure cutting large tool results to the task (Jev)
  sessionkit skills [--draft n]         find work you repeat, where a skill would help (Jev)
  sessionkit --version                  print the version
  sessionkit \"prompt\"                   the same as start

Run sessionkit <command> --help for the options of a command.";

pub type Result<T> = std::result::Result<T, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        println!("{HELP}");
        std::process::exit(2);
    };
    if matches!(command.as_str(), "help" | "-h" | "--help") {
        println!("{HELP}");
        return;
    }
    if matches!(command.as_str(), "version" | "-V" | "--version") {
        println!("sessionkit {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let rest = &args[1..];
    let result = match command.as_str() {
        "start" => start::start_main(rest),
        "next" => next::next_main(rest),
        "peers" => peers::peers_main(rest),
        "usage" => usage::usage_main(rest),
        "measure" => measure::measure_main(rest),
        "statusline" => claude::statusline_main(),
        "hook" => claude::hook_main(rest),
        "setup" => setup::setup_main(rest),
        "auth" => auth::auth_main(rest),
        "architect" => architect::architect_main(rest),
        "grep" => grep::grep_main(rest),
        "ask" => ask::ask_main(rest),
        "compact" => compact::compact_main(rest),
        "trim" => trim::trim_main(rest),
        "skills" => skills::skills_main(rest),
        #[cfg(feature = "test-api")]
        "grep-inspect" => grep::inspect_main(rest),
        #[cfg(feature = "test-api")]
        "grep-python" => grep::python_main(rest),
        _ => start::start_main(&args),
    };
    if let Err(message) = result {
        eprintln!("sessionkit: {message}");
        std::process::exit(1);
    }
}
