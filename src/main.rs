use regex::Regex;

use poise::{CreateReply, serenity_prelude as serenity};
use serenity::OnlineStatus;
use serenity::all::EditMessage;
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

use std::collections::HashMap;
use std::env;
use std::sync::LazyLock;

#[cfg(debug_assertions)]
use dotenvy::dotenv;

struct Handler;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;
pub struct Data {}

struct UrlReplacement {
    new_domain: &'static str,
    new_subdomain: Option<&'static str>,
    subdomain_to_replace: Option<&'static str>,
    remove_embed: bool,
}

async fn handle_message(msg: &Message) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(https?:\/\/)(www\.)?([-a-zA-Z0-9@:%._\+~#=]{1,256}\.)*([-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6})\b([-a-zA-Z0-9()@:%_\+.~#?&\/=]*)").unwrap()
    });

    static REPLACEMENTS: LazyLock<HashMap<&str, UrlReplacement>> = LazyLock::new(|| {
        HashMap::from([
            ("x.com", UrlReplacement { new_domain: "fxtwitter.com", new_subdomain: None, subdomain_to_replace: None, remove_embed: true }),
            ("twitter.com", UrlReplacement { new_domain: "fxtwitter.com", new_subdomain: None, subdomain_to_replace: None, remove_embed: true }),
            ("old.reddit.com", UrlReplacement { new_domain: "redditez.com", new_subdomain: Some(""), subdomain_to_replace: Some("old."), remove_embed: true }),
            ("reddit.com", UrlReplacement { new_domain: "redditez.com", new_subdomain: None, subdomain_to_replace: None, remove_embed: true }),
            ("redd.it", UrlReplacement { new_domain: "redditez.com", new_subdomain: None, subdomain_to_replace: None, remove_embed: true }),
            ("instagram.com", UrlReplacement { new_domain: "kkinstagram.com", new_subdomain: None, subdomain_to_replace: None, remove_embed: true }),
        ])
    });

    let mut new_embeds: Vec<String> = Vec::new();
    for capture in RE.captures_iter(msg.content.as_str()) {
        println!("{:?}", capture);
        if let Some(domain) = capture.get(4)
            && let domain = domain.as_str()
        {
            if let Some(subdomain) = capture.get(3) && let subdomain = subdomain.as_str() && let Some(repl) = REPLACEMENTS.get((subdomain.to_string() + domain).as_str()) {
               new_embeds.push(
                    capture
                        .iter()
                        .skip(1)
                        .flatten()
                        .map(|c| c.as_str())
                        .map(|c| {
                            if c == domain {
                                repl.new_domain
                            } else if let Some(subdomain) = repl.subdomain_to_replace && let Some(new_subdomain) = repl.new_subdomain && c == subdomain {
                                new_subdomain
                            } else {
                                c
                            }
                        })
                        .collect(),
                );
            } else if let Some(repl) = REPLACEMENTS.get(domain) {
                new_embeds.push(
                    capture
                        .iter()
                        .skip(1)
                        .flatten()
                        .map(|c| c.as_str())
                        .map(|c| {
                            if c == domain {
                                repl.new_domain
                            } else if let Some(subdomain) = repl.subdomain_to_replace && let Some(new_subdomain) = repl.new_subdomain && c == subdomain {
                                new_subdomain
                            } else {
                                c
                            }
                        })
                        .collect(),
                );
            }
        }
    }

    new_embeds.join("\n")
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: serenity::Context, mut msg: Message) {
        if msg.author.bot {
            return;
        }

        let new_embeds = handle_message(&msg).await;
        if new_embeds.is_empty() {
            return;
        }

        if let Err(error) = msg.reply(&ctx.http, new_embeds).await {
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

/// Echo content of a message
#[poise::command(
    context_menu_command = "Fix embed",
    install_context = "Guild|User",
    interaction_context = "Guild|BotDm|PrivateChannel"
)]
pub async fn fixembed(
    ctx: Context<'_>,
    #[description = "Message to fix (enter a link or ID)"] msg: serenity::Message,
) -> Result<(), Error> {
    let new_embeds = handle_message(&msg).await;
    if new_embeds.is_empty() {
        ctx.send(
            CreateReply::default()
                .content("Can't find a supported link")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    ctx.send(CreateReply::default().content(new_embeds).ephemeral(true))
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    #[cfg(debug_assertions)]
    dotenv().ok();

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![fixembed()],
            on_error: |error| {
                Box::pin(async move {
                    match error {
                        poise::FrameworkError::ArgumentParse { error, .. } => {
                            if let Some(error) = error.downcast_ref::<serenity::RoleParseError>() {
                                println!("Found a RoleParseError: {:?}", error);
                            } else {
                                println!("Not a RoleParseError :(");
                            }
                        }
                        other => poise::builtins::on_error(other).await.unwrap(),
                    }
                })
            },
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data {})
            })
        })
        .build();

    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&token, intents)
        .framework(framework)
        .event_handler(Handler)
        .status(OnlineStatus::Invisible)
        .await
        .expect("Err creating client");

    if let Err(why) = client.start().await {
        eprintln!("Client error: {why:?}");
    }
}
