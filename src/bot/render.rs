//! Final-message delivery and pagination rendering.

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
/// reply message when one was sent, so callers can attach emoji reactions.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_final_message(
    ctx: &Context,
    msg: &Message,
    text: &str,
    paginate: bool,
    suppress_embeds: bool,
    owner_id: u64,
    store: &Mutex<HashMap<String, PaginatedResponse>>,
    progress: Option<&Message>,
    allowed_pings: &[u64],
) -> Option<MessageId> {
    let mentions = build_allowed_mentions(allowed_pings);
    // When embeds are suppressed (server override), fall back to plain text
    // even if pagination was requested.
    let use_pagination = paginate && !suppress_embeds;
    if !use_pagination {
        if paginate && suppress_embeds {
            if let Some(progress) = progress {
                let _ = progress.delete(&ctx.http).await;
            }
        }
        let chunks = split_text(text, MAX_MESSAGE_LENGTH);
        let mut first_id = None;
        for (i, chunk) in chunks.iter().enumerate() {
            if i == 0 {
                let sent = if !allowed_pings.is_empty() {
                    reply_with_mentions_and_suppress(
                        ctx,
                        msg,
                        chunk,
                        allowed_pings,
                        suppress_embeds,
                    )
                    .await
                } else {
                    reply_no_ping_with_suppress(ctx, msg, chunk, suppress_embeds).await
                };
                first_id = sent.ok().map(|m| m.id);
            } else {
                let mut msg_builder = CreateMessage::new()
                    .content(chunk)
                    .allowed_mentions(mentions.clone());
                if suppress_embeds {
                    msg_builder = msg_builder.flags(MessageFlags::SUPPRESS_EMBEDS);
                }
                let _ = msg.channel_id.send_message(&ctx.http, msg_builder).await;
            }
        }
        return first_id;
    }

    if let Some(progress) = progress {
        let _ = progress.delete(&ctx.http).await;
    }
    let pages = split_text(text, EMBED_DESCRIPTION_LIMIT);
    let token = uuid::Uuid::new_v4().simple().to_string();
    store.lock().await.insert(
        token.clone(),
        PaginatedResponse {
            owner_id,
            pages: pages.clone(),
        },
    );
    let builder = CreateMessage::new()
        .embed(pagination_embed(&pages, 0))
        .components(pagination_components(&token, 0, pages.len()))
        .reference_message(msg)
        .allowed_mentions(mentions);
    let sent = msg.channel_id.send_message(&ctx.http, builder).await;
    sent.ok().map(|m| m.id)
}

pub(crate) fn pagination_embed(pages: &[String], page: usize) -> CreateEmbed {
    CreateEmbed::new()
        .description(&pages[page])
        .footer(serenity::all::CreateEmbedFooter::new(format!(
            "Page {} of {}",
            page + 1,
            pages.len()
        )))
}

pub(crate) fn pagination_components(
    token: &str,
    page: usize,
    page_count: usize,
) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!(
            "{PAGINATION_PREFIX}{token}:{}",
            page.saturating_sub(1)
        ))
        .label("←")
        .style(ButtonStyle::Secondary)
        .disabled(page == 0),
        CreateButton::new(format!("{PAGINATION_PREFIX}{token}:{}", page + 1))
            .label("→")
            .style(ButtonStyle::Secondary)
            .disabled(page + 1 >= page_count),
    ])]
}
