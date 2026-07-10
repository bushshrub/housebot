package main

import (
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"github.com/bushshrub/housebot/discord-gateway/internal/bot"
	"github.com/bushshrub/housebot/discord-gateway/internal/config"
)

func main() {
	cfg := config.FromEnv()
	if cfg.DiscordToken == "" {
		fmt.Fprintln(os.Stderr, "DISCORD_BOT_TOKEN is required")
		os.Exit(1)
	}

	b := bot.New(cfg)
	if err := b.Start(); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to start bot: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("Discord gateway running. Press Ctrl+C to stop.")

	sc := make(chan os.Signal, 1)
	signal.Notify(sc, syscall.SIGINT, syscall.SIGTERM)
	<-sc
	b.Stop()
}
