// The cache state of one Claude Code session, read from the end of its transcript: how large
// the context is, what one more call costs in cache reads, how long the cache stays warm, and
// what the first call costs once it has gone cold.
//
// Used by the status line and by the warning before a cold call. It reads only the last part of
// the transcript, so it stays fast on a transcript of tens of megabytes.

use crate::js;
use crate::pricing::{Rate, Ttl};
use crate::transcript::Entry;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// The last `limit` bytes of a file as text; a character the cut splits becomes U+FFFD.
pub fn read_tail(path: &Path, limit: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let length = size.min(limit);
    file.seek(SeekFrom::Start(size - length))?;
    let mut buffer = Vec::with_capacity(length as usize);
    file.take(length).read_to_end(&mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

struct LastCall {
    time: f64,
    model: Option<String>,
    fast: bool,
    context: f64,
    read: f64,
    write: f64,
    ttl: Ttl,
    /// A compaction came after the call: nothing is cached, and the context is what it left.
    compacted: bool,
}

/// The last API call of the main conversation, and whether this session writes its cache for
/// one hour or for five minutes. Subagent calls live in other files and are not counted here.
/// After a compaction the call's context no longer holds: the next call writes what the
/// compaction left, so that counts, from the time of the compaction.
fn last_call(transcript: &Path) -> std::io::Result<Option<LastCall>> {
    let text = read_tail(transcript, TAIL_BYTES)?;
    let mut last: Option<LastCall> = None;
    let mut compaction: Option<(f64, f64)> = None;
    let mut one_hour = false;
    for line in text.split('\n').rev() {
        if last.is_none() && compaction.is_none() && Entry::may_be_compaction(line) {
            let entry = Entry::parse(line).filter(|e| !e.is_sidechain());
            if let Some((entry, after)) = entry.and_then(|e| e.compaction().map(|c| (e, c.after))).filter(|&(_, after)| after > 0.0) {
                compaction = Some((entry.time(), after));
            }
            continue;
        }
        if !Entry::may_be_call(line) {
            continue;
        }
        let Some(entry) = Entry::parse(line) else { continue }; // the first line of the tail can be cut
        let Some(call) = entry.api_call().filter(|_| !entry.is_sidechain()) else { continue };
        if call.write1h > 0.0 {
            one_hour = true;
        }
        if last.is_none() {
            let (time, context) = compaction.unwrap_or((entry.time(), call.context()));
            last = Some(LastCall {
                time,
                fast: call.fast,
                context,
                read: if compaction.is_some() { 0.0 } else { call.read },
                write: if compaction.is_some() { context } else { call.write() },
                ttl: Ttl::FiveMinutes,
                compacted: compaction.is_some(),
                model: call.model,
            });
        }
        if last.is_some() && one_hour {
            break;
        }
    }
    Ok(last.map(|call| LastCall { ttl: if one_hour { Ttl::OneHour } else { Ttl::FiveMinutes }, ..call }))
}

#[derive(Clone)]
pub struct CacheState {
    pub context: f64,
    pub model: Option<String>,
    pub last_call_time: f64,
    pub last_call_cold: bool,
    /// None when no transcript says it.
    pub ttl: Option<Ttl>,
    pub warm: bool,
    pub left: f64,
    pub per_call: Option<f64>,
    pub cold_call: Option<f64>,
}

/// The cache state now. per_call is the cache-read cost of one more call at this context;
/// cold_call is what the first call costs when the cache has expired: the whole context written
/// again.
pub fn cache_state(transcript: &Path) -> Option<CacheState> {
    let call = last_call(transcript).ok()??;
    let rate = Rate::of(call.model.as_deref(), call.fast);
    let left = if call.compacted { 0.0 } else { call.time + call.ttl.ms() - js::now() };
    Some(CacheState {
        context: call.context,
        model: call.model,
        last_call_time: call.time,
        last_call_cold: call.write > call.read,
        ttl: Some(call.ttl),
        warm: left > 0.0,
        left: if left.is_nan() { f64::NAN } else { left.max(0.0) },
        per_call: rate.map(|r| r.read(call.context)),
        cold_call: rate.map(|r| r.write(call.context, call.ttl)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn state_of(name: &str, lines: &[Value]) -> CacheState {
        let path = std::env::temp_dir().join(format!("sessionkit-cache-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&path, lines.iter().map(|line| line.to_string() + "\n").collect::<String>()).unwrap();
        let state = cache_state(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        state
    }

    fn call(context: f64) -> Value {
        json!({"type": "assistant", "timestamp": "2026-09-30T09:00:00Z", "message": {"model": "claude-opus-5-5",
            "usage": {"input_tokens": 10, "cache_read_input_tokens": context - 10.0, "cache_creation_input_tokens": 0}}})
    }

    #[test]
    fn the_last_call_sets_the_context() {
        assert_eq!(state_of("call", &[call(912_000.0)]).context, 912_000.0);
    }

    #[test]
    fn a_later_compaction_sets_the_context_and_empties_the_cache() {
        let boundary = json!({"type": "system", "subtype": "compact_boundary", "timestamp": "2026-09-30T09:53:43Z",
            "compactMetadata": {"trigger": "manual", "preTokens": 912_000, "postTokens": 39_000}});
        let state = state_of("compacted", &[call(912_000.0), boundary]);
        assert_eq!(state.context, 39_000.0);
        assert!(!state.warm);
        assert!(state.last_call_cold);
    }
}
