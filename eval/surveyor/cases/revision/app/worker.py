class Worker:
    def __init__(self, repository, cache, pending, send):
        self.repository = repository
        self.cache = cache
        self.pending = pending
        self.send = send

    def step(self):
        if not self.pending:
            return False
        key = self.pending[0]
        record = self.repository.read(key)
        payload = self.cache.get((key, record["revision"]), lambda: record)
        acknowledged = self.send(payload)
        if acknowledged:
            self.pending.pop(0)
        return bool(acknowledged)
