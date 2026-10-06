// One entry of a Claude Code transcript (a line of its .jsonl file), decoded. The rules of the
// format live here: which entries are API calls, how a call records its tokens, what a
// compaction boundary is, which entries belong to a subagent. The readers in cache, usage,
// compact, start and claude ask an entry instead of reading its JSON fields themselves.

use crate::js;
use serde_json::Value;

pub struct Entry {
    value: Value,
}

/// What one API call recorded in its `usage`.
#[derive(Clone)]
pub struct ApiCall {
    /// The message id, or the request id when there is none; a response written as several
    /// lines repeats it.
    pub id: Option<String>,
    pub model: Option<String>,
    pub fast: bool,
    pub input: f64,
    pub read: f64,
    pub write5m: f64,
    pub write1h: f64,
    pub output: f64,
}

impl ApiCall {
    pub fn write(&self) -> f64 {
        self.write5m + self.write1h
    }

    /// The whole context the call sent: uncached input, cache reads and cache writes.
    pub fn context(&self) -> f64 {
        self.input + self.read + self.write()
    }
}

/// A compaction boundary: the context before it, and what it left.
pub struct Compaction {
    pub before: f64,
    pub after: f64,
}

impl Entry {
    /// None for a line that is not JSON, such as the first line of a tail that cut it.
    pub fn parse(line: &str) -> Option<Entry> {
        js::parse(line).map(|value| Entry { value })
    }

    /// A quick test on the raw line, before parsing: false means it is surely no API call.
    pub fn may_be_call(line: &str) -> bool {
        line.contains("\"usage\"")
    }

    /// A quick test on the raw line, before parsing: false means it is surely no compaction.
    pub fn may_be_compaction(line: &str) -> bool {
        line.contains("\"compact_boundary\"")
    }

    /// The entry's `type`: user, assistant, system, ai-title, summary and others.
    pub fn kind(&self) -> &str {
        js::str_of(&self.value, "type")
    }

    /// Written by a subagent into the main transcript (older Claude Code versions did this).
    pub fn is_sidechain(&self) -> bool {
        js::truthy(self.value.get("isSidechain"))
    }

    pub fn timestamp(&self) -> &str {
        js::str_of(&self.value, "timestamp")
    }

    /// The timestamp in epoch milliseconds; NaN when there is none.
    pub fn time(&self) -> f64 {
        js::parse_date(self.timestamp())
    }

    /// The fields the readers decode themselves: message content, titles, cwd and others.
    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn compaction(&self) -> Option<Compaction> {
        if js::str_of(&self.value, "subtype") != "compact_boundary" {
            return None;
        }
        let metadata = self.value.get("compactMetadata").cloned().unwrap_or(Value::Null);
        Some(Compaction { before: js::num_of(&metadata, "preTokens"), after: js::num_of(&metadata, "postTokens") })
    }

    /// An assistant entry that records the usage of an API call. Claude Code writes
    /// "<synthetic>" messages itself; they are no API calls.
    pub fn api_call(&self) -> Option<ApiCall> {
        if self.kind() != "assistant" {
            return None;
        }
        let message = self.value.get("message").filter(|m| !m.is_null())?;
        let usage = message.get("usage").filter(|u| js::truthy(Some(u)))?;
        let model = message.get("model").and_then(Value::as_str);
        if model == Some("<synthetic>") {
            return None;
        }
        let id = message.get("id").filter(|v| !v.is_null()).or_else(|| self.value.get("requestId")).filter(|v| js::truthy(Some(v)));
        let write = js::num_of(usage, "cache_creation_input_tokens");
        let split = usage.get("cache_creation");
        let write1h = split.map_or(0.0, |split| js::num_of(split, "ephemeral_1h_input_tokens"));
        Some(ApiCall {
            id: id.map(|id| id.as_str().map_or_else(|| id.to_string(), str::to_string)),
            model: model.map(str::to_string),
            fast: usage.get("speed").and_then(Value::as_str) == Some("fast"),
            input: js::num_of(usage, "input_tokens"),
            read: js::num_of(usage, "cache_read_input_tokens"),
            write5m: split.and_then(|split| split.get("ephemeral_5m_input_tokens")).and_then(Value::as_f64).unwrap_or(write - write1h),
            write1h,
            output: js::num_of(usage, "output_tokens"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(value: Value) -> Entry {
        Entry::parse(&value.to_string()).unwrap()
    }

    #[test]
    fn a_call_splits_its_cache_writes_by_lifetime() {
        let call = entry(json!({"type": "assistant", "message": {"id": "m1", "model": "claude-opus-5-5", "usage": {
            "input_tokens": 3, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 50, "output_tokens": 7,
            "cache_creation": {"ephemeral_1h_input_tokens": 40}}}})).api_call().unwrap();
        assert_eq!((call.write5m, call.write1h, call.context()), (10.0, 40.0, 153.0));
        assert_eq!(call.id.as_deref(), Some("m1"));
    }

    #[test]
    fn a_synthetic_message_is_no_api_call() {
        let synthetic = entry(json!({"type": "assistant", "message": {"model": "<synthetic>", "usage": {"input_tokens": 0}}}));
        assert!(synthetic.api_call().is_none());
        let user = entry(json!({"type": "user", "message": {"usage": {"input_tokens": 1}}}));
        assert!(user.api_call().is_none());
    }

    #[test]
    fn a_boundary_records_the_context_before_and_after() {
        let boundary = entry(json!({"type": "system", "subtype": "compact_boundary",
            "compactMetadata": {"preTokens": 912_000, "postTokens": 39_000}}));
        let compaction = boundary.compaction().unwrap();
        assert_eq!((compaction.before, compaction.after), (912_000.0, 39_000.0));
        assert!(entry(json!({"type": "system"})).compaction().is_none());
    }
}
