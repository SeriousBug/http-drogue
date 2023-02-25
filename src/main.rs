mod avg_range;
mod download_actor;
mod store;

use std::{env, process::exit};

use actix_web::{
    get,
    middleware::DefaultHeaders,
    post,
    web::{Data, Form},
    App, HttpResponse, HttpServer, Responder,
};
use actix_web_httpauth::{
    extractors::{basic, AuthenticationError},
    middleware::HttpAuthentication,
};
use actix_web_rust_embed_responder::IntoResponse;
use askama::Template;
use cuttlestore::Cuttlestore;
use download_actor::{url_to_filename, Coordinator};
use futures::StreamExt;
use lazy_static::lazy_static;
use ractor::{cast, Actor, ActorRef};
use rust_embed_for_web::RustEmbed;
use scrypt::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, Salt},
    Params, Scrypt,
};
use serde::Deserialize;
use store::Progress;
use tracing::{debug, error};
use tracing_subscriber::{
    fmt::format::FmtSpan, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt,
};

use crate::{
    download_actor::{CoordinatorMsg, StartDownload},
    store::DownloadProgressStore,
};

#[derive(Template)]
#[template(path = "index.html")]

struct HomeTemplate;

#[derive(Template)]
#[template(path = "download_progress.html")]

struct DownloadListTemplate {
    files: Vec<ProgressDisplay>,
}

/// A version of `Progress` that is suitable for display in a template.
///
/// In particular, we calculate some things here so we don't have to do it in
/// the template code. Since we don't have IDE support for templates, it's
/// easier to do it this way.
#[derive(Debug)]
struct ProgressDisplay {
    pub failed: bool,
    pub url: String,
    pub name: String,
    pub percent: Option<String>,
    pub progress: String,
    pub total: Option<String>,
    pub speed: String,
    pub time_estimate: Option<String>,
}

fn human_speed(speed: f64) -> String {
    if speed < 1024f64 {
        format!("{:.2} B/s", speed)
    } else if speed < 1024f64 * 1024f64 {
        format!("{:.2} KiB/s", speed / 1024f64)
    } else if speed < 1024f64 * 1024f64 * 1024f64 {
        format!("{:.2} MiB/s", speed / 1024f64 / 1024f64)
    } else {
        format!("{:.2} GiB/s", speed / 1024f64 / 1024f64 / 1024f64)
    }
}

fn human_bytes(bytes: u64) -> String {
    if bytes < 1024u64 {
        format!("{} B", bytes)
    } else if bytes < 1024u64 * 1024u64 {
        format!("{:.2} KiB", bytes as f64 / 1024f64)
    } else if bytes < 1024u64 * 1024u64 * 1024u64 {
        format!("{:.2} MiB", bytes as f64 / 1024f64 / 1024f64)
    } else {
        format!("{:.2} GiB", bytes as f64 / 1024f64 / 1024f64 / 1024f64)
    }
}

fn human_time(seconds: f64) -> String {
    let minutes = seconds / 60.0;
    let hours = minutes / 60.0;
    let days = hours / 24.0;
    if seconds < 60.0 {
        format!("{:.2} seconds", seconds)
    } else if minutes < 60.0 {
        format!("{:.2} minutes", minutes)
    } else if hours < 24.0 {
        format!("{:.2} hours", hours)
    } else {
        format!("{:.2} days", days)
    }
}

impl From<Progress> for ProgressDisplay {
    #[tracing::instrument(level = "debug")]
    fn from(value: Progress) -> Self {
        ProgressDisplay {
            failed: value.failed,
            name: url_to_filename(&value.url),
            url: value.url,
            percent: value
                .total
                .map(|total| format!("{:.2}", value.progress as f64 / total as f64 * 100f64)),
            progress: human_bytes(value.progress),
            total: value.total.map(|total| human_bytes(total)),
            speed: human_speed(value.speed),
            time_estimate: value
                .total
                .map(|total| human_time((total - value.progress) as f64 / value.speed)),
        }
    }
}

#[get("/")]
#[tracing::instrument(level = "debug")]
async fn home() -> impl Responder {
    let response = HomeTemplate.render().unwrap();
    HttpResponse::Ok().content_type("text/html").body(response)
}

