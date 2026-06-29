import os

import sentry_sdk
from dotenv import load_dotenv

load_dotenv()

sentry_sdk.init(
    dsn=os.environ["SENTRY_DSN"],
    environment=os.environ.get("SENTRY_ENVIRONMENT", "production"),
    traces_sample_rate=1.0,
)


def main() -> None:
    from src.bot import run
    run()


if __name__ == "__main__":
    main()
