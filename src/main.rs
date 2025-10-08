use regex::Regex;
use serenity::all::{EditMessage, MessageBuilder};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

use std::env;
use std::sync::LazyLock;

#[cfg(debug_assertions)]
use dotenvy::dotenv;

struct Handler;
async fn handle_message(msg: &Message) -> Vec<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(https?:\/\/)(www\.)?([-a-zA-Z0-9@:%._\+~#=]{1,256}\.)*([-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6})\b([-a-zA-Z0-9()@:%_\+.~#?&\/=]*)").unwrap()
    });

    let mut new_embeds: Vec<String> = Vec::new();
    for capture in RE.captures_iter(msg.content.as_str()) {
        if let Some(domain) = capture.get(4)
            && let domain = domain.as_str()
            && (domain == "x.com" || domain == "twitter.com")
        {
            new_embeds.push(
                capture
                    .iter()
                    .skip(1)
                    .flatten()
                    .map(|c| {
                        let c = c.as_str();
                        if c == "x.com" || c == "twitter.com" {
                            "fxtwitter.com"
                        } else {
                            c
                        }
                    })
                    .collect(),
            );
        }
    }

    new_embeds
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, mut msg: Message) {
        if msg.author.bot {
            return;
        }

        let new_embeds = handle_message(&msg).await;
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

    async fn ready(&self, _: serenity::Context, ready: Ready) {
        println!("{} is connected! {:?}", ready.user.name, ready.guilds);
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
