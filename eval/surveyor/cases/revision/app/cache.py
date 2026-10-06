class Cache:
    def __init__(self):
        self.values = {}

    def get(self, key, load):
        if key not in self.values:
            self.values[key] = load()
        return dict(self.values[key])
