package config

import (
	"os"
	"strconv"
)

type Config struct {
	DiscordToken        string
	AgentURL            string
	StorageURL          string
	RemindersURL        string
	ConversationTimeout int // seconds
}

func FromEnv() *Config {
	return &Config{
		DiscordToken:        envOr("DISCORD_BOT_TOKEN", ""),
		AgentURL:            envOr("AGENT_URL", "http://agent:3003"),
		StorageURL:          envOr("STORAGE_URL", "http://storage:3001"),
		RemindersURL:        envOr("REMINDERS_URL", "http://reminders:3004"),
		ConversationTimeout: envParseInt("CONVERSATION_IDLE_TIMEOUT", 300),
	}
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func envParseInt(key string, fallback int) int {
	v := os.Getenv(key)
	if v == "" {
		return fallback
	}
	n, err := strconv.Atoi(v)
	if err != nil {
		return fallback
	}
	return n
}
