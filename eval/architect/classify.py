"""Measure the architect's wake question against real Jev.

Every case is an edit, a new file or a plan against the fixture memory in fixture/ARCHITECTURE.md;
`expect` says whether the architect should wake. Healthy cases include known debt that is not
widened and a deliberate exception; planted ones include debt that is widened.

    cargo build && python3 eval/architect/classify.py [path/to/sessionkit]

About 12 Jev questions, a fraction of a cent. Prints the relevance per case, sorted, and the
margin between the lowest planted case and the highest healthy one.
"""
import json, os, subprocess, sys

here = os.path.dirname(os.path.abspath(__file__))
binary = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "..", "..", "target", "debug", "sessionkit")
root = os.path.join(here, "fixture")
cases = json.load(open(os.path.join(here, "cases.json")))

rows = []
for case in cases:
    payload = {"session_id": "eval", "cwd": root, "kind": case["kind"],
               "file": os.path.join(root, case["file"]) if case["file"] else "", "change": case["change"]}
    run = subprocess.run([binary, "hook", "architect-check"], input=json.dumps(payload), capture_output=True, text=True)
    if run.returncode != 0:
        sys.exit(f"{case['name']}: {run.stderr.strip()}")
    answer = json.loads(run.stdout)
    if "relevance" not in answer:
        sys.exit(f"{case['name']}: not classified ({answer.get('reason')})")
    against = sorted(f.split("/")[-1][:4] for f in answer.get("against", []))
    rows.append((answer["relevance"], answer["category"], case["expect"], answer["wake"], case["name"], against, case.get("against")))

rows.sort(reverse=True)
for relevance, category, expect, wake, name, against, labelled in rows:
    mark = "ok " if expect == wake else "XX "
    adr = "  " if labelled is None else ("ok" if against == sorted(labelled) else "XX")
    print(f"{mark}{relevance:.2f}  {category:<12} {'planted' if expect else 'healthy'}  {adr} against {','.join(against) or '-':<10} {name}")
planted = [r[0] for r in rows if r[2]]  # relevance alone, without the decision records
healthy = [r[0] for r in rows if not r[2]]
print(f"\nlowest planted {min(planted):.2f}, highest healthy {max(healthy):.2f}, margin {min(planted) - max(healthy):+.2f}")
print(f"right at the current line: {sum(1 for r in rows if r[2] == r[3])}/{len(rows)}")
labelled = [r for r in rows if r[6] is not None]
print(f"decision records right: {sum(1 for r in labelled if r[5] == sorted(r[6]))}/{len(labelled)}")
