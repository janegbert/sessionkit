// sessionkit setup — put the status line and the two hooks in Claude Code's settings, and the
// zsh hook that lets sessionkit next open the next session, in ~/.zshrc.
//
// A Claude Code plugin can ship hooks but not a status line, so this command writes both into
// ~/.claude/settings.json, with the path where sessionkit is installed on this machine. It makes
// a backup first, leaves every other setting and hook alone, and can run again safely: it
// replaces its own entries instead of adding them twice. --remove takes them out again.
//
// The zsh hook sits between two marker lines in ~/.zshrc, so setup can replace or remove it.
//
// It also installs the skill that tells Claude Code when and how to use sessionkit grep and
// sessionkit ask, in the skills folder next to the settings file, and the plugin through which
// /compact and auto-compaction go to sessionkit compact. That plugin is a function hook, an
// early-access part of Claude Code, so setup turns function hooks on in the settings.

use crate::Result;
use crate::js;
use crate::usage::home;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

const USAGE: &str = "Usage: sessionkit setup [options]

Adds the sessionkit status line and hooks to Claude Code's user settings, the sessionkit skill
to ~/.claude/skills, the plugin for compaction without a summary, and on zsh the hook that lets
sessionkit next open the next session to ~/.zshrc.

Options:
  --remove           take the sessionkit status line, hooks, skill and plugin out again
  --force            replace a status line that is not sessionkit's without asking
  --settings <file>  another settings file (default ~/.claude/settings.json)
  --zshrc <file>     another zsh startup file (default ~/.zshrc)
  --dry-run          print the result instead of writing it
  -h, --help         show this help";

const SKILL: &str = include_str!("SKILL.md");
const SKILL_NAME: &str = "sessionkit";
const OLD_SKILL_NAME: &str = "sessionkit-grep";

const PLUGIN_FILES: [(&str, &str); 23] = [
    (".claude-plugin/plugin.json", include_str!("../plugin/.claude-plugin/plugin.json")),
    (".claude-plugin/marketplace.json", include_str!("../plugin/.claude-plugin/marketplace.json")),
    ("hooks/hooks.json", include_str!("../plugin/hooks/hooks.json")),
    ("hooks/agent.js", include_str!("../plugin/hooks/agent.js")),
    ("hooks/architect.js", include_str!("../plugin/hooks/architect.js")),
    ("hooks/awareness.js", include_str!("../plugin/hooks/awareness.js")),
    ("agents/architect.md", include_str!("../plugin/agents/architect.md")),
    ("skills/architecture/SKILL.md", include_str!("../plugin/skills/architecture/SKILL.md")),
    ("skills/architecture/ONBOARD.md", include_str!("../plugin/skills/architecture/ONBOARD.md")),
    ("skills/architecture/AUDIT.md", include_str!("../plugin/skills/architecture/AUDIT.md")),
    ("hooks/compact.js", include_str!("../plugin/hooks/compact.js")),
    ("hooks/edit.js", include_str!("../plugin/hooks/edit.js")),
    ("hooks/effort.js", include_str!("../plugin/hooks/effort.js")),
    ("hooks/fold.js", include_str!("../plugin/hooks/fold.js")),
    ("hooks/index.js", include_str!("../plugin/hooks/index.js")),
    ("hooks/auto-permission.js", include_str!("../plugin/hooks/auto-permission.js")),
    ("hooks/permission-probe.js", include_str!("../plugin/hooks/permission-probe.js")),
    ("hooks/search-tools.js", include_str!("../plugin/hooks/search-tools.js")),
    ("hooks/prompt.js", include_str!("../plugin/hooks/prompt.js")),
    ("hooks/read.js", include_str!("../plugin/hooks/read.js")),
    ("hooks/repeat.js", include_str!("../plugin/hooks/repeat.js")),
    ("hooks/waiting.js", include_str!("../plugin/hooks/waiting.js")),
    ("output-styles/robot.md", include_str!("../plugin/output-styles/robot.md")),
];
/// What the plugin does, one line per hook or file, for the output of setup.
const PLUGIN_FEATURES: [&str; 12] = [
    "/compact and auto-compaction go through sessionkit compact, without a summary",
    "before a message to a cold session, a dialog asks: continue here, or compacted first",
    "before a message to a warm session above 375K, a dialog offers to compact first, or always at 375K",
    "an Edit refused because the file was not read is read and tried again, without the text in the context",
    "a teammate's idle notification that repeats its report gets a one-line result",
    "after the first large read, a tip that sessionkit ask answers without reading; outline first as an option in /config",
    "search_code and ask_file tools for agents, with a short navigation rule; disable Code search tools for CLI-only use",
    "per subagent, in shadow: the model and effort Jev would give it, and which warm session would fit",
    "per typed prompt, in shadow: the effort level Jev finds enough, beside the one the turn ran at; applying it as an option in /config",
    "as an option in /config: when a turn ends that added much tool output, its outputs are cut and the prompt cache stays",
    "the architect: /sessionkit:architecture onboard, then Jev wakes it for architectural plans and changes; advise, shadow or off in /config",
    "the output style sessionkit-robot, to pick with /output-style sessionkit-robot",
];

