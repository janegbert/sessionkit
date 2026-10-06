from .store import MemoryStore
from .remote import RemoteStore

BACKENDS = {"memory": MemoryStore}


def create_store(kind):
    return BACKENDS[kind]()
