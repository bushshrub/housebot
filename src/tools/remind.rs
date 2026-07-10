//! Agent tool for creating timed reminders.

use serde_json::{json, Value};

use crate::reminders::Reminders;

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "set_reminder",
        "description": "Set a timed reminder for the current user. The bot will DM them the \
            message when the delay elapses. Use this whenever a user asks to be reminded about \
            something later.",
        "input_schema": {
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "The reminder message to send to the user."},
                "delay_minutes": {"type": "number", "description": "How many minutes from now to deliver the reminder (minimum 1, maximum 43200)."}
            },
            "required": ["message", "delay_minutes"]
        }
    })
}

/// Format a whole-minute delay the way the confirmation message expects.
pub fn format_delay(delay_minutes: i64) -> String {
    let hours = delay_minutes / 60;
    let mins = delay_minutes % 60;
    if hours > 0 && mins > 0 {
        format!("{hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h")
    } else {
        format!("{mins}m")
    }
}

/// Persist a reminder and return a user-facing confirmation (or an `Error:` string).
pub async fn create_reminder(
    reminders: &Reminders,
    user_id: &str,
    message: &str,
    delay_minutes: f64,
) -> String {
    if delay_minutes < 1.0 {
        return "Error: delay_minutes must be at least 1.".to_string();
    }
    if delay_minutes > 43200.0 {
        return "Error: delay_minutes cannot exceed 43200 (30 days).".to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let due_ts = now + delay_minutes * 60.0;
    if reminders.add(user_id, message, due_ts).await.is_err() {
        return "Error: failed to store reminder.".to_string();
    }

    format!(
        "✅ Reminder set! I'll DM you in {}.",
        format_delay(delay_minutes as i64)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, Reminders) {
        let tmp = TempDir::new().unwrap();
        let r = Reminders::new(tmp.path().join("reminders.json"));
        (tmp, r)
    }

    #[test]
    fn format_delay_minutes_only() {
        assert_eq!(format_delay(30), "30m");
    }

    #[test]
    fn format_delay_hours_and_minutes() {
        assert_eq!(format_delay(90), "1h 30m");
    }

    #[test]
    fn format_delay_exact_hours() {
        assert_eq!(format_delay(120), "2h");
    }

    #[tokio::test]
    async fn returns_confirmation() {
        let (_t, r) = store();
        let out = create_reminder(&r, "42", "feed the cat", 30.0).await;
        assert!(out.contains("Reminder set"));
        assert!(out.contains("30m"));
    }

    #[tokio::test]
    async fn stores_reminder() {
        let (_t, r) = store();
        create_reminder(&r, "7", "test", 10.0).await;
        let due = r.pop_due(f64::MAX).await;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].user_id, "7");
        assert_eq!(due[0].message, "test");
    }

    #[tokio::test]
    async fn delay_below_minimum_returns_error() {
        let (_t, r) = store();
        assert!(create_reminder(&r, "1", "now", 0.0)
            .await
            .starts_with("Error:"));
    }

    #[tokio::test]
    async fn delay_above_maximum_returns_error() {
        let (_t, r) = store();
        assert!(create_reminder(&r, "1", "far", 99999.0)
            .await
            .starts_with("Error:"));
    }

    #[test]
    fn definition_has_required_fields() {
        let d = definition();
        assert_eq!(d["name"], "set_reminder");
        let props = &d["input_schema"]["properties"];
        assert!(props.get("message").is_some());
        assert!(props.get("delay_minutes").is_some());
        assert_eq!(
            d["input_schema"]["required"],
            json!(["message", "delay_minutes"])
        );
    }
}
