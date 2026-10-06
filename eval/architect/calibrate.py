"""Measure the citation check that orders onboarding's calibration questions, against real Jev.

Every case is a calibration question as the architect writes it, against the fixture code in
shop/ (or shop-repaired/, the same code with the intent of the two Customer types in a comment).
`expect` is what the cited code shows about the recommended answer: supports, contradicts or
says_nothing; null where the label is not clear-cut. The six real questions come from a live
onboarding of this fixture; the planted ones state what the code shows, or its opposite; the
repaired case is real question 1 after the intent was written down.

    cargo build && python3 eval/architect/calibrate.py [path/to/sessionkit]

One Jev question per case, a fraction of a cent.
"""
import json, os, re, subprocess, sys

here = os.path.dirname(os.path.abspath(__file__))
binary = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "..", "..", "target", "debug", "sessionkit")
cases = json.load(open(os.path.join(here, "calibration.json")))

right = claimed = 0
for case in cases:
    text = (f"1. {case['question']}\n   Recommended: {case['recommended']}  (confidence 0.6)\n"
            f"   Evidence: {', '.join('`' + e + '`' for e in case['evidence'])}\n   Options: a; b\n")
    run = subprocess.run([binary, "architect", "rank", os.path.join(here, case["root"])], input=text, capture_output=True, text=True)
    if run.returncode != 0:
        sys.exit(f"{case['name']}: {run.stderr.strip()}")
    found = re.search(r"Code: (supports|contradicts|says nothing) \(([\d.]+)\)", run.stdout)
    if not found:
        sys.exit(f"{case['name']}: {run.stdout.strip().splitlines()[-1]}")
    relation, p = found.group(1).replace(" ", "_"), float(found.group(2))
    expect = case["expect"]
    mark = "   " if expect is None else ("ok " if relation == expect else "XX ")
    if expect is not None:
        claimed += 1
        right += relation == expect
    print(f"{mark}{relation:<13}{p:.2f}  expect {expect or '-':<13} {case['name']}")
print(f"\nright: {right}/{claimed} labelled cases")
