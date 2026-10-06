import sqlite3
from .settings import database_path


def connect():
    return sqlite3.connect(database_path())


def initialize():
    with connect() as db:
        db.executescript("CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, text TEXT);"
                         "CREATE TABLE IF NOT EXISTS jobs (item_id INTEGER, done INTEGER DEFAULT 0);")


def insert_item(text):
    with connect() as db:
        item_id = db.execute("INSERT INTO items(text) VALUES (?)", (text,)).lastrowid
        db.execute("INSERT INTO jobs(item_id) VALUES (?)", (item_id,))
        return item_id


def rename_direct(item_id, text):
    with connect() as db:
        db.execute("UPDATE items SET text = ? WHERE id = ?", (text, item_id))


def next_job():
    with connect() as db:
        return db.execute("SELECT items.id, items.text FROM items JOIN jobs ON items.id = jobs.item_id WHERE jobs.done = 0 LIMIT 1").fetchone()


def finish_job(item_id):
    with connect() as db:
        db.execute("UPDATE jobs SET done = 1 WHERE item_id = ?", (item_id,))
