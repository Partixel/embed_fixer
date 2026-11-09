use poise::serenity_prelude::{self as serenity, *};
use std::env;

mod music;

const SUBDOMAINS: &[&str] = &["www.", "m.", "mobile.", "old.", "np."];

type Error = Box<serenity::Error>;

/// Every http(s) link in `text`, skipping `<links>` the user already suppressed.
fn links(text: &str) -> Vec<reqwest::Url> {
    text.split_whitespace()
        .filter_map(|word| {
            let start = word.find("http")?;
            if word[..start].ends_with('<') {
                return None;
            }
            let url = word[start..].trim_end_matches(['.', ',', '!', '?', ';', ':', ')', ']', '>']);
            reqwest::Url::parse(url)
                .ok()
                .filter(|url| matches!(url.scheme(), "http" | "https"))
        })
        .collect()
}

/// A link to a site with a better embedder, pointed at that embedder.
fn rewrite(url: &reqwest::Url) -> Option<String> {
    let host = url.host_str()?;
    let host = SUBDOMAINS
        .iter()
        .find_map(|s| host.strip_prefix(s))
        .unwrap_or(host);
    let embedder = match host {
        "x.com" | "twitter.com" => "fxtwitter.com",
        "reddit.com" | "redd.it" => "redditez.com",
        "instagram.com" => "kkinstagram.com",
        _ => return None,
    };
    let mut fixed = url.clone();
    fixed.set_scheme("https").ok()?;
    fixed.set_host(Some(embedder)).ok()?;
    fixed.set_port(None).ok()?;
    Some(fixed.into())
}

/// Everything we replace Discord's own (flaky) embeds with: the rewritten links, then music cards.
async fn fixes(text: &str) -> Vec<poise::CreateReply> {
    let urls = links(text);
    let rewritten = urls
        .iter()
        .filter_map(rewrite)
        .collect::<Vec<_>>()
        .join("\n");
    (!rewritten.is_empty())
        .then(|| poise::CreateReply::default().content(rewritten))
        .into_iter()
        .chain(music::cards(&urls).await)
        .collect()
}

async fn on_event(
    ctx: poise::FrameworkContext<'_, (), Error>,
    event: &serenity::FullEvent,
) -> Result<(), Error> {
    if let serenity::FullEvent::Message { new_message: msg } = event
        && !msg.author.bot
    {
        let http = &ctx.serenity_context.http;
        let mut sent = false;
        for reply in fixes(&msg.content).await {
            let reply = reply
                .reply(true)
                .allowed_mentions(CreateAllowedMentions::new());
            let message = reply.to_prefix(msg.into());
            let result = msg.channel_id.send_message(http, message).await;
            sent |= result
                .inspect_err(|e| eprintln!("embed_fixer couldn't send a reply: {e}"))
                .is_ok();
        }
        // Last, so if every reply failed the original embed is still there.
        if sent {
            let hide = EditMessage::new().suppress_embeds(true);
            let result = msg.channel_id.edit_message(http, msg.id, hide).await;
            result
                .inspect_err(|e| eprintln!("embed_fixer couldn't hide the original embed: {e}"))
                .ok();
        }
    }
    Ok(())
}

#[poise::command(
    context_menu_command = "Fix embed",
    install_context = "Guild|User",
    interaction_context = "Guild|BotDm|PrivateChannel"
)]
async fn fixembed(ctx: poise::Context<'_, (), Error>, msg: Message) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let replies = fixes(&msg.content).await;
    if replies.is_empty() {
        ctx.say("Can't find a supported link").await?;
    }
    for reply in replies {
        ctx.send(reply).await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    #[cfg(debug_assertions)]
    dotenvy::dotenv().ok();

    music::init();

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![fixembed()],
            event_handler: |ctx, event| Box::pin(on_event(ctx, event)),
            ..Default::default()
        })
        .setup(|ctx, ready, framework| {
            Box::pin(async move {
                println!("{} is connected!", ready.user.name);
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(())
            })
        })
        .build();

    let token = env::var("DISCORD_TOKEN").expect("Expected a DISCORD_TOKEN in the environment");
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    Client::builder(&token, intents)
        .framework(framework)
        .status(OnlineStatus::Invisible)
        .await
        .expect("Err creating client")
        .start()
        .await
        .expect("Client error");
}

#[cfg(test)]
mod tests {
    use super::{links, rewrite};

    fn rewritten(text: &str) -> Vec<String> {
        links(text).iter().filter_map(rewrite).collect()
    }

    #[test]
    fn rewrites() {
        assert_eq!(
            rewritten("https://www.x.com/a/status/1?s=2 and https://old.reddit.com/r/x/"),
            [
                "https://fxtwitter.com/a/status/1?s=2",
                "https://redditez.com/r/x/"
            ]
        );
        assert_eq!(rewritten("see https://x.com."), ["https://fxtwitter.com/"]);
        for ignored in [
            "<https://x.com/a>",
            "https://example.com",
            "https://open.spotify.com/track/1",
        ] {
            assert!(rewritten(ignored).is_empty(), "{ignored}");
        }
        assert_eq!(
            links("listen https://open.spotify.com/track/1?si=a").len(),
            1
        );
    }
}
