// One owner for eligibility, listing and content snapshots within the search root (jevgrep's
// core/filesystem.ts). A path is eligible when every component from the root passes: not Git
// metadata or tool storage, not hidden, not a sensitive name, not a symlink or special file, not a
// dependency folder, and not ignored by the .gitignore and .ignore files between the root and it.
// Snapshots are read without following links and checked against the metadata before and after.

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use regex::Regex;
use std::collections::HashMap;
use std::fs::{Metadata, ReadDir};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

const DEPENDENCY_DIRECTORIES: [&str; 13] =
    ["node_modules", "vendor", "venv", ".venv", ".tox", "__pycache__", "dist", "build", "coverage", "target", ".next", ".nuxt", ".turbo"];
const SENSITIVE_NAMES: [&str; 12] = [
    "credentials", "credentials.json", "secrets.json", "secrets.yaml", "secrets.yml", "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".netrc",
    ".npmrc", ".pypirc",
];
const SENSITIVE_SUFFIXES: [&str; 4] = [".pem", ".key", ".p12", ".pfx"];
const PAGE_SIZE: usize = 128;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_IGNORE_BYTES: u64 = 1024 * 1024;

static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----").unwrap());

/// Independent switches that each broaden one exclusion category.
#[derive(Clone, Copy, Default)]
pub struct Policy {
    pub hidden: bool,
    pub no_ignore: bool,
    pub include_dependencies: bool,
    pub include_sensitive: bool,
}

impl Policy {
    /// JSON.stringify of jevgrep's policy object: only the switches that are on, in this order.
    pub fn version(&self) -> String {
        let mut parts = Vec::new();
        for (on, name) in [(self.hidden, "hidden"), (self.no_ignore, "noIgnore"), (self.include_dependencies, "includeDependencies"), (self.include_sensitive, "includeSensitive")] {
            if on {
                parts.push(format!("\"{name}\":true"));
            }
        }
        format!("{{{}}}", parts.join(","))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IssueKind {
    Unreadable,
    Changed,
    ResourceLimit,
    Interrupted,
}

impl IssueKind {
    pub fn name(self) -> &'static str {
        match self {
            IssueKind::Unreadable => "unreadable",
            IssueKind::Changed => "changed",
            IssueKind::ResourceLimit => "resource_limit",
            IssueKind::Interrupted => "interrupted",
        }
    }
}

pub struct Snapshot {
    pub path: String,
    pub content_hash: String,
    pub source: String,
}

pub enum SnapshotResult {
    Ok(Arc<Snapshot>),
    Excluded,
    Issue(IssueKind),
}

/// The metadata that must not change while a file is read.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Identity {
    fn of(meta: &Metadata) -> Identity {
        Identity { dev: meta.dev(), ino: meta.ino(), size: meta.size(), mtime: (meta.mtime(), meta.mtime_nsec()), ctime: (meta.ctime(), meta.ctime_nsec()) }
    }
}

struct Eligible {
    path: String,
    absolute: PathBuf,
    meta: Metadata,
    /// Directories from the root down, with their device and inode.
    ancestors: Vec<(PathBuf, u64, u64)>,
}

enum Eligibility {
    Eligible(Eligible),
    Excluded(&'static str),
    Issue(IssueKind),
}

pub struct Entry {
    pub path: String,
    pub directory: bool,
}

pub struct Page {
    pub entries: Vec<Entry>,
    pub issues: Vec<IssueKind>,
    /// A cursor to read the next page, when there is more.
    pub next: Option<Cursor>,
}

pub struct Cursor {
    directory: String,
    handle: ReadDir,
    identity: Eligible,
}

pub enum Lookup {
    File,
    Missing,
    Other,
}

fn error_kind(error: &std::io::Error) -> IssueKind {
    match error.raw_os_error() {
        Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP) => IssueKind::Changed,
        _ if error.kind() == std::io::ErrorKind::NotFound => IssueKind::Changed,
        _ => IssueKind::Unreadable,
    }
}

