// One search (jevgrep's core/retrieve.ts): discover relevant files breadth first with a two-level
// lookahead, reconsider pruned folders once against a class the best file declares, select
// declarations in two passes while file roles are assessed, then shape what is shown. Every
// admitted file stays in the result, and anything that could not be judged is reported.
//
// jevgrep runs this as concurrent async tasks; here the stages run on threads that share one
// state. Where jevgrep's order depends on which request finishes first, so does this one.

use super::evaluator::{Answers, Evaluator, Failure, Kind};
use super::fs::{Cursor, Lookup, Reader, Snapshot, SnapshotResult};
use super::requests::{Anchor, NavigationItem, file_assessment_request, navigation_request, test_body_request};
use super::selection::{CallLead, EvidenceRange, Excerpt, FileEvidence, Selection, select_file};
use super::source::{self, Bounds, Mode, Range};
use super::python;
use crate::js;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const STAGE_WORKERS: usize = 32;

#[derive(Clone)]
pub struct Candidate {
    pub path: String,
    pub content_hash: String,
    pub score: f64,
}

/// A navigation item and the snapshots its request must still match.
#[derive(Clone)]
struct Item {
    item: NavigationItem,
    donors: Vec<Candidate>,
}

#[derive(Default)]
struct State {
    issues: indexmap::IndexMap<String, usize>,
    provider_failure: Option<String>,
    inspected: HashSet<String>,
    candidates: indexmap::IndexMap<String, Candidate>,
    files: indexmap::IndexMap<String, FileEvidence>,
    declarations: indexmap::IndexMap<String, Vec<(String, Range)>>,
    visited: HashSet<String>,
    pruned: indexmap::IndexMap<String, Item>,
    previews: HashMap<String, Value>,
    entries_seen: usize,
}

pub struct Outcome {
    pub status: &'static str,
    pub files: Vec<FileEvidence>,
    pub issues: Vec<(String, usize)>,
    pub warnings: Vec<(String, usize)>,
    pub provider_failure: Option<String>,
    pub instruction_files: Vec<String>,
    pub instruction_lookup_incomplete: bool,
    pub pytest_files: Vec<String>,
}

struct Search<'a> {
    reader: &'a Reader,
    evaluator: &'a Evaluator,
    query: &'a str,
    state: Mutex<State>,
    stop: AtomicBool,
    validation: Mutex<()>,
    cancelled: &'static AtomicBool,
}

fn byte_length(value: &Value) -> usize {
    js::stringify(value).len()
}

/// Node's path.extname.
fn extname(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(0) | None => String::new(),
        _ if name == ".." => String::new(),
        Some(at) => name[at..].to_string(),
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Node's path.dirname for root-relative paths.
fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(at) => &path[..at],
        None => ".",
    }
}

