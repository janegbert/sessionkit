<!-- sessionkit architect: baseline 0000000 2026-10-03 -->
# Architecture

A webshop in one Rust crate. Fixture for `eval/architect/classify.py`; the code it names does not exist.

## System shape

- **catalog**: products, their descriptions and categories.
- **pricing**: prices, discounts and taxes per product.
- **checkout**: the cart and order completion.
- **billing**: invoices, their numbering, PDFs and persistence; payment status.
- **accounts**: customers, logins and preferences.
- **infra**: the Postgres pool and the HTTP client. Domain modules get them passed in.

## Ownership

- Billing owns invoices and their persistence. Other modules ask through `billing::BillingGateway`. [confirmed]
- Accounts owns customer identity and preferences. [confirmed]
- Checkout owns carts and orders. [confirmed]
- Saved delivery addresses: no owner yet. [open]

## Dependency rules

- Domain modules (catalog, pricing, checkout, billing, accounts) do not import `infra` directly; they take its clients as parameters. [confirmed]
- Checkout depends on billing only through `BillingGateway`. [confirmed]
- Modules talk synchronously, by function calls. No queues, no background workers. [confirmed]

## Decisions

- ADR-0001 One crate, modules as domains, no microservices.
- ADR-0002 Billing is the only module that writes invoices.

## Known debt

- **AD-001 Catalog and pricing import each other.** Pre-existing at onboarding. Policy: no warning on unrelated changes; warn when a change adds another import across this pair.

## Deliberate exceptions

- `accounts::Customer` and `billing::Customer` are different concepts on purpose: the first is a login with preferences, the second a legal entity on an invoice. Do not merge them.

## Open questions

- Who should own saved delivery addresses: accounts or checkout? [inferred accounts 0.55, needs confirmation]
