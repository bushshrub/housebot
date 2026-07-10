package bot

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"regexp"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/bwmarrin/discordgo"
	agentclient "github.com/bushshrub/housebot/discord-gateway/internal/agent"
	"github.com/bushshrub/housebot/discord-gateway/internal/config"
	storageclient "github.com/bushshrub/housebot/discord-gateway/internal/storage"
)

const (
	maxMessageLength   = 2000
	codeFileThreshold  = 800
	editIntervalMs     = 1200
)

var codeFenceRe = regexp.MustCompile("(?s)```(\\w*)\\n(.*?)(?:```|$)")

// ── secret redactor ────────────────────────────────────────────────────────────

type SecretRedactor struct {
	secrets []string
}

func NewSecretRedactor() *SecretRedactor {
	var secrets []string
	for _, env := range os.Environ() {
		parts := strings.SplitN(env, "=", 2)
		if len(parts) != 2 { continue }
		key, val := parts[0], parts[1]
		if len(val) < 12 { continue }
		kl := strings.ToUpper(key)
		if strings.Contains(kl, "TOKEN") || strings.Contains(kl, "KEY") ||
			strings.Contains(kl, "SECRET") || strings.Contains(kl, "PASSWORD") {
			secrets = append(secrets, val)
		}
	}
	return &SecretRedactor{secrets: secrets}
}

func (r *SecretRedactor) Redact(s string) string {
	for _, sec := range r.secrets {
		s = strings.ReplaceAll(s, sec, "[REDACTED]")
	}
	return s
}

// ── conversation tracker ────────────────────────────────────────────────────────

type convKey struct{ channelID, userID string }

type ConversationTracker struct {
	mu      sync.Mutex
	active  map[convKey]time.Time
	timeout time.Duration
}

func NewConversationTracker(timeout time.Duration) *ConversationTracker {
	return &ConversationTracker{
		active:  make(map[convKey]time.Time),
		timeout: timeout,
	}
}

func (t *ConversationTracker) IsActive(channelID, userID string) bool {
	t.mu.Lock()
	defer t.mu.Unlock()
	k := convKey{channelID, userID}
	if ts, ok := t.active[k]; ok {
		return time.Since(ts) <= t.timeout
	}
	return false
}

func (t *ConversationTracker) MarkActive(channelID, userID string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.active[convKey{channelID, userID}] = time.Now()
}

func (t *ConversationTracker) Remove(channelID, userID string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	delete(t.active, convKey{channelID, userID})
}

// ── text utilities ─────────────────────────────────────────────────────────────

func splitText(text string, limit int) []string {
	runes := []rune(text)
	if len(runes) <= limit {
		return []string{text}
	}
	var chunks []string
	start := 0
	for start < len(runes) {
		if len(runes)-start <= limit {
			chunks = append(chunks, string(runes[start:]))
			break
		}
		end := start + limit
		split := end
		for i := end - 1; i > start; i-- {
			if runes[i] == '\n' {
				split = i
				break
			}
		}
		chunks = append(chunks, string(runes[start:split]))
		for split < len(runes) && runes[split] == '\n' {
			split++
		}
		start = split
	}
	return chunks
}

func langExt(lang string) string {
	switch lang {
	case "python", "py":       return ".py"
	case "javascript", "js":   return ".js"
	case "typescript", "ts":   return ".ts"
	case "bash", "sh", "shell": return ".sh"
	case "rust":               return ".rs"
	case "go":                 return ".go"
	case "java":               return ".java"
	case "c":                  return ".c"
	case "cpp", "c++":         return ".cpp"
	case "html":               return ".html"
	case "json":               return ".json"
	case "ruby", "rb":         return ".rb"
	case "php":                return ".php"
	default:                   return ".txt"
	}
}

type codeFile struct {
	Name    string
	Content []byte
}

func extractCodeFiles(text string) (string, []codeFile) {
	var files []codeFile
	counter := 0
	modified := codeFenceRe.ReplaceAllStringFunc(text, func(match string) string {
		sub := codeFenceRe.FindStringSubmatch(match)
		if len(sub) < 3 { return match }
		lang := strings.ToLower(sub[1])
		code := sub[2]
		if utf8.RuneCountInString(code) < codeFileThreshold {
			return match
		}
		counter++
		name := fmt.Sprintf("script_%d%s", counter, langExt(lang))
		files = append(files, codeFile{Name: name, Content: []byte(code)})
		return fmt.Sprintf("*(see attached: `%s`)*", name)
	})
	return modified, files
}

// ── deduplication ──────────────────────────────────────────────────────────────

type deduper struct {
	mu         sync.Mutex
	processing map[string]bool
	seen       []string
}

