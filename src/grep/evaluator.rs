// Jev calls for grep, as jevgrep's core/evaluator.ts and core/cache.ts make them: one System One
// request per evaluation, at most `concurrency` in flight, a shared pause after a 429, no backoff.
// A navigation request with several questions gets one attempt (two after a 429); every other
// request is tried twice. A 401 or 403 stops the search. Validated answers are cached for a week.

use crate::js;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub const MODEL: &str = "jev-1.13.0";
const BASE_URL: &str = "https://api.typesafe.ai/v1";
const REQUEST_LIMIT: usize = 50_000;
const TIMEOUT_MS: u64 = 60_000; // jevgrep 0.4.3 and later, for TypeSafe: a large state can take longer than 15 s
/// Our own parser identity: answers computed on jevgrep's CPython and TypeScript boundaries are
/// never reused.
const PARSER_VERSION: &str = "sessionkit-tree-sitter-python-0.25-oxc-0.151";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Authentication,
    RequestLimit,
    Provider,
    Cancelled,
    SourceInvalid,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Authentication => "authentication",
            Kind::RequestLimit => "request-limit",
            Kind::Provider => "provider",
            Kind::Cancelled => "cancelled",
            Kind::SourceInvalid => "source-invalid",
        }
    }
}

#[derive(Debug)]
pub struct Failure {
    pub kind: Kind,
    pub split_eligible: bool,
    pub message: String,
}

impl Failure {
    pub fn of(kind: Kind) -> Failure {
        Failure { kind, split_eligible: false, message: format!("Jev evaluation failed: {}", kind.name()) }
    }
}

pub type Answers = indexmap::IndexMap<String, f64>;

/// Why an attempt failed, before it becomes a Failure.
enum Attempt {
    Status(u16, Option<String>),
    Timeout,
    Network,
    Invalid,
}

pub struct Evaluator {
    api_key: String,
    endpoint: String,
    concurrency: usize,
    policy_version: String,
    cache: Option<Cache>,
    pub requests: AtomicUsize,
    pub cache_hits: AtomicUsize,
    cooldown_until: Mutex<f64>,
    /// Request slots in use, and who waits for one, first come first served.
    slots: Mutex<Slots>,
    released: Condvar,
    authentication_failed: AtomicBool,
    cancelled: &'static AtomicBool,
    agent: ureq::Agent,
    /// Evaluations validate their sources and take a request slot in the order they were asked
    /// for, as jevgrep's single-threaded queues do.
    next_ticket: AtomicU64,
    serving: Mutex<u64>,
    turn: Condvar,
}

#[derive(Default)]
struct Slots {
    active: usize,
    waiting: std::collections::VecDeque<u64>,
    next_waiter: u64,
}

/// A request slot in use, given back when dropped.
struct Slot<'a> {
    evaluator: &'a Evaluator,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.evaluator.release();
    }
}

/// A place in the evaluation order, given up when dropped.
struct Turn<'a> {
    evaluator: &'a Evaluator,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        *self.evaluator.serving.lock().unwrap() += 1;
        self.evaluator.turn.notify_all();
    }
}

