//! Shared credentials for Jev / TypeSafe. No personal vault is assumed.
use crate::{Result, js};
use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;

const SERVICE: &str = "sessionkit";
const OLD_SERVICE: &str = "jev-start";
const ACCOUNT: &str = "TYPESAFE_API_KEY";
const HELP: &str = "Usage: sessionkit auth login [--stdin] | logout | status

Configure the TypeSafe API key used by Jev. login prompts without echo and stores the key
in the macOS Keychain; --stdin reads it from standard input instead (not a command argument).
TYPESAFE_API_KEY overrides the stored key, on every platform.
Optional: SESSIONKIT_OP_REFERENCE=op://vault/item/field enables a 1Password CLI fallback.
login stores the key without making a paid API request or validating it with TypeSafe.";

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum KeySource { Environment, Keychain, OnePassword }
static KEY: Mutex<Option<(String, KeySource)>> = Mutex::new(None);

fn env_key() -> String {
    std::env::var("TYPESAFE_API_KEY").map(|v| js::trim(&v).to_string()).unwrap_or_default()
}
fn op_reference() -> Option<String> {
    std::env::var("SESSIONKIT_OP_REFERENCE").ok().filter(|v| !v.trim().is_empty())
}
fn read_keychain(service: &str) -> String {
    Command::new("security").args(["find-generic-password", "-s", service, "-a", ACCOUNT, "-w"])
        .stdin(Stdio::null()).stderr(Stdio::null()).output().ok().filter(|o| o.status.success())
        .map(|o| js::trim(&String::from_utf8_lossy(&o.stdout)).to_string()).unwrap_or_default()
}
fn stored_key() -> String {
    let key = read_keychain(SERVICE);
    if key.is_empty() { read_keychain(OLD_SERVICE) } else { key }
}
fn validate(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("The key must be nonempty and contain no control characters.".into());
    }
    Ok(value)
}
fn write_keychain(value: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err("Keychain storage requires macOS. Set TYPESAFE_API_KEY instead.".into());
    }
    let expected = validate(value)?;
    let value = expected.replace('\\', "\\\\").replace('"', "\\\"");
    // Interactive security mode keeps the secret out of process arguments.
    let mut child = Command::new("security").arg("-i").stdin(Stdio::piped())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|_| "Could not open the macOS Keychain.".to_string())?;
    let mut input = child.stdin.take().expect("piped stdin");
    let written = writeln!(input, "add-generic-password -U -s {SERVICE} -a {ACCOUNT} -w \"{value}\"");
    drop(input);
    let status = child.wait().map_err(|_| "Could not store the key in the macOS Keychain.".to_string())?;
    // security -i can exit successfully even when its interactive command failed.
    if written.is_err() || !status.success() || read_keychain(SERVICE) != expected {
        return Err("Could not store the key in the macOS Keychain.".into());
    }
    Ok(())
}
pub(crate) fn refresh_key() -> Result<String> {
    let reference = op_reference().ok_or_else(||
        "No usable TypeSafe key. Run sessionkit auth login or set TYPESAFE_API_KEY.".to_string())?;
    if !reference.starts_with("op://") { return Err("SESSIONKIT_OP_REFERENCE must start with op://.".into()); }
    let output = Command::new("op").args(["read", &reference]).stderr(Stdio::null()).output()
        .map_err(|_| "Could not run the 1Password CLI. Run sessionkit auth login or install and sign in to op.".to_string())?;
    if !output.status.success() { return Err("Could not read SESSIONKIT_OP_REFERENCE from 1Password. Check the reference and your op sign-in.".into()); }
    let text = String::from_utf8_lossy(&output.stdout);
    let key = validate(&text)?.to_string();
    if cfg!(target_os = "macos") {
        if write_keychain(&key).is_err() { eprintln!("sessionkit: could not cache the key in Keychain; 1Password may ask again next time."); }
    }
    *KEY.lock().unwrap() = Some((key.clone(), KeySource::OnePassword));
    Ok(key)
}
pub(crate) fn key() -> Result<(String, KeySource)> {
    let mut cached = KEY.lock().unwrap();
    if let Some(key) = cached.as_ref() { return Ok(key.clone()); }
    let environment = env_key();
    let found = if !environment.is_empty() { (environment, KeySource::Environment) } else {
        let stored = stored_key();
        if stored.is_empty() {
            drop(cached);
            return refresh_key().map(|v| (v, KeySource::OnePassword));
        }
        (stored, KeySource::Keychain)
    };
    *cached = Some(found.clone());
    Ok(found)
}
pub fn typesafe_key() -> Result<String> { key().map(|(v, _)| v) }
/// Hooks never trigger a 1Password prompt.
pub fn key_at_hand() -> bool {
    KEY.lock().unwrap().is_some() || !env_key().is_empty() || !stored_key().is_empty()
}
pub(crate) fn can_refresh() -> bool { op_reference().is_some() }