fn is_sensitive(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == ".env" || lower.starts_with(".env.") || SENSITIVE_NAMES.contains(&lower.as_str()) || SENSITIVE_SUFFIXES.iter().any(|suffix| lower.ends_with(suffix))
}

/// path.resolve for a path relative to a base: the result without `.` and `..` parts.
fn resolve(base: &Path, path: &str) -> PathBuf {
    let mut out = if Path::new(path).is_absolute() { PathBuf::from("/") } else { base.to_path_buf() };
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir => out = PathBuf::from("/"),
            _ => {}
        }
    }
    out
}

fn within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

pub struct Reader {
    pub root: PathBuf,
    root_identity: (u64, u64),
    policy: Policy,
    protected: Vec<PathBuf>,
    rules: Mutex<HashMap<PathBuf, (Identity, Arc<Gitignore>)>>,
    cancelled: &'static AtomicBool,
}

impl Reader {
    pub fn new(root: &str, policy: Policy, protected: &[PathBuf], cancelled: &'static AtomicBool) -> Result<Reader, String> {
        let root = std::fs::canonicalize(resolve(&std::env::current_dir().unwrap_or_default(), root)).map_err(|e| format!("{root}: {e}"))?;
        let meta = std::fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
        if !meta.is_dir() {
            return Err("Search root must be a directory".into());
        }
        let protected = protected.iter().map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())).collect();
        Ok(Reader { root, root_identity: (meta.dev(), meta.ino()), policy, protected, rules: Mutex::new(HashMap::new()), cancelled })
    }

    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn relative(&self, absolute: &Path) -> String {
        absolute.strip_prefix(&self.root).map(|rest| rest.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn hard_excluded(&self, absolute: &Path) -> bool {
        absolute.components().any(|component| component.as_os_str() == ".git") || self.protected.iter().any(|protected| within(protected, absolute))
    }

    /// Ancestors keep their identity, and the file its metadata and real path.
    fn stable(&self, identity: &Eligible) -> bool {
        for (path, dev, ino) in &identity.ancestors {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() && meta.dev() == *dev && meta.ino() == *ino => {}
                _ => return false,
            }
        }
        let same = std::fs::symlink_metadata(&identity.absolute).is_ok_and(|meta| Identity::of(&meta) == Identity::of(&identity.meta));
        same && std::fs::canonicalize(&identity.absolute).is_ok_and(|real| real == identity.absolute)
    }

    fn read_bytes(&self, identity: &Eligible, ceiling: u64) -> Result<Vec<u8>, IssueKind> {
        if self.stopped() {
            return Err(IssueKind::Interrupted);
        }
        if identity.meta.size() > ceiling {
            return Err(IssueKind::ResourceLimit);
        }
        // NONBLOCK keeps a concurrent replacement with a FIFO from hanging the process.
        let mut file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&identity.absolute)
            .map_err(|e| error_kind(&e))?;
        let before = file.metadata().map_err(|e| error_kind(&e))?;
        if !before.is_file() || Identity::of(&before) != Identity::of(&identity.meta) {
            return Err(IssueKind::Changed);
        }
        let mut bytes = Vec::with_capacity(before.size() as usize);
        let mut chunk = vec![0; 64 * 1024];
        loop {
            if self.stopped() {
                return Err(IssueKind::Interrupted);
            }
            let read = file.read(&mut chunk).map_err(|e| error_kind(&e))?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if bytes.len() as u64 > ceiling {
                return Err(IssueKind::ResourceLimit);
            }
        }
        let after = file.metadata().map_err(|e| error_kind(&e))?;
        if Identity::of(&after) != Identity::of(&before) || !self.stable(identity) {
            return Err(IssueKind::Changed);
        }
        Ok(bytes)
    }

    /// The rules of one ignore file, reused while its metadata is unchanged.
    fn rule_file(&self, directory: &Path, name: &str) -> Result<Option<Arc<Gitignore>>, IssueKind> {
        let absolute = directory.join(name);
        let meta = match std::fs::symlink_metadata(&absolute) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.rules.lock().unwrap().remove(&absolute);
                return Ok(None);
            }
            Err(error) => return Err(error_kind(&error)),
        };
        if !meta.is_file() {
            self.rules.lock().unwrap().remove(&absolute);
            return Ok(None);
        }
        let identity = Identity::of(&meta);
        let eligible = Eligible { path: self.relative(&absolute), absolute: absolute.clone(), meta, ancestors: Vec::new() };
        if let Some((known, rules)) = self.rules.lock().unwrap().get(&absolute).cloned() {
            if known == identity {
                return if self.stable(&eligible) { Ok(Some(rules)) } else { Err(IssueKind::Changed) };
            }
        }
        let bytes = self.read_bytes(&eligible, MAX_IGNORE_BYTES)?;
        let text = String::from_utf8(bytes).map_err(|_| IssueKind::Unreadable)?;
        let mut builder = GitignoreBuilder::new(directory);
        for line in text.split('\n') {
            // A pattern that is not a valid glob is left out.
            let _ = builder.add_line(None, line.strip_suffix('\r').unwrap_or(line));
        }
        let rules = Arc::new(builder.build().map_err(|_| IssueKind::Unreadable)?);
        let mut cache = self.rules.lock().unwrap();
        if cache.len() >= 64 {
            if let Some(oldest) = cache.keys().next().cloned() {
                cache.remove(&oldest);
            }
        }
        cache.insert(absolute, (identity, rules.clone()));
        Ok(Some(rules))
    }

    fn eligibility(&self, input: &str, allow_missing: bool) -> Eligibility {
        if Path::new(input).is_absolute() {
            return Eligibility::Excluded("outside_root");
        }
        let absolute = resolve(&self.root, input);
        if !within(&self.root, &absolute) {
            return Eligibility::Excluded("outside_root");
        }
        let path = self.relative(&absolute);
        if self.stopped() {
            return Eligibility::Issue(IssueKind::Interrupted);
        }
        if self.hard_excluded(&absolute) {
            return Eligibility::Excluded("protected");
        }
        let components: Vec<&str> = if path.is_empty() { Vec::new() } else { path.split('/').collect() };
        match self.walk(&path, &absolute, &components) {
            Ok(result) => result,
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => Eligibility::Excluded("missing"),
            Err(error) => Eligibility::Issue(error_kind(&error)),
        }
    }

    fn walk(&self, path: &str, absolute: &Path, components: &[&str]) -> std::io::Result<Eligibility> {
        let root_meta = std::fs::symlink_metadata(&self.root)?;
        if !root_meta.is_dir() || (root_meta.dev(), root_meta.ino()) != self.root_identity {
            return Ok(Eligibility::Issue(IssueKind::Changed));
        }
        let mut ancestors = Vec::new();
        if components.is_empty() {
            return Ok(Eligibility::Eligible(Eligible { path: path.to_string(), absolute: absolute.to_path_buf(), meta: root_meta, ancestors }));
        }
        ancestors.push((self.root.clone(), root_meta.dev(), root_meta.ino()));
        let mut scopes: Vec<(PathBuf, Option<Arc<Gitignore>>, Option<Arc<Gitignore>>)> = Vec::new();
        let mut directory = self.root.clone();
        for (index, name) in components.iter().enumerate() {
            if !self.policy.no_ignore {
                if directory != self.root {
                    match std::fs::symlink_metadata(directory.join(".git")) {
                        Ok(git) if git.is_dir() || git.is_file() => {
                            // A nested repository starts over: outer .gitignore rules stop here.
                            for scope in &mut scopes {
                                scope.1 = None;
                            }
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Ok(Eligibility::Issue(error_kind(&error))),
                    }
                }
                let git = match self.rule_file(&directory, ".gitignore") {
                    Ok(rules) => rules,
                    Err(kind) => return Ok(Eligibility::Issue(kind)),
                };
                let search = match self.rule_file(&directory, ".ignore") {
                    Ok(rules) => rules,
                    Err(kind) => return Ok(Eligibility::Issue(kind)),
                };
                scopes.push((directory.clone(), git, search));
            }
            let child = directory.join(name);
            if self.hard_excluded(&child) {
                return Ok(Eligibility::Excluded("protected"));
            }
            if !self.policy.hidden && name.starts_with('.') {
                return Ok(Eligibility::Excluded("hidden"));
            }
            if !self.policy.include_sensitive && is_sensitive(name) {
                return Ok(Eligibility::Excluded("sensitive_name"));
            }
            let meta = std::fs::symlink_metadata(&child)?;
            if meta.file_type().is_symlink() {
                return Ok(Eligibility::Excluded("symlink"));
            }
            if !meta.is_dir() && !meta.is_file() {
                return Ok(Eligibility::Excluded("special_file"));
            }
            if meta.is_dir() && !self.policy.include_dependencies && DEPENDENCY_DIRECTORIES.contains(name) {
                return Ok(Eligibility::Excluded("dependency"));
            }
            let mut ignored = false;
            for (scope, git, search) in &scopes {
                let candidate = child.strip_prefix(scope).unwrap_or(&child);
                for rules in [git, search].into_iter().flatten() {
                    match rules.matched(candidate, meta.is_dir()) {
                        Match::Ignore(_) => ignored = true,
                        Match::Whitelist(_) => ignored = false,
                        Match::None => {}
                    }
                }
            }
            if ignored {
                return Ok(Eligibility::Excluded("ignored"));
            }
            if index == components.len() - 1 {
                return Ok(Eligibility::Eligible(Eligible { path: path.to_string(), absolute: absolute.to_path_buf(), meta, ancestors }));
            }
            if !meta.is_dir() {
                return Ok(Eligibility::Excluded("not_directory"));
            }
            ancestors.push((child.clone(), meta.dev(), meta.ino()));
            directory = child;
        }
        Ok(Eligibility::Excluded("outside_root"))
    }

    /// Whether a file exists here and is eligible: for AGENTS.md lookups.
    pub fn lookup_file(&self, path: &str) -> Lookup {
        match self.eligibility(path, true) {
            Eligibility::Eligible(eligible) if eligible.meta.is_file() => Lookup::File,
            Eligibility::Excluded("missing") => Lookup::Missing,
            _ => Lookup::Other,
        }
    }

    pub fn read_snapshot(&self, path: &str) -> SnapshotResult {
        let admitted = match self.eligibility(path, false) {
            Eligibility::Eligible(eligible) => eligible,
            Eligibility::Excluded(_) => return SnapshotResult::Excluded,
            Eligibility::Issue(kind) => return SnapshotResult::Issue(kind),
        };
        if !admitted.meta.is_file() {
            return SnapshotResult::Excluded;
        }
        let bytes = match self.read_bytes(&admitted, MAX_FILE_BYTES) {
            Ok(bytes) => bytes,
            Err(kind) => return SnapshotResult::Issue(kind),
        };
        match self.eligibility(path, false) {
            Eligibility::Eligible(current) if Identity::of(&current.meta) == Identity::of(&admitted.meta) => {}
            Eligibility::Eligible(_) => return SnapshotResult::Issue(IssueKind::Changed),
            Eligibility::Excluded(_) => return SnapshotResult::Excluded,
            Eligibility::Issue(kind) => return SnapshotResult::Issue(kind),
        }
        if bytes.iter().any(|&byte| (byte < 32 && ![9, 10, 12, 13].contains(&byte)) || byte == 127) {
            return SnapshotResult::Excluded;
        }
        let Ok(source) = String::from_utf8(bytes.clone()) else { return SnapshotResult::Excluded };
        if !self.policy.include_sensitive && PRIVATE_KEY.is_match(&source) {
            return SnapshotResult::Excluded;
        }
        let content_hash = crate::js::sha256_hex(&bytes);
        SnapshotResult::Ok(Arc::new(Snapshot { path: admitted.path, source, content_hash }))
    }

    /// One page of a directory, in native order: up to 128 scanned names, keeping the eligible
    /// ones. An empty page does not mean the end; the cursor does.
    pub fn list_page(&self, path: &str, cursor: Option<Cursor>) -> Page {
        let mut entries = Vec::new();
        let mut issues = Vec::new();
        let mut cursor = match cursor {
            Some(cursor) => cursor,
            None => {
                let admitted = match self.eligibility(path, false) {
                    Eligibility::Eligible(eligible) if eligible.meta.is_dir() => eligible,
                    Eligibility::Issue(kind) => return Page { entries, issues: vec![kind], next: None },
                    _ => return Page { entries, issues, next: None },
                };
                let handle = match std::fs::read_dir(&admitted.absolute) {
                    Ok(handle) => handle,
                    Err(error) => return Page { entries, issues: vec![error_kind(&error)], next: None },
                };
                if self.stopped() {
                    return Page { entries, issues: vec![IssueKind::Interrupted], next: None };
                }
                Cursor { directory: admitted.path.clone(), handle, identity: admitted }
            }
        };
        if self.stopped() || !self.stable(&cursor.identity) {
            issues.push(if self.stopped() { IssueKind::Interrupted } else { IssueKind::Changed });
            return Page { entries, issues, next: None };
        }
        for _ in 0..PAGE_SIZE {
            let Some(entry) = cursor.handle.next() else {
                if self.stable(&cursor.identity) {
                    return Page { entries, issues, next: None };
                }
                issues.push(IssueKind::Changed);
                return Page { entries: Vec::new(), issues, next: None };
            };
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(error_kind(&error));
                    return Page { entries: Vec::new(), issues, next: None };
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if cursor.directory.is_empty() { name } else { format!("{}/{name}", cursor.directory) };
            match self.eligibility(&relative, false) {
                Eligibility::Issue(kind) => {
                    issues.push(kind);
                    if kind == IssueKind::Interrupted {
                        return Page { entries, issues, next: None };
                    }
                }
                Eligibility::Eligible(eligible) => entries.push(Entry { directory: eligible.meta.is_dir(), path: relative }),
                Eligibility::Excluded(_) => {}
            }
        }
        if !self.stable(&cursor.identity) {
            issues.push(IssueKind::Changed);
            return Page { entries: Vec::new(), issues, next: None };
        }
        Page { entries, issues, next: Some(cursor) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEVER: AtomicBool = AtomicBool::new(false);

    #[test]
    fn eligibility_follows_the_rules() {
        let dir = std::env::temp_dir().join(format!("sessionkit-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for path in ["src/a.py", "src/gen/b.py", "node_modules/x.js", ".hidden/c.py", "keep.log", "drop.log", ".env"] {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, "x = 1\n").unwrap();
        }
        std::fs::write(dir.join(".gitignore"), "*.log\n!keep.log\ngen/\n").unwrap();
        std::fs::write(dir.join("src/bin.dat"), [0u8, 1, 2]).unwrap();
        let reader = Reader::new(dir.to_str().unwrap(), Policy::default(), &[], &NEVER).unwrap();
        let ok = |path: &str| matches!(reader.read_snapshot(path), SnapshotResult::Ok(_));
        assert!(ok("src/a.py"));
        assert!(ok("keep.log"));
        assert!(!ok("drop.log"));
        assert!(!ok("src/gen/b.py"));
        assert!(!ok("node_modules/x.js"));
        assert!(!ok(".hidden/c.py"));
        assert!(!ok(".env"));
        assert!(!ok("src/bin.dat"));
        let page = reader.list_page(".", None);
        let mut names: Vec<String> = page.entries.iter().map(|e| e.path.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["keep.log", "src"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
