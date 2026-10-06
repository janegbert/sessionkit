import os


def database_path():
    return os.environ.get("PROJECT_DB", "dev.sqlite")
