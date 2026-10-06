# Prices are a built-in table

Token prices live in a table in `src/pricing.rs`, and a new model needs a release. Checked on 2026-10-02: the Models API (`/v1/models`) returns no prices, and Claude Code exposes only computed totals (`cost.total_cost_usd`, `estimated_cache_write_usd`), not per-token rates. The only source a program can read is the pricing page of the documentation, which is written for people; a parser on it would break without a message when the page changes. When a model has no price, `usage` says so and leaves its tokens out of the costs.