impl Evaluator {
    pub fn new(api_key: String, concurrency: usize, policy_version: String, cache: Option<Cache>, cancelled: &'static AtomicBool) -> Evaluator {
        let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_millis(TIMEOUT_MS))).http_status_as_error(false).build().into();
        Evaluator {
            api_key,
            endpoint: endpoint(),
            concurrency,
            policy_version,
            cache,
            requests: AtomicUsize::new(0),
            cache_hits: AtomicUsize::new(0),
            cooldown_until: Mutex::new(0.0),
            slots: Mutex::new(Slots::default()),
            released: Condvar::new(),
            authentication_failed: AtomicBool::new(false),
            cancelled,
            agent,
            next_ticket: AtomicU64::new(0),
            serving: Mutex::new(0),
            turn: Condvar::new(),
        }
    }

    /// The next place in the evaluation order.
    pub fn ticket(&self) -> u64 {
        self.next_ticket.fetch_add(1, Ordering::SeqCst)
    }

    fn wait_turn(&self, ticket: u64) -> Turn<'_> {
        let mut serving = self.serving.lock().unwrap();
        while *serving != ticket {
            serving = self.turn.wait(serving).unwrap();
        }
        Turn { evaluator: self }
    }

    pub fn cache_issues(&self) -> Vec<(String, usize)> {
        self.cache.as_ref().map(|cache| cache.issues.lock().unwrap().iter().map(|(k, v)| (k.to_string(), *v)).collect()).unwrap_or_default()
    }

    fn assert_active(&self) -> Result<(), Failure> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(Failure::of(Kind::Cancelled));
        }
        if self.authentication_failed.load(Ordering::SeqCst) {
            return Err(Failure::of(Kind::Authentication));
        }
        Ok(())
    }

    /// Join the queue for a request slot; the returned place is waited on with `acquire`.
    fn enqueue(&self) -> u64 {
        let mut slots = self.slots.lock().unwrap();
        let place = slots.next_waiter;
        slots.next_waiter += 1;
        slots.waiting.push_back(place);
        place
    }

    /// Wait until this place is first in the queue and a slot is free, then take it.
    fn acquire(&self, place: u64) -> Result<(), Failure> {
        let mut slots = self.slots.lock().unwrap();
        loop {
            if let Err(failure) = self.assert_active() {
                slots.waiting.retain(|&waiting| waiting != place);
                self.released.notify_all();
                return Err(failure);
            }
            if slots.waiting.front() == Some(&place) && slots.active < self.concurrency {
                slots.waiting.pop_front();
                slots.active += 1;
                self.released.notify_all();
                return Ok(());
            }
            slots = self.released.wait_timeout(slots, Duration::from_millis(100)).unwrap().0;
        }
    }

    fn release(&self) {
        self.slots.lock().unwrap().active -= 1;
        self.released.notify_all();
    }

    fn namespace(&self) -> Value {
        json!({
            "model": MODEL, "provider": "typesafe", "endpoint": BASE_URL, "protocol": "typesafe-ai-3.0.8",
            "policyVersion": self.policy_version, "parserVersion": PARSER_VERSION, "promptVersion": "unit-locators-1",
        })
    }

    /// Evaluate `{state, questions}` in the order of its ticket. `before_attempt` checks that every
    /// source is still unchanged: before each attempt, and before a cached answer is returned.
    pub fn evaluate(&self, request: &Value, navigation: bool, before_attempt: &dyn Fn() -> Result<(), Failure>, ticket: u64) -> Result<Answers, Failure> {
        let mut turn = Some(self.wait_turn(ticket));
        self.assert_active()?;
        let questions = request["questions"].as_object().cloned().unwrap_or_default();
        let key = self.cache.as_ref().map(|_| cache_key(&self.namespace(), request));
        if let (Some(cache), Some(key)) = (&self.cache, &key) {
            if let Some(cached) = cache.get(key) {
                self.assert_active()?;
                if cached.len() == questions.len() && questions.keys().all(|id| cached.get(id).is_some_and(|p| (0.0..=1.0).contains(p))) {
                    before_attempt()?;
                    self.assert_active()?;
                    self.cache_hits.fetch_add(1, Ordering::SeqCst);
                    return Ok(questions.keys().map(|id| (id.clone(), cached[id])).collect());
                }
            }
        }
        let multiple = questions.len() > 1;
        let mut attempt_limit = if navigation && multiple { 1 } else { 2 };
        let mut attempt = 0;
        while attempt < attempt_limit {
            self.assert_active()?;
            if self.requests.load(Ordering::SeqCst) >= REQUEST_LIMIT {
                return Err(Failure::of(Kind::RequestLimit));
            }
            // Evaluations join the queue in ticket order; a retry joins at the back.
            let place = self.enqueue();
            drop(turn.take());
            self.acquire(place)?;
            let outcome = {
                // The slot is given back even if the attempt panics.
                let _slot = Slot { evaluator: self };
                self.attempt(request, &questions, before_attempt)
            };
            match outcome {
                Ok(answers) => {
                    if let (Some(cache), Some(key)) = (&self.cache, &key) {
                        cache.put(key, &answers);
                    }
                    return Ok(answers);
                }
                Err(Err(failure)) => return Err(failure),
                Err(Ok(error)) => {
                    self.assert_active()?;
                    let status = match &error {
                        Attempt::Status(status, _) => Some(*status),
                        _ => None,
                    };
                    if matches!(status, Some(401 | 403)) {
                        self.authentication_failed.store(true, Ordering::SeqCst);
                        self.released.notify_all();
                        return Err(Failure::of(Kind::Authentication));
                    }
                    if self.requests.load(Ordering::SeqCst) >= REQUEST_LIMIT {
                        return Err(Failure::of(Kind::RequestLimit));
                    }
                    let transient = matches!(status, Some(408 | 429 | 500..=599)) || matches!(error, Attempt::Timeout | Attempt::Network);
                    if navigation && status == Some(429) {
                        attempt_limit = attempt_limit.max(2);
                    }
                    if (navigation && !transient) || attempt + 1 == attempt_limit {
                        let description = match &error {
                            Attempt::Status(status, Some(message)) => format!("HTTP {status}: {message}"),
                            Attempt::Status(status, None) => format!("HTTP {status}"),
                            Attempt::Timeout => format!("Request timed out after {TIMEOUT_MS} ms"),
                            Attempt::Network => "Network request failed (connection unavailable or reset)".into(),
                            Attempt::Invalid => "Invalid or incomplete provider response".into(),
                        };
                        return Err(Failure {
                            kind: Kind::Provider,
                            split_eligible: navigation && multiple && transient && status != Some(429),
                            message: format!("{description} (max concurrent requests: {})", self.concurrency),
                        });
                    }
                }
            }
            attempt += 1;
        }
        Err(Failure::of(Kind::Provider))
    }

    /// One attempt. Ok(Ok) answers, Ok(Err) a provider failure to judge, Err a terminal failure.
    fn attempt(&self, request: &Value, questions: &Map<String, Value>, before_attempt: &dyn Fn() -> Result<(), Failure>) -> Result<Answers, Result<Attempt, Failure>> {
        self.assert_active().map_err(Err)?;
        loop {
            let wait = *self.cooldown_until.lock().unwrap() - js::now();
            if wait <= 0.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis((wait.min(100.0)) as u64 + 1));
            self.assert_active().map_err(Err)?;
        }
        before_attempt().map_err(Err)?;
        self.assert_active().map_err(Err)?;
        if self.requests.load(Ordering::SeqCst) >= REQUEST_LIMIT {
            return Err(Err(Failure::of(Kind::RequestLimit)));
        }
        self.requests.fetch_add(1, Ordering::SeqCst);
        let wire: Map<String, Value> = questions.iter().map(|(id, question)| {
            let mut question = question.as_object().cloned().unwrap_or_default();
            question.insert("type".into(), "noul".into());
            (id.clone(), Value::Object(question))
        }).collect();
        let body = js::stringify(&json!({"model": MODEL, "state": request["state"], "questions": wire}));
        let sent = self.agent.post(&self.endpoint).header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json").send(body.as_str());
        let mut response = match sent {
            Ok(response) => response,
            Err(ureq::Error::Timeout(_)) => return Err(Ok(Attempt::Timeout)),
            Err(_) => return Err(Ok(Attempt::Network)),
        };
        let status = response.status().as_u16();
        if status == 429 {
            let raw = response.headers().get("retry-after").and_then(|value| value.to_str().ok()).map(str::to_string);
            let seconds = js::parse_number(raw.as_deref());
            let date = raw.as_deref().and_then(|raw| chrono::DateTime::parse_from_rfc2822(raw).ok()).map(|d| d.timestamp_millis() as f64);
            let wait = if seconds.is_finite() && seconds >= 0.0 { seconds * 1000.0 } else { date.map_or(1000.0, |date| (date - js::now()).max(0.0)) };
            let mut cooldown = self.cooldown_until.lock().unwrap();
            *cooldown = cooldown.max(js::now() + wait);
        }
        let text = match response.body_mut().read_to_string() {
            Ok(text) => text,
            Err(ureq::Error::Timeout(_)) => return Err(Ok(Attempt::Timeout)),
            Err(_) => return Err(Ok(Attempt::Network)),
        };
        if !(200..300).contains(&status) {
            return Err(Ok(Attempt::Status(status, diagnostic(&text, &self.api_key))));
        }
        let Some(data) = js::parse(&text) else { return Err(Ok(Attempt::Invalid)) };
        let Some(answers) = data.get("answers").and_then(Value::as_object) else { return Err(Ok(Attempt::Invalid)) };
        // The answer keys must be exactly the question keys, each a Noul in [0, 1].
        if answers.len() != questions.len() {
            return Err(Ok(Attempt::Invalid));
        }
        let mut scores = Answers::new();
        for id in questions.keys() {
            let answer = answers.get(id);
            let probability = answer.filter(|a| js::str_of(a, "type") == "noul").and_then(|a| a.get("noul")).and_then(Value::as_f64);
            match probability {
                Some(p) if p.is_finite() && (0.0..=1.0).contains(&p) => scores.insert(id.clone(), p),
                _ => return Err(Ok(Attempt::Invalid)),
            };
        }
        Ok(scores)
    }
}

