class RemoteStore:
    def submit(self, key, title):
        raise NotImplementedError("transport is supplied by an embedding application")