impl<'a> Search<'a> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.cancelled.load(Ordering::SeqCst)
    }

    fn issue(&self, kind: &str, count: usize, message: Option<String>) {
        let mut state = self.state.lock().unwrap();
        if kind == "provider" && state.provider_failure.is_none() {
            state.provider_failure = message;
        }
        *state.issues.entry(kind.to_string()).or_default() += count;
        if matches!(kind, "authentication" | "request-limit" | "cancelled" | "interrupted") {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    fn snapshot(&self, path: &str) -> Option<Arc<Snapshot>> {
        match self.reader.read_snapshot(path) {
            SnapshotResult::Ok(snapshot) => {
                self.state.lock().unwrap().inspected.insert(path.to_string());
                Some(snapshot)
            }
            SnapshotResult::Excluded => None,
            SnapshotResult::Issue(kind) => {
                self.issue(kind.name(), 1, None);
                None
            }
        }
    }

    /// The current snapshot when it still has the admitted content; else the file's evidence is
    /// cleared and the change is reported.
    fn unchanged(&self, candidate: &Candidate) -> Option<Arc<Snapshot>> {
        let result = self.reader.read_snapshot(&candidate.path);
        if let SnapshotResult::Ok(snapshot) = &result {
            if snapshot.content_hash == candidate.content_hash {
                self.state.lock().unwrap().inspected.insert(candidate.path.clone());
                return Some(snapshot.clone());
            }
        }
        let kind = match &result {
            SnapshotResult::Issue(kind) => kind.name(),
            _ => "changed",
        };
        self.issue(kind, 1, None);
        if kind == "interrupted" {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        if let Some(prior) = state.files.get_mut(&candidate.path) {
            prior.roles.clear();
            prior.priority = None;
            prior.presentation_excerpts = Some(Vec::new());
            prior.selected_presentation_excerpts = Some(Vec::new());
            prior.call_leads.clear();
            prior.presentation_selected = Some(Vec::new());
            prior.source_decisions.clear();
            prior.leads.clear();
            prior.selected.clear();
            prior.rendered.clear();
            prior.excerpts.clear();
            prior.source_omitted = true;
        }
        None
    }

    /// Evaluate after checking, in request order, that every source is unchanged; checked again
    /// before each attempt.
    fn fresh_evaluation(&self, request: &Value, sources: &[Candidate], navigation: bool) -> Result<Answers, Failure> {
        let ticket = self.evaluator.ticket();
        self.fresh_evaluation_in_turn(request, sources, navigation, ticket)
    }

    fn fresh_evaluation_in_turn(&self, request: &Value, sources: &[Candidate], navigation: bool, ticket: u64) -> Result<Answers, Failure> {
        let mut unique: indexmap::IndexMap<&str, &Candidate> = indexmap::IndexMap::new();
        for source in sources {
            unique.insert(&source.path, source);
        }
        let validate = || -> Result<(), Failure> {
            let _order = self.validation.lock().unwrap();
            for source in unique.values() {
                if self.unchanged(source).is_none() {
                    return Err(Failure::of(Kind::SourceInvalid));
                }
            }
            Ok(())
        };
        self.evaluator.evaluate(request, navigation, &validate, ticket)
    }

    fn candidate(&self, path: &str) -> Option<Candidate> {
        self.state.lock().unwrap().candidates.get(path).cloned()
    }

    /// Navigation scores in completion order. A group that fails with changed sources, or with a
    /// transient provider failure, is split in half and both halves join the same queue.
    fn score(&self, items: Vec<Item>, anchor: Option<(&Anchor, &Candidate)>) -> Vec<(Item, f64)> {
        let anchor_only = anchor.map(|(anchor, _)| anchor);
        let mut batches: VecDeque<Vec<Item>> = VecDeque::new();
        let mut batch: Vec<Item> = Vec::new();
        for item in items {
            if byte_length(&navigation_request(self.query, &[&item.item], anchor_only)) > 38_000 {
                self.issue("request-size", 1, None);
                continue;
            }
            if !batch.is_empty() {
                let mut with: Vec<&NavigationItem> = batch.iter().map(|i| &i.item).collect();
                with.push(&item.item);
                if batch.len() >= 128 || byte_length(&navigation_request(self.query, &with, anchor_only)) > 38_000 {
                    batches.push_back(std::mem::take(&mut batch));
                }
            }
            batch.push(item);
        }
        if !batch.is_empty() {
            batches.push_back(batch);
        }
        let queue = Mutex::new((batches, 0usize));
        let changed = Condvar::new();
        let results: Mutex<Vec<(Item, f64)>> = Mutex::new(Vec::new());
        let score_group = |group: Vec<Item>, ticket: u64| -> Option<(Vec<Item>, Vec<Item>)> {
            let mut sources: Vec<Candidate> = group.iter().flat_map(|item| item.donors.clone()).collect();
            if let Some((_, donor)) = anchor {
                sources.push(donor.clone());
            }
            let items: Vec<&NavigationItem> = group.iter().map(|i| &i.item).collect();
            match self.fresh_evaluation_in_turn(&navigation_request(self.query, &items, anchor_only), &sources, true, ticket) {
                Ok(scores) => {
                    let mut results = results.lock().unwrap();
                    for (index, item) in group.into_iter().enumerate() {
                        let score = scores.get(&format!("q{index}")).copied().unwrap_or(f64::NAN);
                        results.push((item, score));
                    }
                    None
                }
                Err(failure) => {
                    if (failure.kind == Kind::SourceInvalid || (failure.kind == Kind::Provider && failure.split_eligible)) && group.len() > 1 {
                        let middle = group.len().div_ceil(2);
                        let mut first = group;
                        let second = first.split_off(middle);
                        Some((first, second))
                    } else {
                        if failure.kind != Kind::SourceInvalid {
                            self.issue(failure.kind.name(), 1, Some(failure.message));
                        }
                        None
                    }
                }
            }
        };
        std::thread::scope(|scope| {
            for _ in 0..STAGE_WORKERS {
                scope.spawn(|| loop {
                    let group = {
                        let mut guard = queue.lock().unwrap();
                        loop {
                            if self.stopped() {
                                changed.notify_all();
                                return;
                            }
                            if let Some(group) = guard.0.pop_front() {
                                guard.1 += 1;
                                // The order in which groups leave the queue is the order they are asked.
                                break (group, self.evaluator.ticket());
                            }
                            if guard.1 == 0 {
                                changed.notify_all();
                                return;
                            }
                            guard = changed.wait(guard).unwrap();
                        }
                    };
                    // The group stops counting as in flight even if scoring it panics.
                    struct InFlight<'q>(&'q Mutex<(VecDeque<Vec<Item>>, usize)>, &'q Condvar);
                    impl Drop for InFlight<'_> {
                        fn drop(&mut self) {
                            if let Ok(mut guard) = self.0.lock() {
                                guard.1 -= 1;
                            }
                            self.1.notify_all();
                        }
                    }
                    let in_flight = InFlight(&queue, &changed);
                    let halves = score_group(group.0, group.1);
                    if let Some((first, second)) = halves {
                        let mut guard = queue.lock().unwrap();
                        guard.0.push_back(first);
                        guard.0.push_back(second);
                    }
                    drop(in_flight);
                });
            }
        });
        results.into_inner().unwrap()
    }

    fn preview_directory(&self, path: &str) -> Option<Value> {
        let mut entries: Vec<(String, &'static str)> = Vec::new();
        let (mut truncated, mut files, mut directories, mut bytes) = (false, 0, 0, 0);
        let mut extensions: indexmap::IndexMap<String, usize> = indexmap::IndexMap::new();
        let mut cursor: Option<Cursor> = None;
        let mut first = true;
        while first || (cursor.is_some() && !self.stopped()) {
            first = false;
            let page = self.reader.list_page(path, cursor.take());
            cursor = page.next;
            for kind in &page.issues {
                self.issue(kind.name(), 1, None);
            }
            if !page.issues.is_empty() {
                return None;
            }
            for entry in page.entries {
                let kind = if entry.directory { "directory" } else { "file" };
                let name = basename(&entry.path).to_string();
                let size = byte_length(&json!({"name": name, "kind": kind}));
                if entries.len() >= 64 || bytes + size > 4096 {
                    truncated = true;
                    break;
                }
                bytes += size;
                if entry.directory {
                    directories += 1;
                } else {
                    files += 1;
                    let extension = extname(&entry.path);
                    let extension = if extension.is_empty() { "[no extension]".to_string() } else { extension };
                    *extensions.entry(extension).or_default() += 1;
                }
                entries.push((name, kind));
            }
            if truncated {
                break;
            }
        }
        if cursor.is_some() {
            truncated = true;
        }
        entries.sort_by(|a, b| js::locale_compare(&a.0, &b.0));
        Some(json!({
            "entries": entries.iter().map(|(name, kind)| json!({"name": name, "kind": kind})).collect::<Vec<_>>(),
            "truncated": truncated, "sampledFiles": files, "sampledDirectories": directories, "sampledExtensions": extensions.into_iter().map(|(k, v)| (k, Value::from(v))).collect::<Map<String, Value>>(),
        }))
    }

    /// Content samples of a directory's files, for the relationship question.
    fn with_directory_content(&self, item: &Item) -> Item {
        let mut preview = item.item.preview.as_object().cloned().unwrap_or_default();
        let children: Vec<String> = preview.get("entries").and_then(Value::as_array).into_iter().flatten()
            .filter(|child| js::str_of(child, "kind") == "file").map(|child| js::str_of(child, "name").to_string()).collect();
        let per_file = 80.max(16000 / children.len().max(1));
        let part = per_file / 3;
        let mut samples: Vec<(String, bool, String)> = Vec::new();
        let mut donors = Vec::new();
        for child in children {
            if self.stopped() {
                break;
            }
            let Some(snapshot) = self.snapshot(&format!("{}/{child}", item.item.path)) else { continue };
            if snapshot.source.len() > 1_000_000 {
                continue;
            }
            donors.push(Candidate { path: snapshot.path.clone(), content_hash: snapshot.content_hash.clone(), score: 0.0 });
            let source = snapshot.source.as_str();
            let length = js::len(source);
            let text = if length <= per_file {
                source.to_string()
            } else {
                let offsets = [0, (length / 2).saturating_sub(part / 2), length.saturating_sub(part)];
                offsets.iter().map(|&start| format!("[character offset {start}]\n{}", js::slice(source, start, start + part))).collect::<Vec<_>>().join("\n...\n")
            };
            samples.push((child, length > per_file, text));
        }
        let build = |samples: &[(String, bool, String)]| {
            let mut preview = preview.clone();
            preview.insert("contentSamples".into(), samples.iter().map(|(name, truncated, source)| json!({"name": name, "truncated": truncated, "source": source})).collect());
            Value::Object(preview)
        };
        while byte_length(&build(&samples)) > 28000 && samples.iter().any(|sample| js::len(&sample.2) > 80) {
            for sample in &mut samples {
                let keep = 80.max((js::len(&sample.2) as f64 * 0.8) as usize);
                sample.2 = js::head(&sample.2, keep).to_string();
                sample.1 = true;
            }
        }
        preview = build(&samples).as_object().cloned().unwrap_or_default();
        Item { item: NavigationItem { preview: Value::Object(preview), ..item.item.clone() }, donors }
    }

    fn preview_file(&self, snapshot: &Snapshot) -> Value {
        let bytes = snapshot.source.as_bytes();
        let opening = &bytes[..bytes.len().min(16384)];
        // A streaming decoder drops a trailing partial sequence.
        let valid = match std::str::from_utf8(opening) {
            Ok(text) => text,
            Err(error) => std::str::from_utf8(&opening[..error.valid_up_to()]).unwrap_or(""),
        };
        let mut text = valid.to_string();
        let mut truncated = bytes.len() > 16384;
        while js::quote(&text).len() > 24000 {
            let end = (js::len(&text) as f64 * 0.75) as usize;
            text = js::head(&text, end).to_string();
            truncated = true;
        }
        let path = &snapshot.path;
        let (mut text, mut preview_bytes, mut range) = (text.clone(), text.len(), "opening bytes");
        if truncated && source::is_python(path) && bytes.len() <= 1_000_000 {
            if let Some(sampled) = python::checked_preview(self.query, &snapshot.source, 16384) {
                if sampled.truncated && !sampled.text.is_empty() && sampled.text.len() <= 16384 && js::quote(&sampled.text).len() <= 24000 {
                    text = sampled.text;
                    preview_bytes = sampled.preview_bytes;
                    range = "sampled source ranges";
                }
            }
        }
        let build = |declarations: &[Value], index_truncated: bool| json!({
            "sizeBytes": bytes.len(), "extension": extname(path), "text": text, "previewBytes": preview_bytes, "truncated": truncated,
            "range": range, "declarations": declarations, "declarationIndexTruncated": index_truncated,
        });
        let mut declarations: Vec<Value> = Vec::new();
        let mut index_truncated = false;
        if truncated && (source::is_python(path) || source::is_script(path) || source::is_php(path)) && bytes.len() <= 1_000_000 {
            let syntax = source::inspect(path, &snapshot.source, Bounds { max_unit_bytes: 4.max(bytes.len()), ..Bounds::default() });
            declarations = syntax.units.iter().filter(|unit| !unit.partial)
                .map(|unit| json!({"name": unit.name, "startLine": unit.range.start, "endLine": unit.range.end})).collect();
            while !declarations.is_empty() && byte_length(&build(&declarations, index_truncated)) > 32000 {
                declarations.pop();
                index_truncated = true;
            }
        }
        build(&declarations, index_truncated)
    }

    fn discover(&self, seeds: Vec<String>, anchor: Option<(&Anchor, &Candidate)>) {
        let mut directories = seeds;
        while !directories.is_empty() && !self.stopped() && self.state.lock().unwrap().entries_seen < 100_000 {
            let mut level: Vec<(String, usize)> = directories.drain(..).map(|path| (path, 0)).collect();
            let mut items: Vec<Item> = Vec::new();
            let mut hashes: HashMap<String, String> = HashMap::new();
            let mut index = 0;
            while index < level.len() && !self.stopped() {
                let (current, depth) = level[index].clone();
                index += 1;
                if self.state.lock().unwrap().entries_seen >= 100_000 {
                    self.issue("resource_limit", 1, None);
                    break;
                }
                if !self.state.lock().unwrap().visited.insert(current.clone()) {
                    continue;
                }
                let mut entries = Vec::new();
                let mut cursor: Option<Cursor> = None;
                let mut first = true;
                while first || (cursor.is_some() && !self.stopped()) {
                    first = false;
                    let page = self.reader.list_page(&current, cursor.take());
                    cursor = page.next;
                    for kind in &page.issues {
                        self.issue(kind.name(), 1, None);
                    }
                    entries.extend(page.entries);
                    let seen = self.state.lock().unwrap().entries_seen;
                    if seen + entries.len() > 100_000 || (cursor.is_some() && seen + entries.len() == 100_000) {
                        self.issue("resource_limit", 1, None);
                        break;
                    }
                }
                entries.sort_by(|a, b| js::locale_compare(&a.path, &b.path));
                for entry in entries {
                    if self.stopped() {
                        break;
                    }
                    let over = {
                        let mut state = self.state.lock().unwrap();
                        state.entries_seen += 1;
                        state.entries_seen > 100_000
                    };
                    if over {
                        self.issue("resource_limit", 1, None);
                        break;
                    }
                    if entry.directory {
                        if depth == 0 {
                            level.push((entry.path, 1));
                        } else if let Some(child_preview) = self.preview_directory(&entry.path) {
                            let item = Item { item: NavigationItem { path: entry.path, directory: true, preview: child_preview }, donors: Vec::new() };
                            items.push(if anchor.is_some() { self.with_directory_content(&item) } else { item });
                        }
                        continue;
                    }
                    let Some(snapshot) = self.snapshot(&entry.path) else { continue };
                    hashes.insert(entry.path.clone(), snapshot.content_hash.clone());
                    let file_preview = self.preview_file(&snapshot);
                    self.state.lock().unwrap().previews.insert(entry.path.clone(), file_preview.clone());
                    let donor = Candidate { path: snapshot.path.clone(), content_hash: snapshot.content_hash.clone(), score: 0.0 };
                    if snapshot.source.len() > 1_000_000 {
                        self.issue("resource_limit", 1, None);
                        items.push(Item { item: NavigationItem { path: entry.path, directory: false, preview: file_preview }, donors: vec![donor] });
                        continue;
                    }
                    let chunks = source::split_source(&snapshot.source, 12_000);
                    let multiple = chunks.len() > 1;
                    for chunk in &chunks {
                        let text = source::source_for_unit(&snapshot.source, chunk);
                        let preview = json!({
                            "sizeBytes": snapshot.source.len(), "extension": extname(&entry.path), "text": text, "previewBytes": text.len(),
                            "truncated": multiple, "range": "sampled source ranges",
                        });
                        items.push(Item { item: NavigationItem { path: entry.path.clone(), directory: false, preview }, donors: vec![donor.clone()] });
                    }
                }
            }
            for (item, probability) in self.score(items, anchor) {
                if item.item.directory {
                    if probability > 0.5 {
                        directories.push(item.item.path.clone());
                    } else if anchor.is_none() {
                        self.state.lock().unwrap().pruned.insert(item.item.path.clone(), item);
                    }
                } else if probability > 0.5 {
                    let mut state = self.state.lock().unwrap();
                    let better = state.candidates.get(&item.item.path).is_none_or(|prior| probability > prior.score);
                    if better {
                        let content_hash = hashes.get(&item.item.path).cloned().unwrap_or_default();
                        state.candidates.insert(item.item.path.clone(), Candidate { path: item.item.path.clone(), content_hash, score: probability });
                    }
                }
            }
        }
        if !directories.is_empty() {
            self.issue("resource_limit", 1, None);
        }
    }

    fn sorted_candidates(&self) -> Vec<Candidate> {
        let mut sorted: Vec<Candidate> = self.state.lock().unwrap().candidates.values().cloned().collect();
        sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| js::locale_compare(&a.path, &b.path)));
        sorted
    }

    /// Run work over the candidates on up to 32 threads, until the search stops.
    fn parallel(&self, items: &[Candidate], work: &(dyn Fn(&Candidate) + Sync)) {
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..STAGE_WORKERS.min(items.len()) {
                scope.spawn(|| {
                    while !self.stopped() {
                        let index = next.fetch_add(1, Ordering::SeqCst);
                        let Some(item) = items.get(index) else { break };
                        work(item);
                    }
                });
            }
        });
    }

    fn select(&self, ordered: &[Candidate], evidence: Option<&(dyn Fn() -> Option<Value> + Sync)>) {
        self.parallel(ordered, &|candidate| {
            let Some(snapshot) = self.unchanged(candidate) else { return };
            if snapshot.source.len() > 1_000_000 {
                self.issue("source_inspection_limit", 1, None);
                return;
            }
            let evaluate = |request: &Value| -> Result<Answers, Failure> {
                let mut sources = vec![candidate.clone()];
                for entry in request["state"].get("selectedEvidence").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(donor) = self.candidate(js::str_of(entry, "path")) {
                        sources.push(donor);
                    }
                }
                self.fresh_evaluation(request, &sources, false)
            };
            let prepare = || -> Result<Option<Option<Value>>, Failure> {
                if self.cancelled.load(Ordering::SeqCst) {
                    return Err(Failure::of(Kind::Cancelled));
                }
                let current = self.unchanged(candidate);
                if self.cancelled.load(Ordering::SeqCst) {
                    return Err(Failure::of(Kind::Cancelled));
                }
                if current.is_none() {
                    return Ok(None);
                }
                Ok(Some(evidence.and_then(|evidence| evidence())))
            };
            let previous = self.state.lock().unwrap().files.get(&candidate.path).cloned();
            let Selection { file, declarations, issues, provider_failure } =
                select_file(&snapshot, self.query, candidate.score, &evaluate, &prepare, previous.as_ref());
            {
                let mut state = self.state.lock().unwrap();
                state.files.insert(candidate.path.clone(), file);
                state.declarations.insert(candidate.path.clone(), declarations);
            }
            for (kind, count) in issues {
                if kind != "source-invalid" {
                    self.issue(&kind, count, provider_failure.clone());
                }
            }
        });
    }

    fn select_evidence(&self, ordered: &[Candidate]) {
        self.select(ordered, None);
        let mut evidence: Vec<Value> = Vec::new();
        // Donors follow selection completion order; concurrent completion can affect the context.
        let paths: Vec<String> = self.state.lock().unwrap().declarations.keys().cloned().collect();
        for path in paths {
            if self.stopped() {
                break;
            }
            let Some(candidate) = self.candidate(&path) else { continue };
            let excerpts = self.state.lock().unwrap().files.get(&path).map(|f| f.excerpts.clone()).unwrap_or_default();
            if excerpts.is_empty() || self.unchanged(&candidate).is_none() {
                continue;
            }
            for excerpt in excerpts {
                let mut entry = Map::new();
                entry.insert("path".into(), path.clone().into());
                entry.extend(excerpt.range.fields());
                entry.insert("source".into(), excerpt.source.into());
                evidence.push(Value::Object(entry));
            }
        }
        let all = Value::Array(evidence.clone());
        if !evidence.is_empty() && byte_length(&all) <= 64_000 && !self.stopped() {
            let fresh = || -> Option<Value> {
                let mut current: HashSet<String> = HashSet::new();
                let mut seen: HashSet<String> = HashSet::new();
                for entry in &evidence {
                    let path = js::str_of(entry, "path").to_string();
                    if seen.insert(path.clone()) {
                        if let Some(candidate) = self.candidate(&path) {
                            if self.unchanged(&candidate).is_some() {
                                current.insert(path);
                            }
                        }
                    }
                }
                let kept: Vec<Value> = evidence.iter().filter(|entry| current.contains(js::str_of(entry, "path"))).cloned().collect();
                (!kept.is_empty()).then_some(Value::Array(kept))
            };
            self.select(ordered, Some(&fresh));
        }
    }

    fn assess(&self, ordered: &[Candidate], assessments: &Mutex<HashMap<String, (Vec<String>, f64)>>) {
        self.parallel(ordered, &|candidate| {
            if self.unchanged(candidate).is_none() {
                return;
            }
            let Some(preview) = self.state.lock().unwrap().previews.get(&candidate.path).cloned() else { return };
            match self.fresh_evaluation(&file_assessment_request(self.query, &candidate.path, &preview), std::slice::from_ref(candidate), false) {
                Ok(scores) => {
                    let labels = scores.iter().filter(|(role, p)| role.as_str() != "priority" && **p > 0.5).map(|(role, _)| role.clone()).collect();
                    let priority = scores.get("priority").copied().unwrap_or(f64::NAN);
                    assessments.lock().unwrap().insert(candidate.path.clone(), (labels, priority));
                }
                Err(failure) if failure.kind != Kind::SourceInvalid => self.issue(failure.kind.name(), 1, Some(failure.message)),
                Err(_) => {}
            }
        });
    }
}

