class Repository:
    def __init__(self):
        self.records = {}

    def submit(self, key, title):
        self.records[key] = {"key": key, "title": title, "revision": 1}

    def retitle(self, key, title):
        record = self.records[key]
        record["title"] = title
        record["revision"] += 1

    def read(self, key):
        return dict(self.records[key])
