//! Session lifecycle: reset, compaction, token accounting, and conversation bookkeeping.

use super::*;

impl Agent {
    // ── session lifecycle ────────────────────────────────────────────────────

    /// Clear conversation history and counters without preserving a summary.
    pub async fn reset_session(&self, user_id: &str) {
        self.session_stats.lock().await.remove(user_id);
        let _ = self.history.clear(user_id).await;
        self.finish_active_conversation(user_id).await;
    }

    /// Summarize the current conversation, then start a fresh session.
    pub async fn compact_session(&self, user_id: &str, deep_memory_enabled: bool) {
        self.compact_session_with_hooks(user_id, deep_memory_enabled, &NoHooks)
            .await;
    }

    /// Summarize the current conversation, reporting coarse-grained progress to the caller.
    pub async fn compact_session_with_hooks(
        &self,
        user_id: &str,
        deep_memory_enabled: bool,
        hooks: &dyn AgentHooks,
    ) {
        tracing::info!(target: "housebot::agent", user_id, "Compacting session");
        hooks.on_progress("compact:10").await;
        self.session_stats.lock().await.remove(user_id);
        let past = self.history.load(user_id).await;
        if past.is_empty() {
            self.finish_active_conversation(user_id).await;
            hooks.on_progress("compact:100:Nothing to compact.").await;
            return;
        }
        let conversation_id = self.current_conversation_id(user_id, user_id, 0).await;
        if !deep_memory_enabled {
            let _ = self.history.clear(user_id).await;
            self.finish_active_conversation(user_id).await;
            hooks
                .on_progress("compact:100:Conversation cleared without saving a memory summary.")
                .await;
            return;
        }
        hooks.on_progress("compact:25").await;
        let user_memory = self.memory.load(user_id).await;
        let convo: String = past
            .iter()
            .filter_map(|m| {
                let role = m.get("role").and_then(|r| r.as_str())?;
                let content = m.get("content").and_then(|c| c.as_str())?;
                Some(format!("{}: {}", role.to_uppercase(), content))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let truncated: String = convo.chars().take(6000).collect();
        let prompt = format!(
            "The following is a conversation that has ended. Write a concise bullet-point summary \
             of the key facts, preferences, and decisions discussed. This will be appended to the \
             user's memory for future reference. Be brief — 3-8 bullets max.\n\nCONVERSATION:\n{truncated}"
        );
        hooks.on_progress("compact:45").await;
        let completion = self
            .client
            .chat_once(
                &self.model,
                &[json!({"role": "user", "content": prompt})],
                512,
            )
            .await
            .unwrap_or_default();
        self.record_usage(user_id, &conversation_id, completion.usage)
            .await;
        let summary = completion.content.unwrap_or_default();

        if !summary.trim().is_empty() {
            let now = Local::now().format("%Y-%m-%d %H:%M");
            let mut updated = String::new();
            if !user_memory.trim().is_empty() {
                updated.push_str(user_memory.trim_end());
                updated.push_str("\n\n");
            }
            updated.push_str(&format!("## Conversation summary ({now})\n{summary}"));
            let _ = self.memory.save(user_id, &updated).await;
        }
        hooks.on_progress("compact:80").await;
        let _ = self.history.clear(user_id).await;
        self.finish_active_conversation(user_id).await;
        hooks
            .on_progress("compact:100:Conversation compacted.")
            .await;
    }

    pub fn model_info(&self) -> String {
        format!(
            "**Model**\nName: `{}`\nMax context: ~{} tokens",
            self.model, self.context_window_tokens
        )
    }

    pub async fn session_info(&self, user_id: &str) -> SessionInfo {
        let history = self.history.load(user_id).await;
        let context_window_tokens = self
            .client
            .context_window_tokens()
            .await
            .ok()
            .flatten()
            .map(|tokens| tokens as usize)
            .unwrap_or(self.context_window_tokens);
        let stats = self
            .session_stats
            .lock()
            .await
            .get(user_id)
            .copied()
            .unwrap_or_default();
        SessionInfo {
            context_tokens: stats.context_tokens as usize,
            context_window_tokens,
            messages: history.len(),
            requests: stats.requests,
            input_tokens: stats.input_tokens,
            output_tokens: stats.output_tokens,
            cached_tokens: stats.cached_tokens,
        }
    }

    /// Render the persistent global token leaderboard for Discord.
    pub async fn token_leaderboard(
        &self,
        period: LeaderboardPeriod,
        metric: LeaderboardMetric,
        requester_id: &str,
    ) -> String {
        match self
            .token_monitor
            .leaderboard_for(period, metric, 10, Some(requester_id))
            .await
        {
            Ok(leaderboard) => format_token_leaderboard(&leaderboard),
            Err(error) => {
                tracing::error!(%error, "failed to load token leaderboard");
                "⚠️ Token usage statistics are temporarily unavailable.".into()
            }
        }
    }

    /// Remove a user's archived conversations and token statistics.
    pub async fn clear_token_data(&self, user_id: &str) {
        if let Err(error) = self.token_monitor.clear_user(user_id).await {
            tracing::error!(%error, %user_id, "failed to erase token-monitor data");
        }
    }

    pub(crate) async fn last_context_tokens(&self, user_id: &str) -> u64 {
        self.session_stats
            .lock()
            .await
            .get(user_id)
            .map_or(0, |stats| stats.context_tokens)
    }

    pub(crate) async fn current_conversation_id(
        &self,
        user_id: &str,
        display_name: &str,
        channel_id: u64,
    ) -> String {
        let mut active = self.active_conversations.lock().await;
        if let Some(id) = active.get(user_id) {
            return id.clone();
        }
        // After a restart the in-memory map is empty. Try to recover the
        // active conversation from the database so token counts continue
        // accumulating on the same row and the leaderboard stays accurate.
        if let Some(id) = self.token_monitor.get_active_conversation_id(user_id).await {
            active.insert(user_id.to_string(), id.clone());
            return id;
        }
        let id = uuid::Uuid::new_v4().to_string();
        if let Err(error) = self
            .token_monitor
            .start_conversation(&id, user_id, display_name, channel_id)
            .await
        {
            tracing::error!(%error, %user_id, conversation_id = %id, "failed to persist conversation");
        }
        active.insert(user_id.to_string(), id.clone());
        id
    }

    async fn finish_active_conversation(&self, user_id: &str) {
        let id = self.active_conversations.lock().await.remove(user_id);
        if let Some(id) = id {
            if let Err(error) = self.token_monitor.finish_conversation(&id).await {
                tracing::error!(%error, %user_id, conversation_id = %id, "failed to close conversation");
            }
        }
    }

    pub(crate) async fn record_usage(
        &self,
        user_id: &str,
        conversation_id: &str,
        usage: TokenUsage,
    ) {
        if let Err(error) = self
            .token_monitor
            .record_usage(conversation_id, usage)
            .await
        {
            tracing::error!(%error, %user_id, %conversation_id, "failed to persist token usage");
        }
        let mut all = self.session_stats.lock().await;
        let stats = all.entry(user_id.to_string()).or_default();
        stats.requests += 1;
        stats.context_tokens = usage.prompt_tokens + usage.completion_tokens;
        stats.input_tokens += usage.prompt_tokens;
        stats.output_tokens += usage.completion_tokens;
        stats.cached_tokens += usage.prompt_tokens_details.cached_tokens;
    }
}
