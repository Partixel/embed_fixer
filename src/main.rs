#[cfg(debug_assertions)]
use dotenvy::dotenv;

use regex::Regex;
use serenity::all::{EditMessage, MessageBuilder};
use std::env;
use std::sync::LazyLock;

use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, mut msg: Message) {
        if msg.author.bot {
            return;
        }

        static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"twitter\.com").unwrap());

        let mut new_embeds: Vec<String> = Vec::new();
        for embed in msg.embeds.clone() {
            if let Some(url) = embed.url
                && RE.is_match(url.as_str())
            {
                new_embeds.push(format!(
                    "[⠀]({})",
                    RE.replace(url.as_str(), "fxtwitter.com")
                ));
            }
        }

        if new_embeds.is_empty() {
            return;
        }

        let mut msg_builder = MessageBuilder::new();
        for embed in new_embeds {
            msg_builder.push(embed);
        }

        if let Err(error) = msg.reply(&ctx.http, msg_builder.build()).await {
            eprintln!("embed_fixer errored when replying:\n{}", error);
            return;
        };

        if let Err(error) = msg
            .edit(&ctx.http, EditMessage::new().suppress_embeds(true))
            .await
        {
            eprintln!(
                "embed_fixer errored when removing the original embed:\n{}",
                error
            );
            return;
        };
    }
}

#[tokio::main]
async fn main() {
    #[cfg(debug_assertions)]
    dotenv().ok();

    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler)
        .await
        .expect("Err creating client");

    if let Err(why) = client.start().await {
        eprintln!("Client error: {why:?}");
    }
}