const PLUGIN_FOLDER: &str = "sessionkit-plugin";
const PLUGIN_ID: &str = "sessionkit@sessionkit";
const MARKETPLACE: &str = "sessionkit";
const FUNCTION_HOOKS: &str = "CLAUDE_CODE_ENABLE_FUNCTION_HOOKS";

const ZSH_START: &str = "# >>> sessionkit >>>";
const ZSH_END: &str = "# <<< sessionkit <<<";

/// A command is ours when it runs sessionkit's statusline or hook subcommand; this also matches
/// the entries of the JavaScript version, so setup replaces them.
fn ours(command: &str) -> bool {
    Regex::new(r#"sessionkit(\.mjs)?["']?\s+(statusline|hook\s)"#).unwrap().is_match(command)
}

struct Options {
    remove: bool,
    force: bool,
    dry_run: bool,
    settings: PathBuf,
    zshrc: Option<PathBuf>,
}

fn parse_arguments(argv: &[String]) -> Options {
    let zsh = std::env::var("SHELL").is_ok_and(|shell| shell.ends_with("zsh"));
    let zdotdir = std::env::var_os("ZDOTDIR").filter(|dir| !dir.is_empty()).map_or_else(home, PathBuf::from);
    let mut options = Options {
        remove: false,
        force: false,
        dry_run: false,
        settings: home().join(".claude").join("settings.json"),
        zshrc: zsh.then(|| zdotdir.join(".zshrc")),
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--remove" => options.remove = true,
            "--force" => options.force = true,
            "--dry-run" => options.dry_run = true,
            "--settings" => {
                i += 1;
                options.settings = argv.get(i).map(PathBuf::from).unwrap_or_default();
            }
            "--zshrc" => {
                i += 1;
                options.zshrc = argv.get(i).filter(|path| !path.is_empty()).map(PathBuf::from);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => {
                eprintln!("sessionkit setup: unknown option {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    options
}

/// Where sessionkit is: the command it was started as when that is a link named sessionkit
/// (Homebrew's, which survives an update); else the program itself.
fn sessionkit_path() -> String {
    let invoked = std::env::args().next().unwrap_or_default();
    let path = if invoked.contains('/') {
        std::path::absolute(&invoked).unwrap_or_else(|_| PathBuf::from(&invoked))
    } else {
        std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|dir| dir.join(&invoked)).find(|path| path.is_file()))
            .unwrap_or_else(|| PathBuf::from(&invoked))
    };
    let path = if path.file_name().is_some_and(|name| name == "sessionkit") {
        path
    } else {
        std::env::current_exe().and_then(std::fs::canonicalize).unwrap_or(path)
    };
    path.to_string_lossy().into_owned()
}

/// How Claude Code should call sessionkit from a command line.
fn sessionkit_command() -> String {
    let text = sessionkit_path();
    if text.chars().any(char::is_whitespace) { format!("\"{text}\"") } else { text }
}

/// Remove sessionkit's hook entries; drop matcher groups and events that end up empty.
fn without_our_hooks(hooks: Option<&Value>) -> Map<String, Value> {
    let mut result = Map::new();
    let Some(Value::Object(hooks)) = hooks else { return result };
    for (event, groups) in hooks {
        let kept: Vec<Value> = groups.as_array().into_iter().flatten().filter_map(|group| {
            let mut group = group.as_object()?.clone();
            let list: Vec<Value> = group.get("hooks").and_then(Value::as_array).into_iter().flatten()
                .filter(|hook| !ours(hook.get("command").and_then(Value::as_str).unwrap_or(""))).cloned().collect();
            let empty = list.is_empty();
            group.insert("hooks".into(), Value::Array(list));
            (!empty).then_some(Value::Object(group))
        }).collect();
        if !kept.is_empty() {
            result.insert(event.clone(), Value::Array(kept));
        }
    }
    result
}

/// Before each prompt, zsh looks for a handoff that sessionkit next left for this terminal, and
/// runs it. The file test keeps the prompt fast: sessionkit starts only when there is a handoff.
fn zsh_block(command: &str) -> String {
    [
        ZSH_START.to_string(),
        "# sessionkit next: open the next Claude Code session when this one closes".into(),
        "export SESSIONKIT_SHELL=zsh".into(),
        format!("_sessionkit_next() {{ [[ -f ~/.cache/sessionkit/next/${{TTY:t}}.json ]] && {command} next --run-pending ${{TTY:t}} }}"),
        "autoload -Uz add-zsh-hook && add-zsh-hook precmd _sessionkit_next".into(),
        ZSH_END.to_string(),
    ].join("\n")
}

fn without_zsh_block(text: &str) -> String {
    let block = Regex::new(&format!(r"\n*{}[\s\S]*?{}\n?", regex::escape(ZSH_START), regex::escape(ZSH_END))).unwrap();
    let text = block.replacen(text, 1, "\n");
    text.strip_prefix('\n').unwrap_or(&text).to_string()
}

fn backup(file: &Path, messages: &mut Vec<String>) -> Result<()> {
    let copy = format!("{}.sessionkit-backup-{}", file.display(), js::file_stamp());
    std::fs::copy(file, &copy).map_err(|error| format!("{error}, copyfile '{}'", file.display()))?;
    messages.push(format!("backup: {copy}"));
    Ok(())
}

fn ask(question: &str) -> bool {
    js::stdin_is_tty() && js::ask(question).to_lowercase().starts_with('y')
}

/// Claude Code's settings with sessionkit's status line, hooks and env put in, or taken out with
/// `remove`, and a line for each change. `replace` decides on a status line that is not ours; it
/// gets that status line's command. None when the settings are not a JSON object.
fn edit_settings(settings: &Value, command: &str, remove: bool, replace: impl FnOnce(&str) -> bool) -> Option<(Value, Vec<String>)> {
    let Value::Object(mut next) = settings.clone() else { return None };
    let mut hooks = without_our_hooks(settings.get("hooks"));
    // The hooks keep their place among the settings, or come last, as a spread in JavaScript
    // puts them: before a status line that is added below.
    next.insert("hooks".into(), json!({}));
    let mut messages: Vec<String> = Vec::new();

    let current = settings.get("statusLine").and_then(|s| s.get("command")).and_then(Value::as_str).unwrap_or("").to_string();
    let status_line_is_ours = ours(&current);
    if remove {
        if status_line_is_ours {
            next.shift_remove("statusLine");
            messages.push("removed the status line".into());
        }
        messages.push("removed the hooks".into());
        if settings.get("env").and_then(|env| env.get(FUNCTION_HOOKS)).is_some() {
            messages.push(format!("kept {FUNCTION_HOOKS}; other plugins may use function hooks too"));
        }
    } else {
        if !js::truthy(settings.get("statusLine")) || status_line_is_ours || replace(&current) {
            next.insert("statusLine".into(), json!({"type": "command", "command": format!("{command} statusline"), "refreshInterval": 60}));
            messages.push("status line: context, cost per call and cache time".into());
        } else {
            messages.push("kept the existing status line (run with --force to replace it)".into());
        }
        let mut add = |event: &str, group: Value| {
            let list = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
            if let Value::Array(list) = list {
                list.push(group);
            }
        };
        add("SessionStart", json!({"matcher": "resume|fork", "hooks": [{"type": "command", "command": format!("{command} hook session-start")}]}));
        add("UserPromptSubmit", json!({"hooks": [{"type": "command", "command": format!("{command} hook prompt"), "timeout": 10}]}));
        messages.push("hook on --resume: warns when the expired cache is expensive to write again".into());
        messages.push("hook on each message: a message to a cold, expensive session continues in a fresh one".into());
        let mut env = next.get("env").and_then(Value::as_object).cloned().unwrap_or_default();
        if env.get(FUNCTION_HOOKS).and_then(Value::as_str) != Some("1") {
            env.insert(FUNCTION_HOOKS.into(), "1".into());
            next.insert("env".into(), Value::Object(env));
            messages.push(format!("{FUNCTION_HOOKS}=1: function hooks, early access, for compaction without a summary"));
        }
    }
    if hooks.is_empty() {
        next.shift_remove("hooks");
    } else {
        next.insert("hooks".into(), Value::Object(hooks));
    }
    Some((Value::Object(next), messages))
}

/// A zsh startup file with sessionkit's block in place of an earlier one, or without it.
fn edit_zshrc(zshrc: &str, command: &str, remove: bool) -> String {
    let kept = without_zsh_block(zshrc);
    if remove {
        return kept;
    }
    let joined = format!("{}\n\n{}\n", kept.trim_end_matches('\n'), zsh_block(command));
    joined.trim_start_matches('\n').to_string()
}

pub fn setup_main(argv: &[String]) -> Result<()> {
    let options = parse_arguments(argv);
    let settings: Value = if options.settings.exists() {
        let text = std::fs::read_to_string(&options.settings).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", options.settings.display()))?
    } else {
        json!({})
    };
    let command = sessionkit_command();
    let replace = |current: &str| options.force || ask(&format!("There is a status line already:\n  {current}\nReplace it with sessionkit's? [y/N] "));
    let (next, mut messages) = edit_settings(&settings, &command, options.remove, replace)
        .ok_or_else(|| format!("{} is not a JSON object.", options.settings.display()))?;

    let text = format!("{}\n", js::stringify_pretty(&next));
    let zshrc = options.zshrc.as_ref().filter(|path| path.exists()).and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    let next_zshrc = options.zshrc.as_ref().map(|_| edit_zshrc(&zshrc, &command, options.remove));
    if options.dry_run {
        println!("{text}");
        if let (Some(path), Some(next_zshrc)) = (&options.zshrc, &next_zshrc) {
            println!("{}:\n{next_zshrc}", path.display());
        }
        return Ok(());
    }
    if options.settings.exists() {
        backup(&options.settings, &mut messages)?;
    } else if let Some(parent) = options.settings.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&options.settings, &text).map_err(|e| e.to_string())?;
    println!("sessionkit setup: {}\n{}\nNew Claude Code sessions pick this up; in a running session, open /hooks once.",
        options.settings.display(), messages.iter().map(|line| format!("  - {line}")).collect::<Vec<_>>().join("\n"));

    let mut zsh_messages: Vec<String> = Vec::new();
    match (&options.zshrc, next_zshrc) {
        (None, _) | (_, None) => {
            println!("No zsh: sessionkit next opens the next session only in sessions that sessionkit start launched.");
        }
        (Some(path), Some(next_zshrc)) if next_zshrc != zshrc => {
            if !zshrc.is_empty() {
                backup(path, &mut zsh_messages)?;
            }
            std::fs::write(path, &next_zshrc).map_err(|e| e.to_string())?;
            zsh_messages.push(if options.remove { "removed the zsh hook".into() } else {
                "zsh hook: sessionkit next opens the next session in this terminal".into()
            });
            println!("sessionkit setup: {}\n{}\n{}", path.display(),
                zsh_messages.iter().map(|line| format!("  - {line}")).collect::<Vec<_>>().join("\n"),
                if options.remove { "" } else { "New terminals pick this up; in an open terminal, run: source ~/.zshrc" });
        }
        _ => {}
    }
    install_skill(&options)?;
    install_plugin(&options, &settings)
}

/// Remove a skill folder that sessionkit wrote: only a SKILL.md with that name in it.
fn remove_skill(folder: &Path, name: &str) -> Result<bool> {
    let file = folder.join("SKILL.md");
    if !std::fs::read_to_string(&file).is_ok_and(|text| text.contains(&format!("name: {name}\n"))) {
        return Ok(false);
    }
    std::fs::remove_file(&file).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir(folder);
    Ok(true)
}

/// The sessionkit skill, in the skills folder beside the settings file. Only a skill with our
/// name is replaced or removed; the grep skill of earlier versions goes.
fn install_skill(options: &Options) -> Result<()> {
    let skills = options.settings.parent().unwrap_or(Path::new(".")).join("skills");
    if remove_skill(&skills.join(OLD_SKILL_NAME), OLD_SKILL_NAME)? {
        println!("sessionkit setup: {}\n  - removed the skill of earlier versions; the sessionkit skill replaces it", skills.join(OLD_SKILL_NAME).display());
    }
    let folder = skills.join(SKILL_NAME);
    let file = folder.join("SKILL.md");
    let current = std::fs::read_to_string(&file).ok();
    if options.remove {
        if remove_skill(&folder, SKILL_NAME)? {
            println!("sessionkit setup: {}\n  - removed the sessionkit skill", file.display());
        }
        return Ok(());
    }
    if current.as_deref() == Some(SKILL) {
        return Ok(());
    }
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    std::fs::write(&file, SKILL).map_err(|e| e.to_string())?;
    println!("sessionkit setup: {}\n  - skill: Claude Code uses sessionkit grep and sessionkit ask instead of reading files one by one", file.display());
    Ok(())
}

/// Run the claude command; its own message when it fails.
fn claude(args: &[&str]) -> std::result::Result<(), String> {
    let output = std::process::Command::new("claude").args(args).stdin(std::process::Stdio::null()).output()
        .map_err(|error| format!("claude {}: {error}", args.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    Err(format!("claude {}: {}", args.join(" "), js::trim(&text)))
}

/// The compaction plugin: written beside the settings file with the path of sessionkit in it,
/// then installed from there as a local marketplace. Claude Code manages its plugins itself,
/// so setup asks the claude command to do it, and only for the real settings file.
fn install_plugin(options: &Options, settings: &Value) -> Result<()> {
    let folder = options.settings.parent().unwrap_or(Path::new(".")).join(PLUGIN_FOLDER);
    let managed = options.settings == home().join(".claude").join("settings.json");
    if options.remove {
        if managed {
            let _ = claude(&["plugin", "uninstall", PLUGIN_ID]);
            let _ = claude(&["plugin", "marketplace", "remove", MARKETPLACE]);
        }
        if folder.exists() {
            std::fs::remove_dir_all(&folder).map_err(|e| e.to_string())?;
            println!("sessionkit setup: {}\n  - removed the compaction plugin", folder.display());
        }
        return Ok(());
    }
    for (path, text) in PLUGIN_FILES {
        let text = text.replace("__VERSION__", env!("CARGO_PKG_VERSION")).replace("\"__SESSIONKIT__\"", &js::quote(&sessionkit_path()));
        let file = folder.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&file, text).map_err(|e| e.to_string())?;
    }
    let installed = if !managed {
        Err(format!("another settings file; install it with: claude plugin marketplace add {} && claude plugin install {PLUGIN_ID}", folder.display()))
    } else {
        // Out and in again: a plugin keeps its cached copy while its version stays the same.
        let _ = claude(&["plugin", "uninstall", PLUGIN_ID]);
        let _ = claude(&["plugin", "marketplace", "remove", MARKETPLACE]);
        claude(&["plugin", "marketplace", "add", &folder.to_string_lossy()]).and_then(|_| claude(&["plugin", "install", PLUGIN_ID, "--scope", "user"]))
    };
    match installed {
        Ok(()) => println!("sessionkit setup: {}\n{}", folder.display(),
            PLUGIN_FEATURES.iter().map(|line| format!("  - plugin: {line}")).collect::<Vec<_>>().join("\n")),
        Err(error) => println!("sessionkit setup: {}\n  - plugin written, not installed: {error}", folder.display()),
    }
    let fast_jev = settings.get("enabledPlugins").and_then(Value::as_object).into_iter().flatten()
        .any(|(id, on)| id.starts_with("fast-jev-compaction@") && on.as_bool() == Some(true));
    if fast_jev {
        println!("  - fast-jev-compaction is enabled too; it does the same job. Turn one off: claude plugin disable fast-jev-compaction@fast-jev-compaction");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Every file of the plugin folder except tests is installed: index.js imports them all.
    #[test]
    fn setup_installs_every_plugin_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin");
        let mut found = Vec::new();
        let mut folders = vec![root.clone()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() { folders.push(path); continue; }
                let relative = path.strip_prefix(&root).unwrap().to_string_lossy().to_string();
                if !relative.ends_with(".test.ts") && !relative.ends_with(".DS_Store") { found.push(relative); }
            }
        }
        let listed: Vec<&str> = PLUGIN_FILES.iter().map(|(path, _)| *path).collect();
        let missing: Vec<&String> = found.iter().filter(|f| !listed.contains(&f.as_str())).collect();
        assert!(missing.is_empty(), "not in PLUGIN_FILES: {missing:?}");
    }

    use super::*;

    fn setup(settings: &Value) -> Value {
        edit_settings(settings, "/opt/sessionkit", false, |_| false).unwrap().0
    }

    fn remove(settings: &Value) -> Value {
        edit_settings(settings, "/opt/sessionkit", true, |_| false).unwrap().0
    }

    #[test]
    fn setup_twice_gives_the_same_settings() {
        let once = setup(&json!({"theme": "dark"}));
        assert_eq!(setup(&once), once);
        assert_eq!(once["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn remove_leaves_the_other_settings_and_hooks_alone() {
        let theirs = json!({"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}});
        let mut expected = theirs.clone();
        // Other plugins may use function hooks too, so remove keeps the env setting.
        expected["env"] = json!({FUNCTION_HOOKS: "1"});
        assert_eq!(remove(&setup(&theirs)), expected);
    }

    #[test]
    fn the_entries_of_the_javascript_version_are_replaced() {
        let old = json!({"hooks": {"UserPromptSubmit": [{"hooks": [
            {"type": "command", "command": "node /x/sessionkit.mjs hook prompt"}, {"type": "command", "command": "other-tool"}]}]}});
        let commands: Vec<String> = setup(&old)["hooks"]["UserPromptSubmit"].as_array().unwrap().iter()
            .flat_map(|group| group["hooks"].as_array().unwrap().iter().map(|hook| hook["command"].as_str().unwrap().to_string())).collect();
        assert_eq!(commands, ["other-tool", "/opt/sessionkit hook prompt"]);
    }

    #[test]
    fn a_status_line_that_is_not_ours_is_replaced_only_when_the_caller_agrees() {
        let theirs = json!({"statusLine": {"type": "command", "command": "ccstatus"}});
        let mut asked = Vec::new();
        let (kept, _) = edit_settings(&theirs, "/opt/sessionkit", false, |current| { asked.push(current.to_string()); false }).unwrap();
        assert_eq!(kept["statusLine"]["command"], "ccstatus");
        assert_eq!(asked, ["ccstatus"]);
        let (replaced, _) = edit_settings(&theirs, "/opt/sessionkit", false, |_| true).unwrap();
        assert_eq!(replaced["statusLine"]["command"], "/opt/sessionkit statusline");
        edit_settings(&replaced, "/opt/sessionkit", false, |_| panic!("asked about our own status line")).unwrap();
    }

    #[test]
    fn settings_that_are_not_an_object_are_refused() {
        assert!(edit_settings(&json!([1, 2]), "/opt/sessionkit", false, |_| false).is_none());
    }

    #[test]
    fn the_zsh_block_is_replaced_once_and_removed_cleanly() {
        let before = "alias a=b\n";
        let once = edit_zshrc(before, "/opt/sessionkit", false);
        assert_eq!(edit_zshrc(&once, "/opt/sessionkit", false), once);
        assert_eq!(once.matches(ZSH_START).count(), 1);
        assert_eq!(edit_zshrc(&once, "/opt/sessionkit", true), before);
    }
}
