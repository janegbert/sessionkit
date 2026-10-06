// What JavaScript does, where the result must match the JavaScript version byte for byte.
//
// String lengths and cuts count UTF-16 code units, as JavaScript's .length and .slice do: the
// request budgets and the clipped texts depend on them. JSON is written as JSON.stringify writes
// it (3, not 3.0; key order kept), so Jev gets the same requests. Numbers are formatted as
// toFixed and toString format them.

use serde_json::Value;
use std::fmt::Write as _;

// ---------------------------------------------------------------- strings

/// The length of a string in UTF-16 code units: JavaScript's .length.
pub fn len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The first `max` UTF-16 code units of a string: JavaScript's .slice(0, max). A surrogate pair
/// that the cut would split is left out whole; JavaScript would keep its first half.
pub fn head(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &text[..index];
        }
    }
    text
}

/// JavaScript's .slice(start, end) in UTF-16 units. A surrogate pair that a cut would split is
/// left out whole.
pub fn slice(text: &str, start: usize, end: usize) -> &str {
    let from = head(text, start).len();
    let to = head(text, end.max(start)).len();
    &text[from..to]
}

/// String.prototype.localeCompare with the default (root) collation, as V8 compares.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    static COLLATOR: std::sync::LazyLock<icu_collator::CollatorBorrowed<'static>> =
        std::sync::LazyLock::new(|| icu_collator::CollatorBorrowed::try_new(Default::default(), Default::default()).expect("baked collation data"));
    COLLATOR.compare(a, b)
}

/// The SHA-256 of some bytes, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Cut a text to `max` units and mark the cut.
pub fn clip(text: &str, max: usize) -> String {
    if len(text) > max { format!("{} […]", head(text, max)) } else { text.to_string() }
}

/// Cut a text to `max` units and mark the cut, but keep its end too: what a conclusion proposes
/// sits at the end, where `clip` would drop it. Two thirds of the budget go to the head.
pub fn clip_middle(text: &str, max: usize) -> String {
    if len(text) <= max {
        return text.to_string();
    }
    let keep_head = max * 2 / 3;
    format!("{} […] {}", head(text, keep_head), tail(text, max - keep_head))
}

/// The last `max` UTF-16 code units of a string. A surrogate pair that the cut would split is
/// left out whole.
pub fn tail(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (index, c) in text.char_indices().rev() {
        units += c.len_utf16();
        if units > max {
            return &text[index + c.len_utf8()..];
        }
    }
    text
}

/// JavaScript's .padEnd(width) after .slice(0, width).
pub fn pad(text: &str, width: usize) -> String {
    let cut = head(text, width);
    format!("{cut}{}", " ".repeat(width - len(cut)))
}

/// JavaScript's .padStart(width).
pub fn lpad(text: &str, width: usize) -> String {
    format!("{}{text}", " ".repeat(width.saturating_sub(len(text))))
}

fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// JavaScript's .trim(): also removes the byte order mark.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

pub fn trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_space)
}

/// JavaScript's text.replace(/\s+/g, " ").
pub fn collapse_space(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

// ---------------------------------------------------------------- numbers

/// Math.round: halves go up, also for negative numbers.
pub fn round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// Number.prototype.toString.
pub fn number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if value == 0.0 {
        return "0".into();
    }
    let size = value.abs();
    if (1e-6..1e21).contains(&size) {
        return format!("{value}");
    }
    let text = format!("{value:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
        _ => text,
    }
}

/// Number.prototype.toFixed: the exact value, rounded half up, as JavaScript does. Rust's own
/// formatting rounds a tie to even (0.125 to 0.12 where JavaScript writes 0.13).
pub fn fixed(value: f64, digits: usize) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.abs() >= 1e21 {
        return number(value);
    }
    let exact = format!("{:.1100}", value.abs());
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits_kept: Vec<u8> = whole.bytes().chain(fraction.bytes().take(digits)).collect();
    if fraction.as_bytes().get(digits).is_some_and(|&next| next >= b'5') {
        let mut i = digits_kept.len();
        loop {
            if i == 0 {
                digits_kept.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits_kept[i] == b'9' {
                digits_kept[i] = b'0';
            } else {
                digits_kept[i] += 1;
                break;
            }
        }
    }
    let text = String::from_utf8(digits_kept).unwrap();
    let split = text.len() - digits;
    let sign = if value < 0.0 { "-" } else { "" };
    if digits == 0 { format!("{sign}{text}") } else { format!("{sign}{}.{}", &text[..split], &text[split..]) }
}

/// toLocaleString("en") of a whole number: thousands separated by commas.
pub fn grouped(value: f64) -> String {
    let text = number(value.abs());
    let (whole, fraction) = text.split_once('.').map_or((text.as_str(), None), |(w, f)| (w, Some(f)));
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(fraction) = fraction {
        let _ = write!(out, ".{}", &fraction[..fraction.len().min(3)]);
    }
    if value < 0.0 { format!("-{out}") } else { out }
}

