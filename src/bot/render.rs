//! Final-message delivery and pagination rendering.

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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_final_message(
    ctx: &Context,
    msg: &Message,
    text: &str,
    paginate: bool,
    owner_id: u64,
    store: &Mutex<HashMap<String, PaginatedResponse>>,
    progress: Option<&Message>,
    allowed_pings: &[u64],
) {
    let mentions = build_allowed_mentions(allowed_pings);
    if !paginate {
        let chunks = split_text(text, MAX_MESSAGE_LENGTH);
        if let (Some(progress), Some(first)) = (progress, chunks.first()) {
            if progress
                .channel_id
                .edit_message(
                    &ctx.http,
                    progress.id,
                    EditMessage::new()
                        .content(first)
                        .allowed_mentions(mentions.clone()),
                )
                .await
                .is_ok()
            {
                for chunk in chunks.iter().skip(1) {
                    let _ = msg
                        .channel_id
                        .send_message(
                            &ctx.http,
                            CreateMessage::new()
                                .content(chunk)
                                .allowed_mentions(mentions.clone()),
                        )
                        .await;
                }
                return;
            }
        }
        for (i, chunk) in chunks.iter().enumerate() {
            if i == 0 {
                if !allowed_pings.is_empty() {
                    let _ = reply_with_mentions(ctx, msg, chunk, allowed_pings).await;
                } else {
                    let _ = reply_no_ping(ctx, msg, chunk).await;
                }
            } else {
                let _ = msg
                    .channel_id
                    .send_message(
                        &ctx.http,
                        CreateMessage::new()
                            .content(chunk)
                            .allowed_mentions(mentions.clone()),
                    )
                    .await;
            }
        }
        return;
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
    let _ = msg.channel_id.send_message(&ctx.http, builder).await;
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
