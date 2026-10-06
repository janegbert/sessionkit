import json
import os
from pathlib import Path
import sys
import tomllib
from .factory import create_store


def build():
    config = tomllib.loads((Path(__file__).parents[1] / "launch.toml").read_text())
    kind = os.environ.get("STORE_KIND", config["store"])
    return create_store(kind)


def main():
    store = build()
    def send(payload):
        print(json.dumps(payload))
        return True
    for line in sys.stdin:
        event = json.loads(line)
        if event["op"] == "submit":
            store.submit(event["key"], event["title"])
        elif event["op"] == "retitle":
            store.retitle(event["key"], event["title"])
        elif event["op"] == "tick":
            store.tick(send)


if __name__ == "__main__":
    main()