/// The provider's own error message, cleaned: control and format characters removed, the key
/// redacted (or the message dropped when the key shows up split), at most 500 characters.
fn diagnostic(text: &str, api_key: &str) -> Option<String> {
    let data = js::parse(text)?;
    let nested = data.get("error");
    let message = data.get("message").and_then(Value::as_str)
        .or_else(|| nested.and_then(Value::as_str))
        .or_else(|| nested.and_then(|n| n.get("message")).and_then(Value::as_str))
        .or_else(|| data.get("detail").and_then(Value::as_str))?;
    let mut safe = String::new();
    let mut in_run = false;
    for c in message.chars() {
        let control = c.is_control() || matches!(c, '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}');
        if control {
            if !in_run {
                safe.push(' ');
            }
            in_run = true;
        } else {
            safe.push(c);
            in_run = false;
        }
    }
    let safe = js::trim(&safe);
    let redacted = if api_key.is_empty() { safe.to_string() } else { safe.replace(api_key, "[redacted]") };
    if !api_key.is_empty() && redacted.split_whitespace().collect::<String>().contains(api_key) {
        return None;
    }
    let cut = js::head(&redacted, 500).to_string();
    (!cut.is_empty()).then_some(cut)
}

/// The System One endpoint. A build with the test-api feature takes it from SESSIONKIT_API, so a
/// test can compare the requests of two versions against a local server.
fn endpoint() -> String {
    #[cfg(feature = "test-api")]
    if let Ok(url) = std::env::var("SESSIONKIT_API") {
        return url;
    }
    format!("{BASE_URL}/systemone")
}

