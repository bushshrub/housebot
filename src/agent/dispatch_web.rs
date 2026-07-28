//! Web search, fetching, translation, and bot-info tools.

use super::*;

impl Agent {
    pub(super) async fn dispatch_web(&self, name: &str, args: &Value) -> Option<ToolOutcome> {
        let outcome = match name {
            "web_search" => ToolOutcome::Text(
                self.searxng
                    .search(
                        str_arg(args, "query"),
                        u64_arg(args, "max_results", 10) as usize,
                        str_arg(args, "language"),
                    )
                    .await,
            ),
            "deep_research" => {
                let questions: Vec<String> = args
                    .get("questions")
                    .and_then(Value::as_array)
                    .map(|questions| {
                        questions
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                ToolOutcome::Text(
                    self.searxng
                        .deep_research(
                            str_arg(args, "topic"),
                            &questions,
                            u64_arg(args, "max_results_per_query", 5) as usize,
                            str_arg(args, "language"),
                        )
                        .await,
                )
            }
            "fetch_webpage" => ToolOutcome::Text(
                self.web_fetch
                    .fetch_content(
                        str_arg(args, "url"),
                        u64_arg(args, "start_index", 0) as usize,
                        u64_arg(args, "max_length", 8000) as usize,
                    )
                    .await,
            ),
            "download_file" => match self
                .file_downloader
                .download(str_arg(args, "url"), str_arg(args, "filename"))
                .await
            {
                Ok(file) => ToolOutcome::Attachment {
                    text: format!(
                        "Attached `{}` ({} bytes{}) to the Discord response.",
                        file.filename,
                        file.bytes.len(),
                        file.content_type
                            .as_deref()
                            .map(|content_type| format!(", {content_type}"))
                            .unwrap_or_default()
                    ),
                    attachment: AgentAttachment {
                        filename: file.filename,
                        bytes: file.bytes,
                    },
                },
                Err(error) => ToolOutcome::Text(error),
            },
            "common_crawl__search" => ToolOutcome::Text(
                self.common_crawl
                    .search(
                        str_arg(args, "pattern"),
                        str_arg(args, "crawl"),
                        args.get("match_type")
                            .and_then(Value::as_str)
                            .unwrap_or("exact"),
                        u64_arg(args, "max_results", 10) as usize,
                    )
                    .await,
            ),
            "summarize_url" => ToolOutcome::Text(
                tools::summarize_url::fetch_and_summarize(
                    &*self.client,
                    &self.model,
                    str_arg(args, "url"),
                )
                .await,
            ),
            "translate" => ToolOutcome::Text(
                tools::translate::translate_text(
                    &*self.client,
                    &self.model,
                    str_arg(args, "text"),
                    str_arg(args, "target_language"),
                )
                .await,
            ),
            "get_bot_features" => ToolOutcome::Text(tools::features::features_text().to_string()),
            "get_token_metrics" => ToolOutcome::Text(
                tools::token_metrics::get_token_metrics(
                    &self.token_monitor,
                    args.get("user_id").and_then(Value::as_str),
                    args.get("period").and_then(Value::as_str),
                    args.get("metric").and_then(Value::as_str),
                )
                .await,
            ),
            _ => return None,
        };
        Some(outcome)
    }
}
