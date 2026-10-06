import json
import urllib.request


def send(payload):
    request = urllib.request.Request("https://publish.example.invalid/records",
        data=json.dumps(payload).encode(), method="POST")
    with urllib.request.urlopen(request) as response:
        return response.status == 200