fn cache_key(namespace: &Value, request: &Value) -> String {
    js::sha256_hex(js::stringify(&json!([1, namespace, request])).as_bytes())
}

// ---------------------------------------------------------------- the answer cache

const TTL_MS: f64 = 7.0 * 24.0 * 60.0 * 60.0 * 1000.0;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 1024 * 1024;

/// Answer-only storage: validated probabilities by request digest, never source or credentials.
pub struct Cache {
    entries: PathBuf,
    issues: Mutex<indexmap::IndexMap<&'static str, usize>>,
}

impl Cache {
    pub fn new(directory: PathBuf) -> Cache {
        Cache { entries: directory.join("entries"), issues: Mutex::new(indexmap::IndexMap::new()) }
    }

    fn warn(&self, kind: &'static str) {
        *self.issues.lock().unwrap().entry(kind).or_default() += 1;
    }

    fn get(&self, key: &str) -> Option<Answers> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.entries.join(format!("{key}.json"));
        let mut file = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&path) {
            Ok(file) => file,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    self.warn("cache_unavailable");
                }
                return None;
            }
        };
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > MAX_ENTRY_BYTES {
            self.warn("cache_corrupt");
            return None;
        }
        let mut text = String::new();
        use std::io::Read;
        if file.read_to_string(&mut text).is_err() {
            self.warn("cache_corrupt");
            return None;
        }
        let value = js::parse(&text);
        let valid = value.as_ref().filter(|v| v.get("schema").and_then(Value::as_f64) == Some(1.0))
            .filter(|v| v.get("createdAt").and_then(Value::as_f64).is_some_and(f64::is_finite))
            .filter(|v| v.get("answers").and_then(Value::as_object).is_some_and(|a| a.values().all(|p| p.as_f64().is_some_and(f64::is_finite))));
        let Some(value) = valid else {
            self.warn("cache_corrupt");
            return None;
        };
        let age = js::now() - value["createdAt"].as_f64().unwrap_or(f64::NAN);
        if !(0.0..TTL_MS).contains(&age) {
            return None;
        }
        Some(value["answers"].as_object()?.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(f64::NAN))).collect())
    }

    fn put(&self, key: &str, answers: &Answers) {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let payload = js::stringify(&json!({"schema": 1, "createdAt": js::num(js::now()), "answers": answers.iter().map(|(k, v)| (k.clone(), js::num(*v))).collect::<Map<_, _>>()}));
        if payload.len() as u64 > MAX_ENTRY_BYTES {
            return self.warn("cache_limit");
        }
        let written = (|| -> std::io::Result<()> {
            for directory in [self.entries.parent().unwrap(), self.entries.as_path()] {
                std::fs::create_dir_all(directory)?;
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
            }
            let temporary = self.entries.join(format!(".pending-{}", uuid::Uuid::new_v4()));
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o600).open(&temporary)?;
            file.write_all(payload.as_bytes())?;
            drop(file);
            if let Err(error) = std::fs::rename(&temporary, self.entries.join(format!("{key}.json"))) {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
            self.trim()
        })();
        if written.is_err() {
            self.warn("cache_unavailable");
        }
    }

    /// Keep at most 256 MiB, in directory order (not least recently used), and drop publications
    /// left pending for more than an hour.
    fn trim(&self) -> std::io::Result<()> {
        let mut retained = 0;
        for entry in std::fs::read_dir(&self.entries)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let pending = name.starts_with(".pending-");
            if !pending && !(name.len() == 69 && name.ends_with(".json")) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
            if !meta.is_file() {
                continue;
            }
            if pending {
                let old = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
                if old {
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            if retained + meta.len() > MAX_BYTES {
                let _ = std::fs::remove_file(entry.path());
            } else {
                retained += meta.len();
            }
        }
        Ok(())
    }
}