func newDeduper() *deduper {
	return &deduper{processing: make(map[string]bool)}
}

func (d *deduper) tryAcquire(id string) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, s := range d.seen {
		if s == id { return false }
	}
	if d.processing[id] { return false }
	d.processing[id] = true
	return true
}

func (d *deduper) release(id string) {
	d.mu.Lock()
	defer d.mu.Unlock()
	delete(d.processing, id)
	if len(d.seen) >= 200 {
		d.seen = d.seen[1:]
	}
	d.seen = append(d.seen, id)
}

// ── bot ────────────────────────────────────────────────────────────────────────

type Bot struct {
	cfg      *config.Config
	agent    *agentclient.Client
	storage  *storageclient.Client
	redactor *SecretRedactor
	convos   *ConversationTracker
	dedup    *deduper
	session  *discordgo.Session
}

func New(cfg *config.Config) *Bot {
	return &Bot{
		cfg:      cfg,
		agent:    agentclient.NewClient(cfg.AgentURL),
		storage:  storageclient.NewClient(cfg.StorageURL),
		redactor: NewSecretRedactor(),
		convos:   NewConversationTracker(time.Duration(cfg.ConversationTimeout) * time.Second),
		dedup:    newDeduper(),
	}
}

func (b *Bot) Start() error {
	dg, err := discordgo.New("Bot " + b.cfg.DiscordToken)
	if err != nil {
		return fmt.Errorf("creating discord session: %w", err)
	}
	b.session = dg
	dg.AddHandler(b.onReady)
	dg.AddHandler(b.onMessage)
	dg.AddHandler(b.onInteraction)
	dg.Identify.Intents = discordgo.IntentsGuilds | discordgo.IntentsGuildMessages |
		discordgo.IntentsDirectMessages | discordgo.IntentsMessageContent
	if err := dg.Open(); err != nil {
		return fmt.Errorf("opening discord connection: %w", err)
	}
	return nil
}

func (b *Bot) Stop() {
	if b.session != nil {
		b.session.Close()
	}
}

func (b *Bot) onReady(s *discordgo.Session, r *discordgo.Ready) {
	fmt.Printf("Logged in as %s (%s)\n", r.User.Username, r.User.ID)
	b.registerSlashCommands(s)
	go b.reminderLoop(s)
}

func (b *Bot) registerSlashCommands(s *discordgo.Session) {
	cmds := []*discordgo.ApplicationCommand{
		{Name: "model",   Description: "Show the current model info"},
		{Name: "reset",   Description: "Clear conversation history"},
		{Name: "compact", Description: "Summarize conversation and start fresh"},
		{Name: "stats",   Description: "Show your stats"},
	}
	for _, cmd := range cmds {
		if _, err := s.ApplicationCommandCreate(s.State.User.ID, "", cmd); err != nil {
			fmt.Printf("Failed to register command %s: %v\n", cmd.Name, err)
		}
	}
}

func (b *Bot) onInteraction(s *discordgo.Session, i *discordgo.InteractionCreate) {
	if i.Type != discordgo.InteractionApplicationCommand { return }
	userID := i.Member.User.ID
	if i.Member == nil { userID = i.User.ID }

	var reply string
	switch i.ApplicationCommandData().Name {
	case "model":
		info, err := b.agent.ModelInfo()
		if err != nil { info = "Error fetching model info." }
		reply = info
	case "reset":
		_ = b.agent.Reset(userID)
		b.convos.Remove(i.ChannelID, userID)
		reply = "Session reset. Your conversation history has been cleared."
	case "compact":
		_ = b.agent.Compact(userID)
		b.convos.Remove(i.ChannelID, userID)
		reply = "Conversation compacted into memory. A new session has started."
	default:
		return
	}

	s.InteractionRespond(i.Interaction, &discordgo.InteractionResponse{
		Type: discordgo.InteractionResponseChannelMessageWithSource,
		Data: &discordgo.InteractionResponseData{
			Content: reply,
			Flags:   discordgo.MessageFlagsEphemeral,
		},
	})
}

