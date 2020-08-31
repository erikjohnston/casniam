#![allow(clippy::type_complexity)]

use std::io::{BufRead, BufReader};
use std::{fs::File, str::FromStr};

use indicatif::{ProgressBar, ProgressStyle};
use opentelemetry::{api::Provider, sdk};
use serde::Deserialize;
use sodiumoxide::crypto::sign;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

use casniam::protocol::client::HyperFederationClient;
use casniam::protocol::{Event, Handler, RoomVersion, RoomVersion5};
use casniam::stores::postgres;

/// Takes a `str` room version and calls the expression with a type alias `R` set
/// to the appopriate type.
#[macro_export]
macro_rules! route_room_version {
    ($ver:expr, $f:expr) => {
        route_room_version!(
            $ver,
            $f,
            actix_web::error::ErrorInternalServerError("Unknown version",)
        )
    };
    ($ver:expr, $f:expr, $error:expr) => {
        match $ver as &str {
            RoomVersion3::VERSION => {
                type R = RoomVersion3;
                $f
            }
            RoomVersion4::VERSION => {
                type R = RoomVersion4;
                $f
            }
            RoomVersion5::VERSION => {
                type R = RoomVersion5;
                $f
            }
            _ => {
                error!("Unrecognized version {}", $ver);
                return Err($error);
            }
        }
    };
}

#[derive(Deserialize, Clone, Debug)]
struct Settings {
    server_name: String,
    key: Key,
    database: String,
    http_bind_addr: String,
}

#[derive(Deserialize, Clone, Debug)]
struct Key {
    id: String,
    seed: sign::Seed,
}

#[tokio::main()]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    // let _guard = slog_envlogger::init().unwrap();

    // We need to import this due to clap::app_from_crate!.
    use clap::{crate_authors, crate_description, crate_name, crate_version};

    let matches = clap::app_from_crate!()
        .arg(
            clap::Arg::with_name("config")
                .short("c")
                .long("config")
                .takes_value(true)
                .multiple(true)
                .help("Specify config files to load"),
        )
        .arg(
            clap::Arg::with_name("file")
                .short("f")
                .takes_value(true)
                .long("file")
                .help("Specify file to load"),
        )
        .get_matches();

    let config_files = matches.values_of("config").unwrap_or_default();

    let event_file = matches.value_of("file").unwrap_or_default();

    let mut settings = config::Config::new();
    for config_file in config_files {
        settings
            .merge(config::File::with_name(config_file))
            .unwrap();
    }

    let settings: Settings = settings.try_into().expect("invalid config");

    let server_name = settings.server_name.clone();

    let (_, secret_key) = sign::keypair_from_seed(&settings.key.seed);
    let key_id = format!("ed25519:{}", settings.key.id);

    let config =
        tokio_postgres::config::Config::from_str(&settings.database).unwrap();
    let pg_mgr = bb8_postgres::PostgresConnectionManager::new(
        config,
        tokio_postgres::NoTls,
    );

    // Create a new tracer
    let exporter = opentelemetry_jaeger::Exporter::builder()
        .with_agent_endpoint("127.0.0.1:6831".parse().unwrap())
        .with_process(opentelemetry_jaeger::Process {
            service_name: "casniam_import_room".to_string(),
            tags: Vec::new(),
        })
        .init()
        .expect("Error initializing Jaeger exporter");

    let provider = sdk::Provider::builder()
        .with_simple_exporter(exporter)
        .with_config(sdk::Config {
            default_sampler: Box::new(sdk::Sampler::AlwaysOn),
            ..Default::default()
        })
        .build();

    let tracer = provider.get_tracer("casniam_import_room");

    // Create a new OpenTelemetry tracing layer
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    let subscriber = Registry::default().with(telemetry);
    tracing::subscriber::set_global_default(subscriber)
        .expect("global tracer init");

    let pool = match bb8::Pool::builder()
        .min_idle(Some(1))
        .max_size(10)
        .build(pg_mgr)
        .await
    {
        Ok(pool) => pool,
        Err(e) => panic!("builder error: {:?}", e),
    };
    let stores = postgres::PostgresEventStore::new(pool);

    // let stores = casniam::stores::memory::MemoryStoreFactory::new();

    let client = HyperFederationClient::new(
        (),
        server_name.clone(),
        key_id.clone(),
        secret_key.clone(),
    );

    let handler = Handler::new(stores.clone(), client);

    let file = File::open(event_file)?;
    let cursor = BufReader::new(file);

    let bar = ProgressBar::new(26745);
    bar.set_style(ProgressStyle::default_bar().template(
        "[{elapsed_precise}] {bar:40} {percent}% ({pos:>7}/{len:7}) - {per_sec} Hz",
    ));

    for line_res in cursor.lines() {
        let line: String = line_res?;
        let line = unescape::unescape(&line).unwrap();

        // println!("Handling: {}", line);

        let event: <RoomVersion5 as RoomVersion>::Event =
            serde_json::from_str(&line)?;

        let room_id = event.room_id().to_string();

        handler
            .handle_new_timeline_events::<RoomVersion5>(
                "origin",
                &room_id,
                vec![event],
            )
            .await
            .unwrap();

        bar.inc(1);
    }

    bar.finish();

    Ok(())
}
