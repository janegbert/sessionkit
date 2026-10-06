# The architect, 2026-10-03

Does Jev's wake question separate architectural changes from ordinary work? Twelve changes
against a test memory (`fixture/ARCHITECTURE.md`, a webshop whose code does not exist): seven
planted architectural ones and five healthy ones, among them a change inside known debt and a
change to a type the memory calls a deliberate twin.

```
cargo build && python3 eval/architect/classify.py
```

About twelve Jev questions, a fraction of a cent; the checks also land in
`~/.cache/sessionkit/architect-checks.jsonl` under session `eval`.

## Result on jev-latest, 2026-10-03

| case | relevance | category |
|---|---|---|
| checkout imports billing's invoice repository | 1.00 | boundary |
| a plan that moves invoice persistence into checkout | 1.00 | boundary |
| a new external dependency in Cargo.toml | 0.99 | structure |
| a background worker with a queue | 0.98 | structure |
| domain code builds its own Postgres pool | 0.96 | boundary |
| a second invoice type in checkout | 0.93 | duplicate |
| pricing adds another import of catalog (widens AD-001) | 0.91 | expands_debt |
| a field on billing::Customer that accounts::Customer also has (deliberate) | 0.53 | structure |
| rename a local variable | 0.00 | local |
| fix an off-by-one in invoice numbering | 0.00 | local |
| change catalog code that already uses pricing (inside AD-001) | 0.00 | local |
| add a unit test | 0.00 | local |

All twelve right at the line of 0.6; margin +0.38 between the lowest planted and the highest
healthy case. The same edit across the catalog–pricing pair scores 0.00 when it stays inside the
known debt and 0.91 when it adds an import: Jev reads the memory, not only the diff. The
deliberate twin is the weak spot: a new public field reads as a contract change. Twelve cases
show the question bites; where the line belongs on real code is for the log to tell.

## Live runs

With `claude -p --plugin-dir` (Claude Code 2.1.288, Sonnet main loop) on a fixture repository:

- An edit that made checkout call billing's repository directly woke the review agent
  (`sessionkit:architect-review`, 7.7 s, two turns). Its `DRIFT` advice named the confirmed rule
  and ADR it broke and arrived in the main loop mid-turn; the coding agent quoted it and put the
  choice to the person. Session cost $0.29.
- Onboarding on a repository without memory ($0.28) wrote a draft with every statement marked
  `[observed]` or `[inferred]`, recorded the catalog–pricing cycle and the checkout shortcut as
  known debt, left the two `Customer` types as an open question with evidence for each side, and
  asked six calibration questions.

Not run live: calibration answers sent back to the architect, a record run, an audit.

## Calibration order, 2026-10-03

Does the citation check that orders onboarding's questions read what the cited code shows?
Twelve calibration questions against the fixture code in `shop/`: the six a live onboarding of
that code asked, five planted ones that state what the code shows or its opposite, and real
question 1 again on `shop-repaired/`, where a comment states the intent.

```
cargo build && python3 eval/architect/calibrate.py
```

| case | expect | criteria as strings | with `what` / `not_for` |
|---|---|---|---|
| real 1: two Customer types separate on purpose | says nothing | says nothing 0.63 | says nothing 0.94 |
| real 2: only through BillingGateway, the direct call is debt | says nothing | contradicts 0.80 | contradicts 0.70 |
| real 3: the catalog-pricing cycle is debt | says nothing | supports 0.64 | says nothing 0.62 |
| real 4: checkout orchestrates | says nothing | supports 0.47 | says nothing 0.77 |
| real 5: a single binary for now | not labelled | supports 0.61 | says nothing 0.86 |
| real 6: pricing owns the price | supports | supports 0.96 | supports 0.77 |
| planted: catalog owns the price | contradicts | contradicts 0.80 | contradicts 0.93 |
| planted: checkout goes only through the gateway | contradicts | contradicts 1.00 | contradicts 1.00 |
| planted: pricing does not depend on catalog | contradicts | contradicts 0.86 | contradicts 0.83 |
| planted: checkout calls the repository directly | supports | supports 0.90 | supports 0.76 |
| planted: the Customer types carry different fields | supports | supports 1.00 | supports 1.00 |
| repaired: real 1 with the intent in a comment | supports | supports 0.62 | supports 0.64 |
| **right** | | **8/11** | **10/11** |

With plain criteria Jev took code that fits an answer about intent for code that shows it. The
structured criteria, the form TypeSafe's Choice page gives for two options that get confused,
fixed two of three; they ship. Real question 2 stays `contradicts`: its answer is a rule the code
breaks today. That puts it at the top of the order, which is defensible (debt or intended is the
person's call), but the label reads as if the draft were wrong. Twelve cases on a toy fixture:
this shows the question is alive and separates here, not that it holds on a real repository.

## Decision records, 2026-10-03

The fixture got three decision records (`fixture/docs/adr/`): one crate and no microservices,
billing as the only writer of invoices, and gapless invoice numbers assigned by billing. Each
case got a label `against`: the records it goes against, or null where that is not clear-cut.
Three cases were added: checkout computing the invoice number itself, a plan to run billing as
an HTTP service, and billing assigning the number at finalization (follows ADR-0003, healthy).
One Choice per record (follows, goes against, unrelated) rides in the same request as the
category question.

```
right at the current line: 15/15 (margin +0.44: planted 0.90 and up, healthy 0.46 and down)
decision records right: 12/13
```

No healthy case was read as going against a record, so the extra wake rule woke nothing
wrongly. Every planted case already crossed 0.6 on relevance; what the records add here is the
exact decision in the architect's brief, not extra wakes. The one miss is the label: "a second
invoice type in checkout" was also read as going against ADR-0003, and its `Invoice` does carry
a `number` of its own, so that reading is defensible. The unlabelled Kafka dependency and
background worker were both read as going against ADR-0001.
