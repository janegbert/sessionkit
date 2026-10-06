// Jev through TypeSafe's System One: one request, a batch of Nouls against one state, or one
// Choice. post retries as TypeSafe's SDKs do, reads the stored key again from 1Password once
// when it is refused, and counts instead of sending while an estimate runs.

use crate::Result;
use crate::auth::{KeySource, can_refresh, key, refresh_key};
use crate::js::{self, len, num};
use serde_json::{Map, Value, json};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

const API: &str = "https://api.typesafe.ai/v1/systemone";
const CONCURRENCY: usize = 6;

/// SESSIONKIT_MODEL, or JEV_START_MODEL from before the rename; jev-latest by default.
pub fn model() -> String {
    std::env::var("SESSIONKIT_MODEL").or_else(|_| std::env::var("JEV_START_MODEL")).ok()
        .filter(|model| !model.is_empty()).unwrap_or_else(|| "jev-latest".into())
}

pub const CHARS_PER_TOKEN: f64 = 3.5; // measured 3.76 on a real run; lower, so the estimate stays an upper bound
pub const PRICE_PER_MILLION_TOKENS: f64 = 0.042;

/// While estimating, nothing goes to TypeSafe: every request is counted and gets a neutral
/// answer (0.5 for a Noul, an even spread for a Choice). Branches that depend on a score
/// then all open, so the count is an upper bound.
static ESTIMATE: Mutex<Option<(usize, usize)>> = Mutex::new(None);

pub struct Estimate {
    pub chars: usize,
    pub requests: usize,
    pub tokens: f64,
    pub cost_usd: f64,
}

pub fn start_estimate() {
    *ESTIMATE.lock().unwrap() = Some((0, 0));
}

pub fn stop_estimate() -> Estimate {
    let (chars, requests) = ESTIMATE.lock().unwrap().take().unwrap_or((0, 0));
    let tokens = (chars as f64 / CHARS_PER_TOKEN).ceil();
    Estimate { chars, requests, tokens, cost_usd: (tokens / 1e6) * PRICE_PER_MILLION_TOKENS }
}

fn neutral_response(body: &Value, chars: usize) -> Value {
    let mut answers = Map::new();
    for (id, question) in body["questions"].as_object().into_iter().flatten() {
        let answer = if js::str_of(question, "type") != "choice" {
            json!({"type": "noul", "noul": 0.5})
        } else {
            let options: Vec<&String> = question["criteria"].as_object().map(|c| c.keys().collect()).unwrap_or_default();
            let probabilities: Map<String, Value> = options.iter().map(|o| ((*o).clone(), num(1.0 / options.len() as f64))).collect();
            json!({"type": "choice", "choice": options.last(), "probabilities": probabilities, "confidence": 0})
        };
        answers.insert(id.clone(), answer);
    }
    json!({"model": "estimate", "answers": answers, "usage": {"input_tokens": (chars as f64 / CHARS_PER_TOKEN).ceil(), "output_tokens": 0}})
}

fn retryable(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

/// The System One endpoint. A build with the test-api feature takes it from SESSIONKIT_API, so a
/// test can compare the requests of two versions against a local server.
fn api() -> String {
    #[cfg(feature = "test-api")]
    if let Ok(url) = std::env::var("SESSIONKIT_API") {
        return url;
    }
    API.to_string()
}

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).http_status_as_error(false).build().into()
});

/// The wait before a retry: from half a second doubling to five, less up to a quarter at random,
/// so parallel requests that failed together do not come back together.
fn backoff(attempt: u32) -> Duration {
    let base = (500 * 2u64.pow(attempt)).min(5000) as f64;
    let jitter = (uuid::Uuid::new_v4().as_u128() % 1000) as f64 / 1000.0;
    Duration::from_millis((base * (1.0 - 0.25 * jitter)) as u64)
}

/// The wait the server asks for in Retry-After (seconds), when it names one of at most a minute.
fn retry_after(response: &ureq::http::Response<ureq::Body>) -> Option<Duration> {
    let seconds = js::parse_number(response.headers().get("retry-after").and_then(|value| value.to_str().ok()));
    (seconds.is_finite() && seconds > 0.0 && seconds <= 60.0).then(|| Duration::from_millis((seconds * 1000.0) as u64))
}