/// Inherited same-file calls in the selected Python lines, with their ranges and class headers
/// added to what is shown.
fn local_call_context(snapshot: &Snapshot, file: &FileEvidence) -> Option<(Vec<Excerpt>, Vec<CallLead>)> {
    let original = file.shown();
    if !source::is_python(&snapshot.path) || snapshot.source.len() > 1_000_000 || file.source_omitted || original.iter().any(|e| e.partial || e.range.bytes.is_some()) {
        return None;
    }
    let selected: Vec<Range> = file.selected.iter().filter(|r| r.bytes.is_none()).map(EvidenceRange::range).collect();
    if selected.is_empty() {
        return None;
    }
    let calls = python::calls(&snapshot.source, &selected)?;
    if calls.is_empty() {
        return None;
    }
    let lines: Vec<&str> = snapshot.source.split('\n').collect();
    let valid = |r: &Range| r.start >= 1 && r.end >= r.start && r.end <= lines.len();
    if calls.iter().any(|c| !valid(&c.range) || !valid(&c.owner_header)) {
        return None;
    }
    let text = |r: &Range| lines[r.start - 1..r.end].join("\n");
    let kept: Vec<&python::Call> = calls.iter().filter(|c| text(&c.range).len() <= 24_000).collect();
    if kept.is_empty() {
        return None;
    }
    let mut ranges: Vec<Range> = original.iter().map(|e| e.range.range()).collect();
    for call in &kept {
        ranges.push(call.range);
        ranges.push(call.owner_header);
    }
    ranges.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    let mut merged: Vec<Range> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end + 1 => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    Some((
        merged.iter().map(|range| Excerpt { range: EvidenceRange::lines(*range), source: text(range), partial: false }).collect(),
        kept.iter().map(|c| CallLead { caller: c.caller.clone(), name: c.name.clone(), range: c.range, unknown_earlier_bases: c.unknown_earlier_bases.clone() }).collect(),
    ))
}

