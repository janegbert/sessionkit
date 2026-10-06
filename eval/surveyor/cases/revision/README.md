# Revision publisher

`app.bootstrap.build()` wires a repository, cache, pending list and delivery
callback. Applications submit a record, optionally revise it, and step a worker.
Integration defaults are in `app.config`. The transport can be supplied at startup.
