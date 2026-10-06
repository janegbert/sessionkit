import os


def delivery_spec():
    return os.environ.get("DELIVERY_IMPL", "app.transport:send")
