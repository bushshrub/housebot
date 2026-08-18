//! The sub-agent loop: a bounded research turn scheduled below user chat.

use super::*;

use crate::llm_scheduler::Priority;
use crate::tools::subagent::{system_prompt, task_message, MAX_SUBAGENT_ROUNDS};

impl Agent {
    /// Tools a sub-agent may call. Deliberately narrow: read-only web access
    /// and nothing that touches the user, persistent state, or another spawn.
    pub(crate) fn subagent_tools(&self) -> Vec<Value> {
        [tools::searxng::definition(), tools::web_fetch::definition()]
            .iter()
            .map(|def| {
                let (name, desc, params) = flatten_tool(def);
                to_openai_tool(&name, &desc, params)
            })
            .collect()
    }

    /// Run a delegated task to completion and return its report.
    ///
    /// Token usage is recorded against the parent's conversation: the user who
    /// caused the spawn owns its cost.
    pub(crate) async fn run_subagent(
        &self,
        task: &str,
        context: &str,
        user_id: &str,
        conversation_id: &str,
        hooks: &dyn AgentHooks,
    ) -> String {
        let task = task.trim();
        if task.is_empty() {
            return "Error: task cannot be empty.".to_string();
        }

        let started = std::time::Instant::now();
        tracing::info!(
            target: "housebot::subagent",
            user_id,
            task_chars = task.chars().count(),
            "Sub-agent started"
        );

        let client = self.scheduled_client.with_priority(Priority::SubAgent);
        let tools = self.subagent_tools();
        let mut messages = vec![
            json!({"role": "system", "content": system_prompt()}),
            json!({"role": "user", "content": task_message(task, context)}),
        ];

        let mut rounds = 0;
        let report = loop {
            rounds += 1;
            if rounds > MAX_SUBAGENT_ROUNDS {
                break "The sub-agent hit its tool-round limit before reaching an answer."
                    .to_string();
            }

            let completion = match client
                .chat_stream(
                    &self.model,
                    &messages,
                    &tools,
                    None,
                    ThinkingMode::Low,
                    None,
                    None,
                )
                .await
            {
                Ok(completion) => completion,
                Err(error) => {
                    tracing::error!(target: "housebot::subagent", %error, "Sub-agent LLM error");
                    break "The sub-agent could not reach the model.".to_string();
                }
            };
            self.record_usage(user_id, conversation_id, completion.usage)
                .await;

            let mut assistant = json!({"role": "assistant", "content": completion.content});
            if !completion.tool_calls.is_empty() {
                assistant["tool_calls"] = Value::Array(
                    completion
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {"name": tc.name, "arguments": tc.arguments},
                            })
                        })
                        .collect(),
                );
            }
            messages.push(assistant);

            if completion.tool_calls.is_empty() {
                break completion.content.unwrap_or_default();
            }

            let mut rate_limited = false;
            for tc in &completion.tool_calls {
                let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                hooks.on_subagent_tool_called(&tc.name, &args).await;
                let content = self.dispatch_subagent_tool(&tc.name, &args).await;
                if tc.name == "web_search" && search_rate_limited(&content) {
                    rate_limited = true;
                }
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": content,
                }));
            }
            if rate_limited {
                break "The sub-agent stopped because web search is rate-limited.".to_string();
            }
        };

        tracing::info!(
            target: "housebot::subagent",
            user_id,
            rounds,
            report_chars = report.chars().count(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Sub-agent finished"
        );

        if report.trim().is_empty() {
            "The sub-agent returned an empty report.".to_string()
        } else {
            report
        }
    }

    async fn dispatch_subagent_tool(&self, name: &str, args: &Value) -> String {
        match name {
            "web_search" => {
                self.searxng
                    .search(
                        str_arg(args, "query"),
                        u64_arg(args, "max_results", 10) as usize,
                        str_arg(args, "language"),
                    )
                    .await
            }
            "fetch_webpage" => {
                self.web_fetch
                    .fetch_content(
                        str_arg(args, "url"),
                        u64_arg(args, "start_index", 0) as usize,
                        u64_arg(args, "max_length", 8000) as usize,
                    )
                    .await
            }
            other => format!("Error: unknown tool '{other}'."),
        }
    }
}
