import urllib.request
from .storage import insert_item


def create_item(text):
    return insert_item(text)


def publish_index(item_id, text):
    request = urllib.request.Request("https://index.example.invalid/items", data=text.encode(), method="POST")
    with urllib.request.urlopen(request) as response:
        return response.status