#[cfg(unix)]
fn prompt_key() -> Result<String> {
    use std::os::fd::AsRawFd;
    let mut tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty")
        .map_err(|_| "No terminal available. Use sessionkit auth login --stdin.".to_string())?;
    let fd = tty.as_raw_fd();
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 { return Err("Could not read terminal settings.".into()); }
    let original = unsafe { original.assume_init() };
    struct Restore(i32, libc::termios);
    impl Drop for Restore {
        fn drop(&mut self) { unsafe { libc::tcsetattr(self.0, libc::TCSANOW, &self.1); } }
    }
    let mut hidden = original;
    hidden.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 { return Err("Could not disable terminal echo.".into()); }
    let restore = Restore(fd, original);
    write!(tty, "TypeSafe API key: ").and_then(|_| tty.flush()).map_err(|_| "Could not write to terminal.".to_string())?;
    let mut value = String::new();
    let result = std::io::BufReader::new(&tty).read_line(&mut value);
    drop(restore);
    let _ = writeln!(tty);
    result.map_err(|_| "Could not read key.".to_string())?;
    Ok(value)
}
#[cfg(not(unix))]
fn prompt_key() -> Result<String> { Err("Use sessionkit auth login --stdin or set TYPESAFE_API_KEY.".into()) }

pub fn auth_main(args: &[String]) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["--help"] | ["-h"] | ["login", "--help"] => println!("{HELP}"),
        ["login"] | ["login", "--stdin"] => {
            if !cfg!(target_os = "macos") { return Err("Keychain storage requires macOS. Set TYPESAFE_API_KEY instead.".into()); }
            let value = if args.len() == 2 {
                let mut value = String::new();
                std::io::stdin().read_to_string(&mut value).map_err(|_| "Could not read key from stdin.".to_string())?;
                value
            } else { prompt_key()? };
            write_keychain(validate(&value)?)?;
            println!("TypeSafe key for Jev stored in the macOS Keychain (not validated with TypeSafe).");
            if !env_key().is_empty() { println!("Note: TYPESAFE_API_KEY is set and takes precedence over this key."); }
        }
        ["status"] => {
            let source = if !env_key().is_empty() { "TYPESAFE_API_KEY" }
                else if !stored_key().is_empty() { "macOS Keychain" }
                else if can_refresh() { "1Password fallback configured (not checked)" }
                else { "not configured; run sessionkit auth login or set TYPESAFE_API_KEY" };
            println!("TypeSafe key for Jev: {source}");
        }
        ["logout"] => {
            if !cfg!(target_os = "macos") { return Err("Keychain storage requires macOS. Unset TYPESAFE_API_KEY instead.".into()); }
            for service in [SERVICE, OLD_SERVICE] {
                let output = Command::new("security").args(["delete-generic-password", "-s", service, "-a", ACCOUNT])
                    .stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|_| "Could not open the macOS Keychain.".to_string())?;
                if !output.success() && output.code() != Some(44) { return Err("Could not remove the stored TypeSafe key.".into()); }
            }
            *KEY.lock().unwrap() = None;
            println!("Stored TypeSafe keys removed. Environment variables and 1Password items are unchanged.");
        }
        _ => return Err(HELP.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_validation() {
        assert_eq!(validate("  test-key\n").unwrap(), "test-key");
        assert!(validate("").is_err());
        assert!(validate("key\nanother").is_err());
        assert!(validate("key\0").is_err());
    }
}
