from .storage import initialize, rename_direct


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("id", type=int)
    parser.add_argument("text")
    args = parser.parse_args()
    initialize()
    rename_direct(args.id, args.text)


if __name__ == "__main__":
    main()
