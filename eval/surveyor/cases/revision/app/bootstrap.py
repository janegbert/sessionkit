import importlib
from .repository import Repository
from .cache import Cache
from .worker import Worker
from .config import delivery_spec


def load_sender(spec):
    module, name = spec.split(":", 1)
    return getattr(importlib.import_module(module), name)


class Application:
    def __init__(self, send):
        self.repository = Repository()
        self.cache = Cache()
        self.pending = []
        self.worker = Worker(self.repository, self.cache, self.pending, send)

    def submit(self, key, title):
        self.repository.submit(key, title)
        self.pending.append(key)

    def retitle(self, key, title):
        self.repository.retitle(key, title)


def build(send=None):
    return Application(send if send is not None else load_sender(delivery_spec()))