/// A number setting: the fallback when the text is missing, empty or not a number, so a typo
/// does not turn a check off (NaN) or on for everything (0).
pub fn number_or(text: Option<&str>, fallback: f64) -> f64 {
    let value = text.filter(|text| !trim(text).is_empty()).map_or(f64::NAN, |text| parse_number(Some(text)));
    if value.is_nan() { fallback } else { value }
}

/// Number(text): a number, NaN when the text is not one, 0 for an empty text.
pub fn parse_number(text: Option<&str>) -> f64 {
    let Some(text) = text else { return f64::NAN };
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ if text.starts_with("0x") || text.starts_with("0X") => {
            u64::from_str_radix(&text[2..], 16).map_or(f64::NAN, |value| value as f64)
        }
        _ if text.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c)) => text.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// JavaScript truthiness of a number: not 0 and not NaN.
pub fn truthy_number(value: f64) -> bool {
    value != 0.0 && !value.is_nan()
}

// ---------------------------------------------------------------- time

/// Date.now().
pub fn now() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

/// Date.parse of an ISO timestamp, NaN when it is not one.
pub fn parse_date(text: &str) -> f64 {
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return time.timestamp_millis() as f64;
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis() as f64;
    }
    f64::NAN
}

/// Date.prototype.toISOString.
pub fn iso(millis: f64) -> String {
    chrono::DateTime::from_timestamp_millis(millis as i64)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

/// new Date().toISOString().replace(/[:.]/g, "-"): a timestamp for a file name.
pub fn file_stamp() -> String {
    iso(now()).replace([':', '.'], "-")
}

// ---------------------------------------------------------------- values

/// JavaScript truthiness of a JSON value.
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(n)) => n.as_f64().is_some_and(truthy_number),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// A string field, or "" when it is missing or not a string.
pub fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A number field with `?? 0`: 0 only when it is missing or null.
pub fn num_of(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A number as a JSON value; NaN and infinities become null, as JSON.stringify writes them.
pub fn num(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// An optional number as a JSON value.
pub fn opt(value: Option<f64>) -> Value {
    value.map_or(Value::Null, num)
}

/// JSON.parse of one line. A lone UTF-16 surrogate, which JavaScript accepts and serde_json does
/// not, becomes U+FFFD, so the rest of the line is not lost.
pub fn parse(line: &str) -> Option<Value> {
    match serde_json::from_str(line) {
        Ok(value) => Some(value),
        Err(_) if line.contains("\\u") => serde_json::from_str(&without_lone_surrogates(line)).ok(),
        Err(_) => None,
    }
}

fn surrogate_at(bytes: &[u8], at: usize) -> Option<u16> {
    let hex = bytes.get(at..at + 6)?;
    if hex[0] != b'\\' || hex[1] != b'u' {
        return None;
    }
    let code = u16::from_str_radix(std::str::from_utf8(&hex[2..]).ok()?, 16).ok()?;
    (0xd800..0xe000).contains(&code).then_some(code)
}

fn without_lone_surrogates(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut copied = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        match surrogate_at(bytes, i) {
            Some(high) if high < 0xdc00 && surrogate_at(bytes, i + 6).is_some_and(|low| low >= 0xdc00) => i += 12,
            Some(_) => {
                out.push_str(&line[copied..i]);
                out.push_str("\\ufffd");
                i += 6;
                copied = i;
            }
            None => i += 2,
        }
    }
    out.push_str(&line[copied..]);
    out
}

/// JSON.stringify(value).
pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, None, 0);
    out
}

/// JSON.stringify(value, null, 2).
pub fn stringify_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, Some("  "), 0);
    out
}

/// JSON.stringify of a string.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    write_string(&mut out, text);
    out
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Value, indent: Option<&str>, depth: usize) {
    let newline = |out: &mut String, depth: usize| {
        if let Some(indent) = indent {
            out.push('\n');
            out.push_str(&indent.repeat(depth));
        }
    };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let _ = write!(out, "{i}");
            } else if let Some(u) = n.as_u64() {
                let _ = write!(out, "{u}");
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                out.push_str(&if f.is_finite() { number(f) } else { "null".into() });
            }
        }
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write_value(out, item, indent, depth + 1);
            }
            newline(out, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write_string(out, key);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, item, indent, depth + 1);
            }
            newline(out, depth);
            out.push('}');
        }
    }
}

// ---------------------------------------------------------------- files and terminals

/// The names in a folder, sorted as Node's readdirSync sorts them. Empty when it cannot be read.
pub fn read_dir_sorted(path: &std::path::Path) -> Vec<std::fs::DirEntry> {
    let mut entries: Vec<_> = std::fs::read_dir(path).map(|dir| dir.flatten().collect()).unwrap_or_default();
    entries.sort_by_key(|entry| entry.file_name());
    entries
}

