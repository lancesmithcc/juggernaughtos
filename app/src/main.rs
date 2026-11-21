//! `juggernaught` — one binary: web server, MCP tool bridge, maintenance CLI.

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use clap::{Parser, Subcommand};
    use juggernaught::{app::*, server::*};
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use std::net::SocketAddr;

    #[derive(Parser)]
    #[command(name = "juggernaught", version, about = "Juggernaught OS — all-in-one AI business system")]
    struct Cli {
        /// Path to juggernaught.toml (defaults to dev mode in ./.jugg-dev when absent).
        #[arg(long, env = "JUGG_CONFIG")]
        config: Option<std::path::PathBuf>,
        #[command(subcommand)]
        cmd: Option<Cmd>,
    }

    #[derive(Subcommand)]
    enum Cmd {
        /// Run the admin app and public site (default).
        Serve,
        /// Serve the tool registry over MCP stdio (launched by Claude Code / Codex).
        Mcp {
            #[arg(long)]
            conversation: Option<String>,
        },
        /// Print health of crypto, database, provider and tools.
        Doctor,
    }

    let cli = Cli::parse();
    let cfg = match &cli.config {
        Some(p) => jugg_core::Config::load(p).unwrap_or_else(|e| panic!("config: {e}")),
        None => jugg_core::Config::dev(),
    };

    let log_to_stderr = matches!(cli.cmd, Some(Cmd::Mcp { .. }));
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into());
    if log_to_stderr {
        tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let conf = get_configuration(None).expect("leptos configuration");
    let mut leptos_options = conf.leptos_options;
    if cfg.dev {
        leptos_options.site_addr = cfg.listen_http.parse().expect("listen address");
    }
    let state = AppState::init(cfg, leptos_options.clone()).await.unwrap_or_else(|e| panic!("startup: {e}"));

    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Mcp { conversation } => {
            let brand = state.brand.read().await.clone();
            let ctx = jugg_tools::ToolContext {
                job_dir: state.cfg.data_dir.join("jobs").join(conversation.unwrap_or_else(|| "mcp".into())),
                brand: std::sync::Arc::new(brand),
                approved: false,
            };
            jugg_tools::mcp::serve_stdio(state.registry.clone(), ctx).await.expect("mcp stdio");
        }
        Cmd::Doctor => {
            println!("database     ok  {}", state.cfg.db_path().display());
            println!("keyring      ok  KEK #{}", state.ring.active_id());
            println!("signing key  ok  ML-DSA-65 ({} byte public key)", state.signer.public_key().len());
            println!("tls groups   {}", tls_provider_groups().join(", "));
            println!("provider     {} ({})", state.provider.label(), state.provider.health().await.map(|s| s).unwrap_or_else(|e| e.to_string()));
            for t in state.registry.specs() {
                println!("tool         {:<14} {:?}", t.name, t.risk);
            }
            match jugg_db::audit::verify_chain(&state.pool).await {
                Ok(None) => println!("audit chain  ok"),
                Ok(Some(seq)) => println!("audit chain  BROKEN at seq {seq}"),
                Err(e) => println!("audit chain  error {e}"),
            }
        }
        Cmd::Serve => {
            let addr = leptos_options.site_addr;
            let routes = generate_route_list(App);
            let app = Router::new()
                .route("/brand.css", axum::routing::get(brand_css))
                .leptos_routes(&state, routes, {
                    let opts = leptos_options.clone();
                    move || shell(opts.clone())
                })
                .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
                .layer(axum::middleware::from_fn_with_state(state.clone(), security_layer))
                .layer(tower_http::compression::CompressionLayer::new())
                .with_state(state);
            tracing::info!("Juggernaught listening on http://{addr}");
            let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("server");
        }
    }
}

#[cfg(not(feature = "ssr"))]
pub fn main() {}
