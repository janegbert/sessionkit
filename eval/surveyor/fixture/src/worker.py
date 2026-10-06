import time
from .service import publish_index
from .storage import initialize, next_job, finish_job


def main():
    initialize()
    while True:
        job = next_job()
        if job:
            publish_index(*job)
            finish_job(job[0])
        time.sleep(1)


if __name__ == "__main__":
    main()
