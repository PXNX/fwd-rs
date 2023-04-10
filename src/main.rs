use std::net::SocketAddr;
use std::{env, sync::Arc};

use axum::extract::{ConnectInfo, Path};
use axum::http::StatusCode;
use axum::response::Redirect;
use axum::routing::get;
use axum::{async_trait, Extension};
use dotenv::dotenv;
use models::Link;
use reqwest::Url;

use sqlx::{query, query_as, Executor, PgPool};
use teloxide::adaptors::DefaultParseMode;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::types::ParseMode;
use teloxide::{
    dispatching::update_listeners::webhooks, error_handlers::IgnoringErrorHandlerSafe, prelude::*,
};

mod models;

#[derive(Clone, Default)]
pub enum LinkDialogueState {
    #[default]
    StartLink,
    ReceiveTarget,
    ReceiveTitle {
        target: String,
    },
}

pub struct CustomService {
    pool: PgPool,
}

#[async_trait]
impl shuttle_service::Service for CustomService {
    async fn bind(
        mut self,
        addr: std::net::SocketAddr,
    ) -> Result<(), shuttle_service::error::Error> {
        //  let (_, _) = tokio::join!(self.router, self.bot);

        self.pool
            .execute(include_str!("../sql/schema.sql"))
            .await
            .map_err(shuttle_service::error::CustomError::new)?;

        let bot: DefaultParseMode<Bot> = Bot::from_env().parse_mode(ParseMode::Html);
        let token = bot.inner().token();
        let host = env::var("HOST").expect("HOST env variable is not set");
        let url = Url::parse(&format!("https://{host}/webhooks/{token}")).unwrap();

        /*    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&*env::var("PgPool_URL").expect("PgPool_URL must be provided!"))
        .await
        .unwrap();*/
        let b = bot.clone();
        let p = self.pool.clone();

        let listener = webhooks::axum_to_router(bot.clone(), webhooks::Options::new(addr, url))
            .await
            .expect("Couldn't setup webhook");

        let handler = dptree::entry().branch(
            Update::filter_message()
                .enter_dialogue::<Message, InMemStorage<LinkDialogueState>, LinkDialogueState>()
                .branch(dptree::case![LinkDialogueState::StartLink].endpoint(start_link))
                .branch(dptree::case![LinkDialogueState::ReceiveTarget].endpoint(receive_target))
                .branch(
                    dptree::case![LinkDialogueState::ReceiveTitle { target }]
                        .endpoint(receive_title),
                ),
        );

        let mut dp = Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![
                self.pool,
                InMemStorage::<LinkDialogueState>::new()
            ])
            .build();

        let router = listener
            .2
            .route("/:link_id/:title", get(redirect_link))
            .layer(Extension(b))
            .layer(Extension(p));

        let server = axum::Server::bind(&addr)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>());

        let bot = dp.dispatch_with_listener(listener.0, Arc::new(IgnoringErrorHandlerSafe));

        tokio::select!(
        _ = server=>{},
           _ = bot=>{}
        );

        Ok(())
    }
}

#[shuttle_runtime::main]
async fn init(
    #[shuttle_shared_db::Postgres] pool: PgPool,
) -> Result<CustomService, shuttle_service::Error> {
    dotenv().ok();
    /*    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{l} - {m}\n")))
        .build("log/output.log")
        .unwrap();

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(Root::builder().appender("logfile").build(LevelFilter::Info))
        .unwrap();

    log4rs::init_config(config).unwrap();*/

    log::info!("Starting fwd...");

    Ok(CustomService { pool: pool })
}

type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
type LinkDialogue = Dialogue<LinkDialogueState, InMemStorage<LinkDialogueState>>;

async fn start_link(
    bot: DefaultParseMode<Bot>,
    dialogue: LinkDialogue,
    msg: Message,
) -> HandlerResult {
    match msg.text() {
        Some(text) => {
            if text == "/shorten" {
                bot.send_message(msg.chat.id, "Please send me the Url you want to shorten.")
                    .await?;
                dialogue.update(LinkDialogueState::ReceiveTarget).await?;
            }
        }
        None => {
            bot.send_message(msg.chat.id, "Send me plain text.").await?;
        }
    }

    Ok(())
}

async fn receive_target(
    bot: DefaultParseMode<Bot>,
    dialogue: LinkDialogue,
    msg: Message,
) -> HandlerResult {
    match msg.text() {
        Some(text) => {
            bot.send_message(
                msg.chat.id,
                "Please send me the title that should display in the link.",
            )
            .await?;
            dialogue
                .update(LinkDialogueState::ReceiveTitle {
                    target: text.into(),
                })
                .await?;
        }
        None => {
            bot.send_message(msg.chat.id, "Send me plain text.").await?;
        }
    }

    Ok(())
}

async fn receive_title(
    bot: DefaultParseMode<Bot>,
    dialogue: LinkDialogue,
    target: String,
    msg: Message,
    pool: PgPool,
) -> HandlerResult {
    match msg.text() {
        Some(text) => {
            let shortened_link: Link = query_as!(
                Link,
                r#"insert into links(author,target,title) values($1,$2,$3) returning *;"#,
                msg.chat.id.0,
                target,
                text
            )
            .fetch_one(&pool)
            .await?;
            let host = env::var("HOST").expect("HOST env variable is not set");
            bot.send_message(
                msg.chat.id,
                format!(
                    "Here's your shortened link:\n\n<code>https://{}/{}/{}</code>",
                    host, shortened_link.id, shortened_link.title
                ),
            )
            .await?;
            dialogue.exit().await?;
        }
        None => {
            bot.send_message(msg.chat.id, "Send me plain text.").await?;
        }
    }

    Ok(())
}

async fn redirect_link(
    Extension(bot): Extension<DefaultParseMode<Bot>>,
    Extension(pool): Extension<PgPool>,
    Path((link_id, _title)): Path<(i32, String)>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Redirect, StatusCode> {
    let mut stored_url: Link = query_as!(Link, r#"select * from links where id = $1;"#, link_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| match err {
            sqlx::Error::RowNotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    let parsed_addr: String = addr.to_string();

    query!(
        r#"insert into accesses(link_id, address) values ($1,$2);"#,
        link_id,
        parsed_addr
    )
    .execute(&pool)
    .await
    .map_err(|err| match err {
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    })?;

    bot.send_message(
        ChatId(stored_url.author),
        format!(
            "New Access on: {}\n\nLinking to: {}\n\nBy address: {}",
            stored_url.title, stored_url.target, parsed_addr
        ),
    )
    .await
    .map_err(|err| match err {
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    })?;

    if !stored_url.target.contains("http") {
        stored_url.target = format!("https://{}", stored_url.target);
    }

    Ok(Redirect::to(&stored_url.target))
}
