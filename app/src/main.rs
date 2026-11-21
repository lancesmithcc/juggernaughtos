//! `juggernaught` — one binary: web server, MCP tool bridge, maintenance CLI.
#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::routing::{get, post};
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
        /// Connect an intelligence provider: claude, codex or openrouter.
        Connect { provider: String },
        /// Print the one-time setup link for a fresh install.
        SetupUrl,
        /// Print health of crypto, database, provider and tools.
        Doctor,
    }

    let cli = Cli::parse();
    let cfg = match &cli.config {
        Some(p) => jugg_core::Config::load(p).unwrap_or_else(|e| panic!("config: {e}")),
        None => jugg_core::Config::dev(),
    };
    if cli.config.is_none() {
        // Children (provider CLIs → `juggernaught mcp`) must resolve the same dev data dir.
        std::env::set_var("JUGG_DEV_ROOT", cfg.data_dir.parent().unwrap_or(&cfg.data_dir));
    }

    let quiet = matches!(cli.cmd, Some(Cmd::Mcp { .. }) | Some(Cmd::SetupUrl) | Some(Cmd::Doctor) | Some(Cmd::Connect { .. }));
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| if quiet { "warn,sqlx=warn".into() } else { "info,sqlx=warn".into() });
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();

    if let Some(Cmd::SetupUrl) = cli.cmd {
        let origin = admin_origin(&cfg);
        if cfg.dev {
            println!("{origin}/setup  (development mode: no token needed)");
        } else {
            match std::fs::read_to_string(cfg.keys_dir.join("setup-token")) {
                Ok(t) => println!("{origin}/setup?token={}", t.trim()),
                Err(_) => println!("No setup pending: this workspace already has an owner."),
            }
        }
        return;
    }

    let conf = get_configuration(None).expect("leptos configuration");
    let mut leptos_options = conf.leptos_options;
    if cfg.dev {
        leptos_options.site_addr = cfg.listen_http.parse().expect("listen address");
    }
    let state = AppState::init(cfg, leptos_options.clone()).await.unwrap_or_else(|e| panic!("startup: {e}"));

    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::SetupUrl => unreachable!(),
        Cmd::Mcp { conversation } => {
            let brand = state.brand.read().await.clone();
            let ctx = jugg_tools::ToolContext {
                job_dir: state.cfg.data_dir.join("jobs").join(conversation.unwrap_or_else(|| "mcp".into())),
                brand: std::sync::Arc::new(brand),
                approved: false,
            };
            jugg_tools::mcp::serve_stdio(state.registry.clone(), ctx).await.expect("mcp stdio");
        }
        Cmd::Connect { provider } => {
            let (kind, login): (&str, Option<(&str, Vec<&str>)>) = match provider.as_str() {
                "claude" => ("claude_code_cli", Some(("claude", vec![]))),
                "codex" => ("codex_cli", Some(("codex", vec!["login"]))),
                "openrouter" => ("openrouter", None),
                other => {
                    eprintln!("Unknown provider `{other}`. Use claude, codex or openrouter.");
                    std::process::exit(2);
                }
            };
            let mut key = None;
            match login {
                Some((bin, args)) => {
                    if bin == "claude" {
                        println!("Starting Claude Code. Type /login, finish signing in, then /exit.");
                    }
                    // The vendor's own login flow: Juggernaught never sees these credentials.
                    let status = std::process::Command::new(bin).args(&args).status();
                    if !status.map(|s| s.success()).unwrap_or(false) {
                        eprintln!("`{bin}` is not installed or login did not finish.");
                        std::process::exit(1);
                    }
                }
                None => {
                    eprint!("OpenRouter API key: ");
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).expect("stdin");
                    key = Some(line.trim().to_string());
                }
            }
            match save_provider(&state, kind, None, key).await {
                Ok(msg) => println!("{msg}"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Doctor => {
            let provider = state.provider().await;
            println!("database     ok  {}", state.cfg.db_path().display());
            println!("keyring      ok  KEK #{}", state.ring.active_id());
            println!("signing key  ok  ML-DSA-65 ({} byte public key)", state.signer.public_key().len());
            println!("tls groups   {}", tls_provider_groups().join(", "));
            println!("admin        {}", admin_origin(&state.cfg));
            println!("mail         {}", if state.mailer.is_some() { "SMTP configured" } else { "not configured (magic links only logged in dev)" });
            println!("provider     {} ({})", provider.label(), provider.health().await.unwrap_or_else(|e| e.to_string()));
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
                .route("/brand.css", get(brand_css))
                .route("/auth/magic/{token}", get(magic_redeem))
                .route("/auth/passkey/register/start", post(passkey_register_start))
                .route("/auth/passkey/register/finish", post(passkey_register_finish))
                .route("/auth/passkey/login/start", post(passkey_login_start))
                .route("/auth/passkey/login/finish", post(passkey_login_finish))
                .route("/auth/logout", post(logout))
                .route("/api/chat/stream/{turn_id}", get(chat_stream))
                .leptos_routes(&state, routes, {
                    let opts = leptos_options.clone();
                    move || shell(opts.clone())
                })
                .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
                .layer(axum::middleware::from_fn_with_state(state.clone(), auth_gate))
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