/// One System One call, with the default retry policy of TypeSafe's SDKs: two retries with
/// backoff and jitter, or the server's Retry-After when it sends one.
pub fn post(body: &Value) -> Result<Value> {
    let text = js::stringify(body);
    {
        let mut estimate = ESTIMATE.lock().unwrap();
        if let Some((chars, requests)) = estimate.as_mut() {
            let size = len(&text);
            *chars += size;
            *requests += 1;
            return Ok(neutral_response(body, size));
        }
    }
    let mut attempt = 0;
    loop {
        let (api_key, source) = key()?;
        let sent = AGENT.post(api()).header("Authorization", &format!("Bearer {api_key}"))
            .header("Content-Type", "application/json").send(text.as_str());
        let mut response = match sent {
            Ok(response) => response,
            Err(error) => {
                if attempt < 2 {
                    std::thread::sleep(backoff(attempt));
                    attempt += 1;
                    continue;
                }
                return Err(error.to_string());
            }
        };
        let status = response.status().as_u16();
        let wait = retry_after(&response);
        let reply = response.body_mut().read_to_string().map_err(|error| error.to_string())?;
        if (200..300).contains(&status) {
            return js::parse(&reply).ok_or_else(|| format!("TypeSafe {status}: not JSON: {}", js::head(&reply, 200)));
        }
        if status == 401 && source == KeySource::Keychain && !can_refresh() {
            return Err("The stored TypeSafe key was refused. Run sessionkit auth login to replace it, or set TYPESAFE_API_KEY.".into());
        }
        if status == 401 && source == KeySource::Keychain && can_refresh() {
            eprint!("sessionkit: the stored key was refused; reading it again from 1Password.\n");
            refresh_key()?;
            attempt += 1;
            continue;
        }
        if attempt < 2 && retryable(status) {
            std::thread::sleep(wait.unwrap_or_else(|| backoff(attempt)));
            attempt += 1;
            continue;
        }
        return Err(format!("TypeSafe {status}: {}", js::head(&reply, 200)));
    }
}

/// Ask one Noul per question against the same state. Questions are split into requests that fit
/// the budget; every request repeats the state. Returns one probability per question and the
/// input tokens.
pub fn ask_nouls(state: &Value, questions: &[Value], budget: usize) -> Result<(Vec<f64>, f64)> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let (mut batch, mut size) = (Vec::new(), 0);
    for (i, question) in questions.iter().enumerate() {
        let length = len(&js::stringify(question));
        if !batch.is_empty() && size + length > budget {
            batches.push(std::mem::take(&mut batch));
            size = 0;
        }
        batch.push(i);
        size += length;
    }
    if !batch.is_empty() {
        batches.push(batch);
    }

    let model = model();
    let results = js::pool(&batches, CONCURRENCY, |indexes, _| -> Result<(Vec<(usize, f64)>, f64)> {
        let mut asked = Map::new();
        for &i in indexes {
            let mut question = Map::new();
            question.insert("type".into(), "noul".into());
            question.extend(questions[i].as_object().cloned().unwrap_or_default());
            asked.insert(format!("q{i}"), Value::Object(question));
        }
        let data = post(&json!({"model": model, "state": state, "questions": asked}))?;
        let answers = indexes.iter().map(|&i| {
            (i, data.get("answers").and_then(|a| a.get(format!("q{i}"))).and_then(|a| a.get("noul")).and_then(Value::as_f64).unwrap_or(0.0))
        }).collect();
        Ok((answers, data.get("usage").and_then(|u| u.get("input_tokens")).and_then(Value::as_f64).unwrap_or(0.0)))
    });
    let mut probabilities = vec![0.0; questions.len()];
    let mut tokens = 0.0;
    for result in results {
        let (answers, used) = result?;
        tokens += used;
        for (i, p) in answers {
            probabilities[i] = p;
        }
    }
    Ok((probabilities, tokens))
}

/// One Choice question against a state. Returns the answer with its probabilities.
pub fn ask_choice(state: &Value, question: Value) -> Result<(Option<Value>, f64)> {
    let mut asked = Map::new();
    asked.insert("type".into(), "choice".into());
    asked.extend(question.as_object().cloned().unwrap_or_default());
    let data = post(&json!({"model": model(), "state": state, "questions": {"q": asked}}))?;
    let answer = data.get("answers").and_then(|a| a.get("q")).cloned().filter(|a| !a.is_null());
    Ok((answer, data.get("usage").and_then(|u| u.get("input_tokens")).and_then(Value::as_f64).unwrap_or(0.0)))
}
