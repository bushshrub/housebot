//! serenity `EventHandler` wiring.
//!
//! Each gateway event delegates to an inherent method so the handlers can live
//! next to the state they touch rather than in one trait impl.

use super::*;

#[serenity::async_trait]
impl EventHandler for HouseBot {
    async fn ready(&self, ctx: Context, ready: Ready) {
        self.on_ready(ctx, ready).await;
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        self.on_interaction(ctx, interaction).await;
    }

    async fn message(&self, ctx: Context, msg: Message) {
        self.on_message(ctx, msg).await;
    }

    async fn reaction_add(&self, ctx: Context, reaction: serenity::all::Reaction) {
        self.on_reaction_add(ctx, reaction).await;
    }
}
