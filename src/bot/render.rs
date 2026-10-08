//! Final-message delivery.

use serenity::all::MessageId;

use super::*;

pub(crate) fn split_command(content: &str) -> (String, String) {
    match content.split_once('\n') {
        Some((first, rest)) => (first.trim().to_string(), rest.trim().to_string()),
        None => (content.trim().to_string(), String::new()),
    }
}

fn build_allowed_mentions(allowed_pings: &[u64]) -> CreateAllowedMentions {
    let mut mentions = CreateAllowedMentions::new();
    if !allowed_pings.is_empty() {
        mentions = mentions.users(allowed_pings.iter().map(|id| UserId::new(*id)));
    }
    mentions
}

/// Send the final response message. Returns the MessageId of the primary
/// reply message when one was sent.
pub(crate) async fn send_final_message(
    ctx: &Context,
    msg: &Message,
    text: &str,
    allowed_pings: &[u64],
) -> Option<MessageId> {
    let mentions = build_allowed_mentions(allowed_pings);
    let chunks = split_text(text, MAX_MESSAGE_LENGTH);
    let mut first_id = None;
    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 {
            let sent = if !allowed_pings.is_empty() {
                reply_with_mentions(ctx, msg, chunk, allowed_pings).await
            } else {
                reply_no_ping(ctx, msg, chunk).await
            };
            first_id = match sent {
                Ok(sent) => Some(sent.id),
                Err(error) => {
                    tracing::warn!(
                        target: "housebot::message_flow",
                        message_id = msg.id.get(),
                        %error,
                        "Failed to send reply"
                    );
                    None
                }
            };
        } else if let Err(error) = msg
            .channel_id
            .send_message(
                &ctx.http,
                CreateMessage::new()
                    .content(chunk)
                    .allowed_mentions(mentions.clone()),
            )
            .await
        {
            tracing::warn!(
                target: "housebot::message_flow",
                message_id = msg.id.get(),
                chunk = i,
                %error,
                "Failed to send reply"
            );
        }
    }
    first_id
}