#[get("/list")]
#[tracing::instrument(level = "debug")]
async fn list(store: Data<DownloadProgressStore>) -> impl Responder {
    let files = store.scan().await.unwrap();
    let files = files.map(|x| x.unwrap().1);
    let files = files.map(|x| x.into());
    let files = files.collect::<Vec<_>>().await;

    let response = DownloadListTemplate { files }.render().unwrap();
    HttpResponse::Ok().content_type("text/html").body(response)
}

#[derive(Debug, Deserialize)]
struct DownloadRequest {
    url: String,
    restarting: Option<bool>,
}

#[post("/request_download")]
#[tracing::instrument(level = "info", skip(store, coordinator))]
async fn request_download(
    request: Form<DownloadRequest>,
    store: Data<DownloadProgressStore>,
    coordinator: Data<ActorRef<Coordinator>>,
) -> impl Responder {
    debug!("Requesting download of {}", request.url);
    store
        .put(
            request.url.clone(),
            &Progress {
                target_file: None,
                failed: false,
                url: request.url.clone(),
                progress: 0,
                total: None,
                speed: 0f64,
            },
        )
        .await
        .unwrap();
    let msg = CoordinatorMsg::StartDownload(StartDownload {
        url: request.url.clone(),
    });
    cast!(coordinator, msg).unwrap();
    HttpResponse::SeeOther()
        .insert_header((
            "Location",
            // Restart requests come from the list page iframe, so we need to redirect to the list page.
            if request.restarting.unwrap_or(false) {
                "/list"
            } else {
                "/"
            },
        ))
        .finish()
}

#[derive(RustEmbed)]
#[folder = "dist/"]
struct Dist;

#[get("/output.css")]
#[tracing::instrument(level = "debug")]
async fn serve_css() -> impl Responder {
    Dist::get("output.css").into_response()
}

lazy_static! {
    static ref PASS_HASH: Option<PasswordHash<'static>> = {
        env::var("HTTP_DROGUE_PASSWORD").ok().and_then(|password| {
            // Yes, these parameters are deliberately weak. Because we don't
            // actually store the passwords, we're not concerned with the
            // strength of the hash, it is more of a crude way to throttle
            // authentication attempts.
            Scrypt
                .hash_password_customized(
                    password.as_bytes(),
                    None,
                    None,
                    Params::new(1, 8, 1).unwrap(),
                    Salt::new("salt").unwrap(),
                )
                .ok()
        })
    };
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // We'll store the download progress in an sqlite database.
    // This way we can resume after a restart.
    let store: DownloadProgressStore = Cuttlestore::new(
        env::var("STORE_PATH").unwrap_or_else(|_| "sqlite:///data/http-drogue.sqlite".to_string()),
    )
    .await
    .unwrap();

    // The download coordinator will handle concurrently downloading files.
    let coordinator = Coordinator {
        concurrent_downloads: 1,
        store: store.clone(),
    };
    let (actor, _) = Actor::spawn(Some("coordinator".to_string()), coordinator, ())
        .await
        .unwrap();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_span_events(FmtSpan::NEW | FmtSpan::CLOSE))
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    if PASS_HASH.is_none() {
        error!("No password set, please set the HTTP_DROGUE_PASSWORD environment variable.");
        exit(1);
    }

    HttpServer::new(move || {
        let auth = HttpAuthentication::basic(|req, credentials| async move {
            let password = credentials.password().unwrap_or("");
            let Some(hash) = &*PASS_HASH else {
                panic!("No password set, please set the HTTP_DROGUE_PASSWORD environment variable.");
            };

            if Scrypt
                .verify_password(password.as_bytes(), hash)
                .is_ok()
            {
                Ok(req)
            } else {
                let config = req
                    .app_data::<basic::Config>()
                    .cloned()
                    .unwrap_or_default()
                    .realm("Http Drogue");
                Err((AuthenticationError::from(config).into(), req))
            }
        });
        App::new()
            .wrap(
                DefaultHeaders::new()
                    // Block embedding in iframes, except for our own which we use for the download progress page.
                    .add(("X-Frame-Options", "SAMEORIGIN"))
                    .add((
                        "Content-Security-Policy",
                        "script-src 'unsafe-inline'; default-src 'self'",
                    )),
            )
            .wrap(auth)
            .app_data(Data::new(store.clone()))
            .app_data(Data::new(actor.clone()))
            .service(home)
            .service(request_download)
            .service(list)
            .service(serve_css)
    })
    .bind(("127.0.0.1", 8080))?
    .workers(1)
    .run()
    .await
}
