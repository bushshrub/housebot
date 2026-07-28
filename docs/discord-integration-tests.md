# Discord integration tests

The live suite launches the normal Housebot binary against a deterministic local
OpenAI-compatible mock. A second Discord bot sends messages in a private test
channel and verifies the real Gateway, routing, command, agent, and reply paths.
Discord does not permit bot accounts to invoke another application's slash
commands, so slash handler behavior is covered through Housebot's legacy slash
adapter. For example, the driver sends `!/stats @Housebot`; that message and the
real `/stats` interaction use the same command dispatcher and response formatter.
The workflow sets `DISCORD_REGISTER_COMMANDS=false` to avoid mutating the shared
application's command registry during tests; normal deployments default to
registration enabled.

The GitHub `discord-integration` environment must define:

- `INTEGRATION_TEST_DISCORD_TOKEN` for Housebot.
- `INTEGRATION_TEST_DRIVER_DISCORD_TOKEN` for the driver bot.

Both bots must share one private guild. Set the repository variable
`INTEGRATION_TEST_CHANNEL_ID` to its dedicated text channel; the harness derives
the guild ID from that channel and verifies that Housebot is a member.

Housebot needs the Message Content intent and permission to view the channel,
read history, send messages, embed links, and attach files. The driver needs the
Message Content intent and permission to view the channel, read history, and
send messages. The harness enables bot-to-bot mentions and
restricts Housebot to the selected channel in the temporary PostgreSQL database.

To run locally, start PostgreSQL, export the same variables used by the workflow,
then run:

```text
cargo build --package housebot --bin housebot --package discord-integration-tests
bash .github/scripts/run-discord-integration.sh
```

The suite uses unique UUIDs, correlates responses through Discord message
references, and serializes CI runs globally. It preserves the Discord transcript
between `E2E_SESSION_START` and `E2E_SESSION_STOP` markers for later inspection.
Process logs are kept in `integration-logs/` and uploaded on every CI run.
