from dotenv import load_dotenv

load_dotenv()


def main() -> None:
    from src.bot import run
    run()


if __name__ == "__main__":
    main()
