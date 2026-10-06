"""Per sessionkit compaction in real sessions: how much of the conversation kept after it comes
before the first message that changed (thinking blocks included)? That prefix is what a
compaction that rebuilds no assistant message could still read from the cache."""
import glob, json, os

def content(d):
    c = (d.get("message") or {}).get("content")
    return json.dumps(c, sort_keys=True)

def kind(d):
    c = (d.get("message") or {}).get("content")
    if isinstance(c, list) and c and isinstance(c[0], dict): return d.get("type") + ":" + c[0].get("type", "?")
    return d.get("type") + ":text"

for path in glob.glob(os.path.expanduser("~/.claude/projects/*/*.jsonl")):
    if "scratchpad" in path: continue
    entries = []
    for line in open(path, errors="replace"):
        try: entries.append(json.loads(line))
        except ValueError: pass
    bounds = [i for i, d in enumerate(entries) if d.get("subtype") == "compact_boundary"]
    for n, bi in enumerate(bounds):
        if any(d.get("isCompactSummary") for d in entries[bi + 1:bi + 7]): continue
        start = bounds[n - 1] + 1 if n else 0
        chain = lambda ds: [d for d in ds if d.get("type") in ("user", "assistant") and not d.get("isSidechain") and d.get("message")]
        pre = chain(entries[start:bi])
        stamp = entries[bi].get("timestamp") or ""
        post = [d for d in chain(entries[bi + 1:]) if (d.get("timestamp") or "") <= stamp]
        if not post:
            print(stamp[:16], "no copies with an older timestamp; next kinds:", [d.get("type") for d in entries[bi + 1:bi + 6]]); continue
        same = 0
        while same < min(len(pre), len(post)) and content(pre[same]) == content(post[same]): same += 1
        size = lambda ds: sum(len(content(d)) for d in ds)
        total = size(post)
        print(stamp[:16], path.split("/")[-2][-12:], "pre", len(pre), "post", len(post), "| same first", same, "messages = %d of %d chars (%.0f%%)" % (size(post[:same]), total, 100 * size(post[:same]) / total),
              "| first change:", kind(pre[same]) if same < len(pre) else "-", "->", kind(post[same]) if same < len(post) else "-")