struct TestChange {
    path: String,
    presentation_excerpts: Vec<Excerpt>,
    presentation_selected: Vec<EvidenceRange>,
}

/// Optional: which complete test bodies to show. All-negative answers change nothing.
fn select_test_bodies(query: &str, inputs: &[(Arc<Snapshot>, FileEvidence)], evaluate: &dyn Fn(&Value) -> Result<Answers, Failure>) -> Result<Vec<TestChange>, Failure> {
    let contains = |outer: &Range, inner: &Range| outer.start <= inner.start && outer.end >= inner.end;
    let mut sorted: Vec<&(Arc<Snapshot>, FileEvidence)> = inputs.iter().collect();
    sorted.sort_by(|a, b| js::locale_compare(&a.0.path, &b.0.path));
    let mut candidates: Vec<Value> = Vec::new();
    for (snapshot, file) in sorted {
        let shown = file.shown();
        if !file.roles.iter().any(|r| r == "test") || file.source_omitted || !source::is_python(&snapshot.path) || snapshot.source.len() > 1_000_000
            || shown.iter().any(|e| e.partial || e.range.bytes.is_some()) {
            continue;
        }
        let syntax = source::inspect(&snapshot.path, &snapshot.source, Bounds::default());
        if syntax.mode != Mode::Python || syntax.fallback.is_some() {
            continue;
        }
        let lines: Vec<&str> = snapshot.source.split('\n').collect();
        for unit in &syntax.units {
            if unit.partial || unit.name.ends_with(".context")
                || !file.selected.iter().any(|r| r.bytes.is_none() && contains(&r.range(), &unit.range))
                || !shown.iter().any(|e| contains(&e.range.range(), &unit.range)) {
                continue;
            }
            candidates.push(json!({
                "path": snapshot.path, "name": unit.name, "startLine": unit.range.start, "endLine": unit.range.end,
                "source": lines[unit.range.start - 1..unit.range.end.min(lines.len())].join("\n"),
            }));
        }
    }
    let mut groups: Vec<Vec<Value>> = Vec::new();
    let (mut group, mut bytes) = (Vec::new(), 0);
    for candidate in candidates {
        let size = byte_length(&candidate);
        if !group.is_empty() && (group.len() >= 32 || bytes + size > 64_000) {
            groups.push(std::mem::take(&mut group));
            bytes = 0;
        }
        group.push(candidate);
        bytes += size;
    }
    if !group.is_empty() {
        groups.push(group);
    }
    let mut decisions: Vec<(Value, bool)> = Vec::new();
    for batch in groups {
        let answers = evaluate(&test_body_request(query, &batch))?;
        for (i, candidate) in batch.into_iter().enumerate() {
            let score = answers.get(&format!("q{i}")).copied();
            let Some(score) = score.filter(|s| s.is_finite() && (0.0..=1.0).contains(s)) else { return Err(Failure::of(Kind::Provider)) };
            decisions.push((candidate, score > 0.5));
        }
    }
    if !decisions.iter().any(|(_, keep)| *keep) {
        return Ok(Vec::new());
    }
    let mut changes = Vec::new();
    for (snapshot, file) in inputs {
        let local: Vec<&(Value, bool)> = decisions.iter().filter(|(c, _)| js::str_of(c, "path") == snapshot.path).collect();
        let span = |c: &Value| (c["startLine"].as_u64().unwrap_or(0) as usize, c["endLine"].as_u64().unwrap_or(0) as usize);
        let removed: Vec<(usize, usize)> = local.iter().filter(|(_, keep)| !keep).map(|(c, _)| span(c)).collect();
        let kept: Vec<(usize, usize)> = local.iter().filter(|(_, keep)| *keep).map(|(c, _)| span(c)).collect();
        if removed.is_empty() {
            continue;
        }
        let lines: Vec<&str> = snapshot.source.split('\n').collect();
        let remove = |line: usize| removed.iter().any(|&(s, e)| s <= line && line <= e) && !kept.iter().any(|&(s, e)| s <= line && line <= e);
        let subtract = |range: Range| -> Vec<Range> {
            let mut result = Vec::new();
            let mut start: Option<usize> = None;
            for line in range.start..=range.end {
                if remove(line) {
                    if let Some(s) = start.take() {
                        result.push(Range::new(s, line - 1));
                    }
                } else if start.is_none() {
                    start = Some(line);
                }
            }
            if let Some(s) = start {
                result.push(Range::new(s, range.end));
            }
            result
        };
        let join = |r: &Range| lines[(r.start - 1).min(lines.len())..r.end.min(lines.len())].join("\n");
        let presentation_excerpts = file.shown().iter().flat_map(|e| subtract(e.range.range()))
            .map(|range| Excerpt { range: EvidenceRange::lines(range), source: join(&range), partial: false })
            .filter(|e| !js::trim(&e.source).is_empty()).collect();
        let presentation_selected = file.presentation_selected.as_ref().unwrap_or(&file.selected).iter()
            .flat_map(|r| subtract(r.range())).map(EvidenceRange::lines).collect();
        changes.push(TestChange { path: snapshot.path.clone(), presentation_excerpts, presentation_selected });
    }
    Ok(changes)
}

