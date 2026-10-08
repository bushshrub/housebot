//! The classifier: a System One decision model that decides, before the main
//! model is called, whether a message gets nothing, one emoji reaction, or a
//! full answer. One request asks every question the decision needs.

use std::collections::HashMap;
use std::time::Duration;

use super::*;
use crate::bot_config::ClassifierSettings;
use crate::channel_context::Message;
use crate::llm::{Answer, SystemOneClient};

/// How many stored channel messages the classifier reads, newest last.
pub const CLASSIFIER_CONTEXT_MESSAGES: usize = 10;
const CLASSIFIER_TIMEOUT: Duration = Duration::from_secs(10);

/// Options of the emoji question. A choice model only picks among fixed
/// options, so the bot can react only with these.
const EMOJI: [(&str, &str); 6] = [
    ("👍", "Agreement, acknowledgement, or ok."),
    ("❤️", "Affection, kindness, or appreciation of someone."),
    ("😂", "A joke or something funny."),
    ("🎉", "Good news, an achievement, or a celebration."),
    ("👋", "A greeting or goodbye."),
    ("🙏", "Thanks or gratitude."),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveAction {
    Ignore,
    React(String),
    Escalate,
}

pub(crate) struct Classifier {
    settings: ClassifierSettings,
    client: SystemOneClient,
}

impl Classifier {
    pub(crate) fn new(settings: ClassifierSettings) -> Self {
        let llm_base_url = config::env_or("LLM_BASE_URL", "http://server-slop:8080/v1");
        // A configurer, not only the owner, can set the URL, so the key goes
        // only to the gateway it was issued for.
        let api_key = if same_origin(&settings.url, &llm_base_url) {
            config::env_or("LLM_API_KEY", "not-required")
        } else {
            tracing::warn!(
                target: "housebot::classifier",
                url = settings.url,
                "Classifier URL is not on the LLM gateway; sending no API key"
            );
            "not-required".to_string()
        };
        let client = SystemOneClient::new(&settings.url, api_key);
        Self { settings, client }
    }
}

/// Same scheme, host, and port, so a key never leaves its host or drops to
/// plain HTTP.
fn same_origin(a: &str, b: &str) -> bool {
    match (reqwest::Url::parse(a), reqwest::Url::parse(b)) {
        (Ok(a), Ok(b)) => a.origin().is_tuple() && a.origin() == b.origin(),
        _ => false,
    }
}

/// The state document for the classifier: the channel's recent messages,
/// with pings of the bot written as `@<bot_name>` so the model sees who was
/// addressed.
pub fn classifier_state(messages: &[Message], bot_id: u64, bot_name: &str) -> String {
    let mut state = format!(
        "Recent messages in a Discord channel, oldest first. The assistant is named {bot_name}.\n"
    );
    for message in messages {
        let content = message
            .content
            .replace(&format!("<@{bot_id}>"), &format!("@{bot_name}"))
            .replace(&format!("<@!{bot_id}>"), &format!("@{bot_name}"));
        let name = message.nick.as_deref().unwrap_or(&message.username);
        state.push_str(&format!("\n{name}: {content}"));
    }
    state
}

fn emoji_question() -> Value {
    let criteria: serde_json::Map<String, Value> = EMOJI
        .iter()
        .map(|(emoji, meaning)| (emoji.to_string(), json!(meaning)))
        .collect();
    json!({
        "type": "choice",
        "instructions": "Pick the one emoji reaction that best fits the LAST message.",
        "criteria": criteria,
    })
}

fn ping_questions(bot_name: &str) -> Value {
    json!({
        "action": {
            "type": "choice",
            "instructions": format!("{bot_name} was pinged in the LAST message. Can one emoji reaction fully answer it?"),
            "criteria": {
                "react": "A greeting, thanks, joke or acknowledgement needing no information or action.",
                "escalate": "A question, request, command, or anything ambiguous that needs a written answer.",
            },
        },
        "emoji": emoji_question(),
    })
}

fn proactive_questions() -> Value {
    json!({
        "answer": {
            "type": "choice",
            "instructions": "Does the LAST message contain a question, request, or wrong claim that is still unresolved and that an assistant could answer with facts or an action? Requests made to another person in the channel (not the assistant) count as no.",
            "criteria": {
                "yes": "Yes, there is an open question, request, or factual error.",
                "no": "No, it is chat, plans, or feelings.",
            },
        },
        "react": {
            "type": "choice",
            "instructions": "Does the LAST message share good news, an achievement, thanks, or a celebration?",
            "criteria": {"yes": "Yes, it is good news, thanks, or a celebration.", "no": "No."},
        },
        "emoji": emoji_question(),
    })
}

fn chosen_emoji(answers: &HashMap<String, Answer>) -> Option<String> {
    let choice = &answers.get("emoji")?.choice;
    EMOJI
        .iter()
        .any(|(emoji, _)| emoji == choice)
        .then(|| choice.clone())
}

/// `Some(emoji)` when one reaction fully answers the ping.
fn ping_decision(answers: &HashMap<String, Answer>) -> Option<String> {
    if answers.get("action")?.probability("react") < 0.5 {
        return None;
    }
    chosen_emoji(answers)
}

fn proactive_decision(answers: &HashMap<String, Answer>) -> ProactiveAction {
    let yes = |question: &str| answers.get(question).map_or(0.0, |a| a.probability("yes"));
    if yes("answer") >= 0.5 {
        return ProactiveAction::Escalate;
    }
    if yes("react") >= 0.5 {
        if let Some(emoji) = chosen_emoji(answers) {
            return ProactiveAction::React(emoji);
        }
    }
    ProactiveAction::Ignore
}

impl Agent {
    pub fn classifier_settings(&self) -> Option<ClassifierSettings> {
        self.classifier
            .read()
            .expect("classifier lock poisoned")
            .as_ref()
            .map(|classifier| classifier.settings.clone())
    }

    /// Save new classifier settings (`None` turns the classifier off) and use
    /// them from the next message on.
    pub async fn set_classifier(&self, settings: Option<ClassifierSettings>) -> anyhow::Result<()> {
        self.classifier_store.save(settings.as_ref()).await?;
        *self.classifier.write().expect("classifier lock poisoned") =
            settings.map(|settings| Arc::new(Classifier::new(settings)));
        Ok(())
    }

    /// `None` when the classifier is off or fails, so the caller can fall back.
    async fn classify(&self, state: &str, questions: &Value) -> Option<HashMap<String, Answer>> {
        let classifier = self
            .classifier
            .read()
            .expect("classifier lock poisoned")
            .clone()?;
        let model = &classifier.settings.model;
        let start = std::time::Instant::now();
        let call = classifier.client.decide(model, state, questions);
        match tokio::time::timeout(CLASSIFIER_TIMEOUT, call).await {
            Ok(Ok(answers)) => {
                tracing::debug!(
                    target: "housebot::classifier",
                    model,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    ?answers,
                    "Classifier answered"
                );
                Some(answers)
            }
            Ok(Err(error)) => {
                tracing::warn!(target: "housebot::classifier", %error, "Classifier call failed");
                None
            }
            Err(_) => {
                tracing::warn!(target: "housebot::classifier", "Classifier call timed out");
                None
            }
        }
    }

    /// For a ping: `Some(emoji)` when one reaction fully answers it. `None`
    /// means a full answer, which is also the fallback when the classifier is
    /// off or fails, because a ping always gets an answer.
    pub async fn classify_ping(&self, state: &str, bot_name: &str) -> Option<String> {
        let answers = self.classify(state, &ping_questions(bot_name)).await?;
        ping_decision(&answers)
    }

    /// For a message that does not address the bot, in a proactive channel.
    /// Falls back to ignoring the message.
    pub async fn classify_proactive(&self, state: &str) -> ProactiveAction {
        match self.classify(state, &proactive_questions()).await {
            Some(answers) => proactive_decision(&answers),
            None => ProactiveAction::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn answer(choice: &str, probabilities: &[(&str, f64)]) -> Answer {
        Answer {
            choice: choice.into(),
            probabilities: probabilities
                .iter()
                .map(|(option, p)| (option.to_string(), *p))
                .collect(),
        }
    }

    fn answers(entries: Vec<(&str, Answer)>) -> HashMap<String, Answer> {
        entries
            .into_iter()
            .map(|(question, answer)| (question.to_string(), answer))
            .collect()
    }

    #[test]
    fn the_api_key_only_goes_to_the_llm_gateway() {
        let gateway = "https://llm.example.net/v1";
        assert!(same_origin("https://llm.example.net/typesafe", gateway));
        assert!(!same_origin("https://attacker.example/typesafe", gateway));
        assert!(!same_origin("http://llm.example.net/typesafe", gateway));
        assert!(!same_origin(
            "https://llm.example.net:8443/typesafe",
            gateway
        ));
        assert!(!same_origin("not a url", gateway));
    }

    #[test]
    fn a_ping_gets_an_emoji_only_when_react_wins() {
        let react = answers(vec![
            (
                "action",
                answer("react", &[("react", 0.9), ("escalate", 0.1)]),
            ),
            ("emoji", answer("👋", &[])),
        ]);
        assert_eq!(ping_decision(&react), Some("👋".into()));
        let escalate = answers(vec![
            (
                "action",
                answer("escalate", &[("react", 0.2), ("escalate", 0.8)]),
            ),
            ("emoji", answer("👋", &[])),
        ]);
        assert_eq!(ping_decision(&escalate), None);
    }

    #[test]
    fn an_emoji_outside_the_fixed_set_is_never_used() {
        let unknown = answers(vec![
            ("action", answer("react", &[("react", 0.9)])),
            ("emoji", answer("🦀", &[])),
        ]);
        assert_eq!(ping_decision(&unknown), None);
    }

    #[test]
    fn proactive_answer_wins_over_react() {
        let both = answers(vec![
            ("answer", answer("yes", &[("yes", 0.6)])),
            ("react", answer("yes", &[("yes", 0.9)])),
            ("emoji", answer("🎉", &[])),
        ]);
        assert_eq!(proactive_decision(&both), ProactiveAction::Escalate);
        let react = answers(vec![
            ("answer", answer("no", &[("yes", 0.1)])),
            ("react", answer("yes", &[("yes", 0.9)])),
            ("emoji", answer("🎉", &[])),
        ]);
        assert_eq!(
            proactive_decision(&react),
            ProactiveAction::React("🎉".into())
        );
        let neither = answers(vec![
            ("answer", answer("no", &[("yes", 0.3)])),
            ("react", answer("no", &[("yes", 0.2)])),
            ("emoji", answer("🎉", &[])),
        ]);
        assert_eq!(proactive_decision(&neither), ProactiveAction::Ignore);
        assert_eq!(proactive_decision(&HashMap::new()), ProactiveAction::Ignore);
    }

    #[test]
    fn the_state_names_the_bot_and_prefers_nicknames() {
        let message = |nick: Option<&str>, content: &str| Message {
            at: Utc::now(),
            user_id: "1".into(),
            username: "amy_42".into(),
            nick: nick.map(str::to_string),
            content: content.into(),
        };
        let state = classifier_state(
            &[
                message(Some("Amy"), "hi <@99>"),
                message(None, "<@!99> thanks"),
            ],
            99,
            "housebot",
        );
        assert!(state.contains("assistant is named housebot"));
        assert!(state.ends_with("\nAmy: hi @housebot\namy_42: @housebot thanks"));
    }

    #[tokio::test]
    async fn without_settings_a_ping_falls_back_to_a_full_answer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let agent = Agent::for_test(
            Arc::new(crate::testing::MockChatClient::new()),
            History::new(tmp.path().join("history")),
            Memory::new(tmp.path().join("memories")),
            Skills::new(tmp.path().join("skills.json")),
            Reminders::new(tmp.path().join("reminders.json")),
        );
        assert_eq!(agent.classify_ping("state", "housebot").await, None);
        assert_eq!(
            agent.classify_proactive("state").await,
            ProactiveAction::Ignore
        );
    }
}
