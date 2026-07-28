//! Reminder and Discord lookup tools.

use super::*;
use crate::discord_bridge::MessageAnchor;

impl Agent {
    pub(super) async fn dispatch_discord(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        channel_id: u64,
    ) -> Option<ToolOutcome> {
        let outcome = match name {
            "set_reminder" => {
                let delay = args
                    .get("delay_minutes")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                ToolOutcome::Text(
                    tools::remind::create_reminder(
                        &self.reminders,
                        user_id,
                        str_arg(args, "message"),
                        delay,
                    )
                    .await,
                )
            }
            "get_messages" => {
                let mode = args.get("mode").and_then(Value::as_str).unwrap_or("recent");
                let target_channel = args
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(channel_id);
                ToolOutcome::Text(match mode {
                    "search" => {
                        let pattern = str_arg(args, "pattern");
                        let limit = u64_arg(args, "limit", 10).clamp(1, 100) as usize;
                        match self
                            .channel_log
                            .search(target_channel, pattern, limit)
                            .await
                        {
                            Err(e) => format!("Error: {e}"),
                            Ok(msgs) if msgs.is_empty() => {
                                "No matching messages found.".to_string()
                            }
                            Ok(msgs) => msgs
                                .iter()
                                .map(|m| {
                                    let author = m.nick.as_deref().unwrap_or(&m.username);
                                    format!("[{}] {}: {}", m.ts, author, m.content)
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                        }
                    }
                    "before" | "after" | "around" => {
                        let message_id: Option<u64> = str_arg(args, "message_id")
                            .parse()
                            .ok()
                            .filter(|&id| id != 0);
                        match message_id {
                            None => "Error: invalid message_id.".to_string(),
                            Some(message_id) => {
                                let anchor = match mode {
                                    "before" => MessageAnchor::Before(message_id),
                                    "after" => MessageAnchor::After(message_id),
                                    _ => MessageAnchor::Around(message_id),
                                };
                                let limit = u64_arg(args, "limit", 20).clamp(1, 100) as u8;
                                match self
                                    .discord
                                    .fetch_messages(target_channel, anchor, limit)
                                    .await
                                {
                                    Err(e) => format!("Error: {e}"),
                                    Ok(msgs) if msgs.is_empty() => "No messages found.".to_string(),
                                    Ok(msgs) => msgs
                                        .iter()
                                        .map(|m| {
                                            format!(
                                                "[{}] [{}] {}: {}",
                                                m.id, m.ts, m.author, m.content
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                }
                            }
                        }
                    }
                    _ => {
                        let minutes = u64_arg(args, "minutes", 30).clamp(1, 1440) as u32;
                        match self
                            .discord
                            .fetch_messages_recent(target_channel, minutes)
                            .await
                        {
                            Err(e) => format!("Error: {e}"),
                            Ok(msgs) if msgs.is_empty() => {
                                format!("No messages found in the last {minutes} minutes.")
                            }
                            Ok(msgs) => msgs
                                .iter()
                                .map(|m| {
                                    format!("[{}] [{}] {}: {}", m.id, m.ts, m.author, m.content)
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                        }
                    }
                })
            }
            "find_discord_users" => {
                let query = str_arg(args, "query");
                let max_results = u64_arg(args, "max_results", 10).clamp(1, 20) as usize;
                let target_channel = args
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(channel_id);
                ToolOutcome::Text(
                    match self
                        .channel_log
                        .find_authors(target_channel, query, max_results)
                        .await
                    {
                        Err(error) => format!("Error: {error}"),
                        Ok(authors) if authors.is_empty() => {
                            "No matching Discord users found in this channel's history.".to_string()
                        }
                        Ok(authors) => authors
                            .iter()
                            .map(|author| {
                                let nick = author.nick.as_deref().unwrap_or("(none)");
                                format!(
                                    "Username: {} | Nickname: {} | ID: {}",
                                    author.username, nick, author.user_id
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                )
            }
            "get_discord_user" => {
                let uid: u64 = str_arg(args, "user_id").parse().unwrap_or(0);
                ToolOutcome::Text(if uid == 0 {
                    "Error: invalid user_id.".to_string()
                } else {
                    match self.discord.fetch_user(uid).await {
                        Ok(u) => {
                            let avatar = u.avatar_url.as_deref().unwrap_or("(none)");
                            format!(
                            "Username: {}\nDisplay name: {}\nID: {}\nBot: {}\nAccount created: {}\nAvatar URL: {}",
                            u.username, u.display_name, u.id, u.bot, u.created_at, avatar
                        )
                        }
                        Err(e) => format!("Error: {e}"),
                    }
                })
            }
            _ => return None,
        };
        Some(outcome)
    }
}
