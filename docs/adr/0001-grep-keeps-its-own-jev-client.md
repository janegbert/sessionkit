# grep keeps its own Jev client

`src/grep/evaluator.rs` sends System One requests with its own client instead of `src/jev.rs`. It is a port of jevgrep's evaluator and follows its request policy on purpose: a pinned model, a week-long answer cache, request slots with a shared pause after a 429, and no backoff. Merging it with `jev::post` would change which answers grep gives and when it retries, so the two clients stay separate.
