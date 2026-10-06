# Sample index service

Historical description: the API is the only entrypoint. All item writes go
through the service module. Jobs use Redis. The database is always dev.sqlite.

These notes were written before administrative operations were added. Verify
important claims against the repository; no production runtime evidence is supplied.
