class MemoryStore:
    def __init__(self):
        self.records = {}
        self.pending = []

    def submit(self, key, title):
        self.records[key] = {"key": key, "title": title}
        self.pending.append({"payload": dict(self.records[key])})

    def retitle(self, key, title):
        self.records[key]["title"] = title

    def tick(self, send):
        if not self.pending:
            return False
        job = self.pending[0]
        acknowledged = send(dict(job["payload"]))
        if acknowledged:
            self.pending.pop(0)
        return bool(acknowledged)