/// The names in a folder, sorted; an error when the folder cannot be read, as readdirSync throws.
pub fn read_dir_names(path: &std::path::Path) -> Result<Vec<String>, String> {
    let dir = std::fs::read_dir(path).map_err(|error| format!("{error}, scandir '{}'", path.display()))?;
    let mut names: Vec<String> = dir.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Ok(names)
}

/// statSync(path).mtimeMs.
pub fn mtime_ms(path: &std::path::Path) -> Option<f64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(since.as_secs_f64() * 1000.0)
}

pub fn stdin_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Ask one question on stderr and read the answer from the terminal, trimmed.
pub fn ask(question: &str) -> String {
    eprint!("{question}");
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    trim(&answer).to_string()
}

/// Run tasks on a fixed number of threads; the results keep the order of the items.
pub fn pool<T: Sync, R: Send>(items: &[T], concurrency: usize, task: impl Fn(&T, usize) -> R + Sync) -> Vec<R> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..concurrency.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= items.len() {
                        break;
                    }
                    let result = task(&items[i], i);
                    results.lock().unwrap()[i] = Some(result);
                }
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(Option::unwrap).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_setting_that_is_no_number_keeps_its_default() {
        assert_eq!(number_or(Some("500000"), 375_000.0), 500_000.0);
        assert_eq!(number_or(Some(" 2.5 "), 1.0), 2.5);
        assert_eq!(number_or(Some(""), 375_000.0), 375_000.0);
        assert_eq!(number_or(Some("abc"), 375_000.0), 375_000.0);
        assert_eq!(number_or(None, 1.0), 1.0);
        assert_eq!(number_or(Some("Infinity"), 1.0), f64::INFINITY);
    }

    #[test]
    fn fixed_rounds_ties_up_like_javascript() {
        assert_eq!(fixed(0.125, 2), "0.13");
        assert_eq!(fixed(1.005, 2), "1.00"); // 1.005 is 1.00499999… in binary
        assert_eq!(fixed(2.5, 0), "3");
        assert_eq!(fixed(9.999, 2), "10.00");
        assert_eq!(fixed(-0.001, 2), "-0.00");
        assert_eq!(fixed(0.0, 3), "0.000");
    }

    #[test]
    fn numbers_print_like_javascript() {
        assert_eq!(number(3.0), "3");
        assert_eq!(number(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number(1e-7), "1e-7");
        assert_eq!(number(1.5e21), "1.5e+21");
        assert_eq!(number(123456789012.0), "123456789012");
        assert_eq!(grouped(1234567.0), "1,234,567");
        assert_eq!(grouped(999.0), "999");
        assert_eq!(round(-2.5), -2.0);
        assert_eq!(round(0.49999999999999994), 0.0);
    }

    #[test]
    fn stringify_matches_javascript() {
        let value = json!({"b": 1.0, "a": [1, "x\u{1}\n"], "c": {}, "d": [], "e": 0.5});
        assert_eq!(stringify(&value), r#"{"b":1,"a":[1,"x\u0001\n"],"c":{},"d":[],"e":0.5}"#);
        assert_eq!(stringify_pretty(&json!({"a": [1], "b": {}})), "{\n  \"a\": [\n    1\n  ],\n  \"b\": {}\n}");
        assert_eq!(num(f64::NAN), Value::Null);
    }

    #[test]
    fn lengths_count_utf16_units() {
        assert_eq!(len("a😀"), 3);
        assert_eq!(head("a😀b", 2), "a");
        assert_eq!(head("a😀b", 3), "a😀");
        assert_eq!(clip("abcdef", 3), "abc […]");
        assert_eq!(tail("a😀b", 2), "b");
        assert_eq!(tail("a😀b", 3), "😀b");
        assert_eq!(clip_middle("abcdef", 6), "abcdef");
        assert_eq!(clip_middle("abcdefghijkl", 6), "abcd […] kl");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(lpad("ab", 4), "  ab");
    }

    #[test]
    fn lone_surrogates_do_not_lose_the_line() {
        let value = parse(r#"{"a":"x\ud83d y","b":"😀","c":"\\ud83d"}"#).unwrap();
        assert_eq!(value["a"], "x\u{fffd} y");
        assert_eq!(value["b"], "😀");
        assert_eq!(value["c"], "\\ud83d");
    }

    #[test]
    fn number_parses_like_javascript() {
        assert_eq!(parse_number(Some("7")), 7.0);
        assert_eq!(parse_number(Some("")), 0.0);
        assert!(parse_number(Some("abc")).is_nan());
        assert!(parse_number(None).is_nan());
    }
}