func (b *Bot) onMessage(s *discordgo.Session, m *discordgo.MessageCreate) {
	if m.Author.Bot { return }
	content := strings.TrimSpace(m.Content)
	userID  := m.Author.ID
	chanID  := m.ChannelID

	// commands
	switch {
	case content == "!reset":
		_ = b.agent.Reset(userID)
		b.convos.Remove(chanID, userID)
		b.reply(s, m.Message, "Session reset. Your conversation history has been cleared.")
		return
	case content == "!compact":
		_ = b.agent.Compact(userID)
		b.convos.Remove(chanID, userID)
		b.reply(s, m.Message, "Conversation compacted. A new session has started.")
		return
	case strings.HasPrefix(content, "!skill"):
		b.handleSkillCmd(s, m.Message, content)
		return
	case strings.HasPrefix(content, "!note"):
		b.handleNoteCmd(s, m.Message, content)
		return
	}

	// routing
	botID := s.State.User.ID
	isDM  := m.GuildID == ""
	mentioned := false
	for _, u := range m.Mentions {
		if u.ID == botID { mentioned = true; break }
	}
	isReplyToBot := m.ReferencedMessage != nil &&
		m.ReferencedMessage.Author.ID == botID
	isActive := b.convos.IsActive(chanID, userID)

	if !(isDM || mentioned || isReplyToBot || isActive) { return }
	if !b.dedup.tryAcquire(m.ID) { return }
	defer b.dedup.release(m.ID)

	go b.handleMessage(s, m.Message, botID)
}

func (b *Bot) handleMessage(s *discordgo.Session, m *discordgo.Message, botID string) {
	text := m.Content
	for _, tok := range []string{fmt.Sprintf("<@%s>", botID), fmt.Sprintf("<@!%s>", botID)} {
		text = strings.ReplaceAll(text, tok, "")
	}
	text = strings.TrimSpace(text)
	if text == "" && len(m.Attachments) == 0 { return }

	// download image attachments
	var images []agentclient.ImageData
	for _, att := range m.Attachments {
		ext := strings.ToLower(att.Filename[strings.LastIndex(att.Filename, ".")+1:])
		var mt string
		switch ext {
		case "png":        mt = "image/png"
		case "jpg", "jpeg": mt = "image/jpeg"
		case "gif":        mt = "image/gif"
		case "webp":       mt = "image/webp"
		default:           continue
		}
		if resp, err := http.Get(att.URL); err == nil {
			if data, err := io.ReadAll(resp.Body); err == nil {
				images = append(images, agentclient.ImageData{
					MediaType: mt,
					Data:      base64.StdEncoding.EncodeToString(data),
				})
			}
			resp.Body.Close()
		}
	}

	if text == "" { text = "(no text)" }

	// progress message
	progress, _ := s.ChannelMessageSendReply(m.ChannelID, "⚙️ **Generating...**", m.Reference())

	result, err := b.agent.Run(agentclient.RunRequest{
		UserID:   m.Author.ID,
		Username: m.Author.Username,
		Text:     text,
		Images:   images,
	})
	if err != nil {
		if progress != nil {
			s.ChannelMessageEdit(m.ChannelID, progress.ID, "Sorry, something went wrong.")
		} else {
			b.reply(s, m, "Sorry, something went wrong.")
		}
		return
	}

	b.convos.MarkActive(m.ChannelID, m.Author.ID)

	if result.SessionNotice != nil {
		b.reply(s, m, *result.SessionNotice)
	}

	safe := b.redactor.Redact(result.Text)
	display, codeFiles := extractCodeFiles(safe)
	chunks := splitText(display, maxMessageLength)

	if progress != nil {
		s.ChannelMessageEdit(m.ChannelID, progress.ID, chunks[0])
		for _, chunk := range chunks[1:] {
			s.ChannelMessageSend(m.ChannelID, chunk)
		}
	} else {
		b.reply(s, m, chunks[0])
		for _, chunk := range chunks[1:] {
			s.ChannelMessageSend(m.ChannelID, chunk)
		}
	}

	for _, f := range codeFiles {
		safe := b.redactor.Redact(string(f.Content))
		s.ChannelFileSend(m.ChannelID, f.Name, strings.NewReader(safe))
	}
}

func (b *Bot) reply(s *discordgo.Session, m *discordgo.Message, content string) {
	s.ChannelMessageSendReply(m.ChannelID, content, m.Reference())
}

// ── !skill command handler ──────────────────────────────────────────────────────

