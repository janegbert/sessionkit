# ADR-0001 One crate, modules as domains, no microservices

The shop is one Rust crate and one process. Each domain is a module. Modules call each other
in-process, synchronously; there are no separate services, no network calls between domains and
no message broker.

Considered: a billing service of its own. Rejected: one team, one database, and the cost of
distributed transactions for order completion outweighs independent deployment.
