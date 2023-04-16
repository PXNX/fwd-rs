use axum::body::Body;
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Path};
use axum::headers::Header;
use axum::http::request::Parts;
use axum::http::{self, HeaderMap, StatusCode, Uri};
use axum::response::Redirect;
use axum::routing::get;
use axum::{async_trait, Extension};
use axum_client_ip::{
    InsecureClientIp, SecureClientIp, SecureClientIpSource, XForwardedFor, XRealIp,
};
use hyper::server::conn::AddrStream;
use log::{info, LevelFilter};
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Root};
use log4rs::encode::pattern::PatternEncoder;
use log4rs::Config;
use models::Link;
use reqwest::Url;

use axum::http::{header::FORWARDED, Extensions};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use shuttle_secrets::SecretStore;
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
    token: String,
    pool: PgPool,
    secret_store: SecretStore,
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
            .unwrap();

        let bot: DefaultParseMode<Bot> = Bot::new(&self.token).parse_mode(ParseMode::Html);
        let hostname = self
            .secret_store
            .get("HOSTNAME")
            .expect("No hostname provided");
        let url = Url::parse(&format!("{}/webhooks/{}", hostname, self.token)).unwrap();

        info!("done setting up bot. - add: {}", hostname);

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

        info!("done setting up listener.");

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

        info!("done setting up handler.");

        let mut dp = Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![
                self.pool,
                InMemStorage::<LinkDialogueState>::new(),
                self.secret_store
            ])
            .build();

        log::info!("done setting up dp.");

        let router = listener
            .2
            .route("/:link_id/:title", get(redirect_link))
            .layer(SecureClientIpSource::ConnectInfo.into_extension())
            .layer(Extension(b))
            .layer(Extension(p));

        let server = axum::Server::bind(&addr)
            .serve(router.into_make_service_with_connect_info::<MyConnectInfo>());

        //can get addr from router?

        let bot = dp.dispatch_with_listener(listener.0, Arc::new(IgnoringErrorHandlerSafe));

        log::info!("done setting up server.");

        tokio::select!(
        _ = server=>{},
           _ = bot=>{}
        );

        //  let (_, _) = tokio::join!(server, bot);

        Ok(())
    }
}

#[shuttle_runtime::main]
async fn init(
    #[shuttle_shared_db::Postgres(
        local_uri = "postgresql://postgres:area@localhost:5432/fwd"//"postgres://user-putins-fanclub:23BYuPj4xGKF@db.shuttle.rs:5432/db-putins-fanclub" 
    )]
    pool: PgPool,
    #[shuttle_secrets::Secrets] secret_store: SecretStore,
) -> Result<CustomService, shuttle_service::Error> {
    /*  let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{l} - {m}\n")))
        .build("log/output.log")
        .unwrap();

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(Root::builder().appender("logfile").build(LevelFilter::Info))
        .unwrap();

    log4rs::init_config(config).unwrap(); */

    let token = secret_store
        .get("TELOXIDE_TOKEN")
        .expect("No telegram token provided");

    info!("Starting fwd...");

    Ok(CustomService {
        token: token,
        pool: pool,
        secret_store: secret_store,
    })
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
    secret_store: SecretStore,
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

            let hostname = secret_store.get("HOSTNAME").expect("No hostname provided");
            log::info!("hostname: {hostname}");

            bot.send_message(
                msg.chat.id,
                format!(
                    "Here's your shortened link (tap to copy):\n\n<code>{}/{}/{}</code>",
                    hostname, shortened_link.id, shortened_link.title
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

#[derive(Clone, Debug)]
struct MyConnectInfo {
    remote: String,
}

impl Connected<&AddrStream> for MyConnectInfo {
    fn connect_info(target: &AddrStream) -> Self {
        let h = format!("{:?}", target);
        MyConnectInfo { remote: h }
    }
}

async fn redirect_link(
    Extension(bot): Extension<DefaultParseMode<Bot>>,
    Extension(pool): Extension<PgPool>,
    Path((link_id, _title)): Path<(i32, String)>,
    fwd: XForwardedFor,
    headers: HeaderMap,
) -> Result<Redirect, StatusCode> {
    let mut stored_url: Link = query_as!(Link, r#"select * from links where id = $1;"#, link_id)
        .fetch_one(&pool)
        .await
        .map_err(|err| match err {
            sqlx::Error::RowNotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    let parsed_addr2: String = format!("{:?}", fwd.0[0]);

    query!(
        r#"insert into accesses(link_id, address) values ($1,$2);"#,
        link_id,
        parsed_addr2
    )
    .execute(&pool)
    .await
    .map_err(|err| match err {
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    })?;

    let h = format!("{:?}", headers);

    bot.send_message(
        ChatId(stored_url.author),
        format!(
            "New Access on: {}\n\nLinking to: {}\n\nBy address:\nFWD: {}\n\nHeader: {}",
            stored_url.title, stored_url.target, parsed_addr2, h
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