func (b *Bot) handleSkillCmd(s *discordgo.Session, m *discordgo.Message, content string) {
	first, rest := splitCommand(content)
	parts := strings.Fields(first)
	if len(parts) < 2 {
		b.reply(s, m, "Usage: `!skill <list|add|delete|show> [name]`")
		return
	}
	switch parts[1] {
	case "list":
		skills, err := b.storage.LoadSkills()
		if err != nil || len(skills) == 0 {
			b.reply(s, m, "No skills defined yet.")
			return
		}
		var lines []string
		for name, sk := range skills {
			desc := name
			if sk.Description != nil { desc = *sk.Description }
			lines = append(lines, fmt.Sprintf("**%s** — %s", name, desc))
		}
		b.reply(s, m, strings.Join(lines, "\n"))
	case "add":
		if len(parts) < 3 {
			b.reply(s, m, "Usage: `!skill add <name>\\n<prompt>`")
			return
		}
		name := strings.ToLower(parts[2])
		if !isValidSkillName(name) {
			b.reply(s, m, "Skill names must be lowercase letters, numbers, and underscores only.")
			return
		}
		if rest == "" {
			b.reply(s, m, "Please provide the skill prompt on the next line.")
			return
		}
		createdBy := m.Author.Username
		if err := b.storage.SaveSkill(name, rest, nil, &createdBy); err != nil {
			b.reply(s, m, "Error saving skill: "+err.Error())
			return
		}
		b.reply(s, m, fmt.Sprintf("Skill **%s** saved.", name))
	case "delete":
		if len(parts) < 3 {
			b.reply(s, m, "Usage: `!skill delete <name>`")
			return
		}
		name := parts[2]
		if err := b.storage.DeleteSkill(name); err != nil {
			b.reply(s, m, "Error: not found or delete failed.")
			return
		}
		b.reply(s, m, fmt.Sprintf("Skill **%s** deleted.", name))
	default:
		b.reply(s, m, "Unknown subcommand. Use list, add, or delete.")
	}
}

func isValidSkillName(name string) bool {
	if name == "" { return false }
	for _, c := range name {
		if !(c >= 'a' && c <= 'z') && !(c >= '0' && c <= '9') && c != '_' {
			return false
		}
	}
	return true
}

// ── !note command handler ──────────────────────────────────────────────────────

func (b *Bot) handleNoteCmd(s *discordgo.Session, m *discordgo.Message, content string) {
	first, rest := splitCommand(content)
	parts := strings.Fields(first)
	if len(parts) < 2 {
		b.reply(s, m, "Usage: `!note <list|save|get|delete> [name]`")
		return
	}
	userID := m.Author.ID
	switch parts[1] {
	case "list":
		notes, err := b.storage.LoadNotes(userID)
		if err != nil || len(notes) == 0 {
			b.reply(s, m, "You have no saved notes.")
			return
		}
		var names []string
		for k := range notes { names = append(names, k) }
		b.reply(s, m, "Your notes: "+strings.Join(names, ", "))
	case "save":
		if len(parts) < 3 || rest == "" {
			b.reply(s, m, "Usage: `!note save <name>\\n<content>`")
			return
		}
		if err := b.storage.SaveNote(userID, parts[2], rest); err != nil {
			b.reply(s, m, "Error saving note: "+err.Error())
			return
		}
		b.reply(s, m, fmt.Sprintf("Note **%s** saved.", parts[2]))
	case "get":
		if len(parts) < 3 {
			b.reply(s, m, "Usage: `!note get <name>`")
			return
		}
		notes, err := b.storage.LoadNotes(userID)
		if err != nil {
			b.reply(s, m, "Error loading notes.")
			return
		}
		content, ok := notes[parts[2]]
		if !ok {
			b.reply(s, m, fmt.Sprintf("Note **%s** not found.", parts[2]))
			return
		}
		b.reply(s, m, content)
	case "delete":
		if len(parts) < 3 {
			b.reply(s, m, "Usage: `!note delete <name>`")
			return
		}
		if err := b.storage.DeleteNote(userID, parts[2]); err != nil {
			b.reply(s, m, "Error deleting note.")
			return
		}
		b.reply(s, m, fmt.Sprintf("Note **%s** deleted.", parts[2]))
	default:
		b.reply(s, m, "Unknown subcommand. Use list, save, get, or delete.")
	}
}

// ── reminder delivery loop ─────────────────────────────────────────────────────

func (b *Bot) reminderLoop(s *discordgo.Session) {
	for range time.Tick(30 * time.Second) {
		b.deliverDueReminders(s)
	}
}

func (b *Bot) deliverDueReminders(s *discordgo.Session) {
	resp, err := http.Get(b.cfg.RemindersURL + "/reminders/due")
	if err != nil { return }
	defer resp.Body.Close()

	var v struct {
		Due []struct {
			UserID  string  `json:"user_id"`
			Message string  `json:"message"`
			DueTs   float64 `json:"due_ts"`
		} `json:"due"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&v); err != nil { return }

	for _, r := range v.Due {
		ch, err := s.UserChannelCreate(r.UserID)
		if err != nil { continue }
		s.ChannelMessageSend(ch.ID, "⏰ **Reminder:** "+r.Message)
	}
}

func splitCommand(content string) (string, string) {
	idx := strings.Index(content, "\n")
	if idx < 0 {
		return strings.TrimSpace(content), ""
	}
	return strings.TrimSpace(content[:idx]), strings.TrimSpace(content[idx+1:])
}