pub fn retrieve(reader: &Reader, evaluator: &Evaluator, query: &str, cancelled: &'static AtomicBool) -> Result<Outcome, String> {
    let search = Search { reader, evaluator, query, state: Mutex::new(State::default()), stop: AtomicBool::new(false), validation: Mutex::new(()), cancelled };
    search.discover(vec![".".into()], None);
    let mut anchor: Option<(Anchor, Candidate)> = None;
    for candidate in search.sorted_candidates() {
        if candidate.score <= 0.5 || search.stopped() {
            break;
        }
        let Some(snapshot) = search.unchanged(&candidate) else { continue };
        let size = snapshot.source.len();
        let units = source::inspect(&snapshot.path, &snapshot.source, Bounds { max_parse_bytes: 1.max(size), max_unit_bytes: 4.max(size) }).units;
        let mut classes: Vec<String> = Vec::new();
        for unit in units.iter().filter(|unit| unit.name.ends_with(".context")) {
            let class = unit.name.split('.').next().unwrap_or("").to_string();
            if !classes.contains(&class) {
                classes.push(class);
            }
        }
        if !classes.is_empty() && byte_length(&json!(classes)) < 4000 {
            anchor = Some((Anchor { path: candidate.path.clone(), classes }, candidate));
            break;
        }
    }
    if let Some((anchor, donor)) = anchor.as_ref().filter(|_| !search.stopped()) {
        // Only one relationship reconsideration, anchored before new candidates are admitted.
        let pruned: Vec<Item> = search.state.lock().unwrap().pruned.values().cloned().collect();
        let mut items = Vec::new();
        for item in pruned {
            if search.stopped() {
                break;
            }
            items.push(search.with_directory_content(&item));
        }
        let seeds = search.score(items, Some((anchor, donor))).into_iter().filter(|(_, score)| *score > 0.5).map(|(item, _)| item.item.path).collect();
        search.discover(seeds, Some((anchor, donor)));
    }
    let ordered: Vec<Candidate> = search.state.lock().unwrap().candidates.values().cloned().collect();
    {
        let mut state = search.state.lock().unwrap();
        for candidate in &ordered {
            state.files.insert(candidate.path.clone(), FileEvidence::admitted(&candidate.path, &candidate.content_hash, candidate.score));
        }
    }
    // File assessment reads only the discovery preview, so it runs alongside evidence selection.
    let assessments: Mutex<HashMap<String, (Vec<String>, f64)>> = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        scope.spawn(|| search.select_evidence(&ordered));
        scope.spawn(|| search.assess(&ordered, &assessments));
    });
    let assessments = assessments.into_inner().unwrap();
    search.parallel(&ordered, &|candidate| {
        let Some(file) = search.state.lock().unwrap().files.get(&candidate.path).cloned() else { return };
        if file.source_omitted {
            return;
        }
        let Some(snapshot) = search.unchanged(candidate) else { return };
        let mut file = file;
        if assessments.get(&candidate.path).is_some_and(|(labels, _)| labels.iter().any(|l| l == "test")) {
            file.presentation_excerpts = file.selected_presentation_excerpts.clone().or(file.presentation_excerpts.take());
            file.presentation_selected = Some(file.selected.clone());
            if let Some(stored) = search.state.lock().unwrap().files.get_mut(&candidate.path) {
                stored.presentation_excerpts = file.presentation_excerpts.clone();
                stored.presentation_selected = file.presentation_selected.clone();
            }
        }
        if let Some((excerpts, leads)) = local_call_context(&snapshot, &file) {
            if search.unchanged(candidate).is_some() {
                if let Some(stored) = search.state.lock().unwrap().files.get_mut(&candidate.path).filter(|f| !f.source_omitted) {
                    stored.presentation_excerpts = Some(excerpts);
                    stored.call_leads = leads;
                }
            }
        }
    });
    // Selection replaces file records, so assessments attach afterwards; invalidated files keep none.
    {
        let mut state = search.state.lock().unwrap();
        for (path, (labels, priority)) in &assessments {
            if let Some(file) = state.files.get_mut(path).filter(|f| !f.source_omitted) {
                file.roles = labels.clone();
                file.priority = Some(*priority);
            }
        }
    }
    let mut test_inputs = Vec::new();
    for candidate in &ordered {
        let file = search.state.lock().unwrap().files.get(&candidate.path).cloned();
        let Some(file) = file.filter(|f| !f.source_omitted && f.roles.iter().any(|r| r == "test")) else { continue };
        if let Some(snapshot) = search.unchanged(candidate) {
            test_inputs.push((snapshot, file));
        }
    }
    let evaluate = |request: &Value| -> Result<Answers, Failure> {
        let mut paths: Vec<String> = Vec::new();
        for candidate in request["state"]["candidates"].as_object().into_iter().flat_map(|c| c.values()) {
            let path = js::str_of(candidate, "path").to_string();
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        let donors: Vec<Candidate> = paths.iter().filter_map(|path| search.candidate(path)).collect();
        search.fresh_evaluation(request, &donors, false)
    };
    match select_test_bodies(query, &test_inputs, &evaluate) {
        Ok(changes) => {
            let current = test_inputs.iter().all(|(snapshot, file)| {
                search.candidate(&snapshot.path).and_then(|c| search.unchanged(&c)).is_some()
                    && search.state.lock().unwrap().files.get(&snapshot.path).is_some_and(|f| !f.source_omitted && f.content_hash == file.content_hash)
            });
            if current {
                let mut state = search.state.lock().unwrap();
                for change in changes {
                    if let Some(file) = state.files.get_mut(&change.path) {
                        file.presentation_excerpts = Some(change.presentation_excerpts);
                        file.presentation_selected = Some(change.presentation_selected);
                    }
                }
            }
        }
        Err(failure) => search.issue(failure.kind.name(), 1, Some(failure.message)),
    }
    {
        let state = search.state.lock().unwrap();
        if state.issues.contains_key("authentication") && state.files.is_empty() {
            return Err("Jev refused the TypeSafe key (authentication failed).".into());
        }
    }
    if cancelled.load(Ordering::SeqCst) {
        search.issue("cancelled", 1, None);
    }
    {
        let mut state = search.state.lock().unwrap();
        let missing: Vec<Candidate> = state.candidates.values().filter(|c| !state.files.contains_key(&c.path)).cloned().collect();
        for candidate in missing {
            state.files.insert(candidate.path.clone(), FileEvidence::admitted(&candidate.path, &candidate.content_hash, candidate.score));
        }
    }
    let sorted = search.sorted_candidates();
    let files: Vec<FileEvidence> = {
        let state = search.state.lock().unwrap();
        sorted.iter().filter_map(|c| state.files.get(&c.path).cloned()).collect()
    };
    // Repository context: scoped guidance, and pytest entry points that were not run.
    let mut directories: indexmap::IndexSet<String> = indexmap::IndexSet::from([".".to_string()]);
    for file in &files {
        let mut directory = dirname(&file.path);
        while directory != "." && directory != "/" {
            directories.insert(directory.to_string());
            directory = dirname(directory);
        }
    }
    let mut instruction_files = Vec::new();
    let mut instruction_lookup_incomplete = false;
    for directory in &directories {
        let path = if directory == "." { "AGENTS.md".to_string() } else { format!("{directory}/AGENTS.md") };
        match reader.lookup_file(&path) {
            Lookup::File => instruction_files.push(path),
            Lookup::Missing => {}
            Lookup::Other => instruction_lookup_incomplete = true,
        }
    }
    let pytest = regex::Regex::new(r"(^|\n)\s*(import pytest\b|from pytest\b)").unwrap();
    let mut pytest_files = Vec::new();
    for file in &files {
        if !file.path.ends_with(".py") || file.excerpts.is_empty() {
            continue;
        }
        let Some(candidate) = search.candidate(&file.path) else { continue };
        let Some(snapshot) = search.unchanged(&candidate) else { continue };
        if !pytest.is_match(&snapshot.source) {
            continue;
        }
        let declarations = search.state.lock().unwrap().declarations.get(&file.path).cloned().unwrap_or_default();
        let has_test = declarations.iter().any(|(name, range)| {
            name.rsplit('.').next().unwrap_or("").starts_with("test_") && file.rendered.iter().any(|r| range.start >= r.start && range.end <= r.end)
        });
        if has_test {
            pytest_files.push(file.path.clone());
        }
    }
    // File assessment may outlive the bytes it classified.
    for candidate in &sorted {
        search.unchanged(candidate);
    }
    let files: Vec<FileEvidence> = {
        let state = search.state.lock().unwrap();
        sorted.iter().filter_map(|c| state.files.get(&c.path).cloned()).collect()
    };
    let state = search.state.into_inner().unwrap();
    let status = if cancelled.load(Ordering::SeqCst) { "interrupted" } else if state.issues.is_empty() { "complete" } else { "incomplete" };
    Ok(Outcome {
        status,
        files,
        issues: state.issues.into_iter().collect(),
        warnings: evaluator.cache_issues(),
        provider_failure: state.provider_failure,
        instruction_files,
        instruction_lookup_incomplete,
        pytest_files,
    })
}
