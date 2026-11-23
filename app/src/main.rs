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
        /// Relay for sandboxed agent CLIs: stdio <-> the app's tool socket (token in JUGG_MCP_TOKEN).
        #[command(hide = true)]
        McpProxy {
            #[arg(long)]
            socket: std::path::PathBuf,
        },
        /// Connect an intelligence provider: claude, codex or openrouter.
        Connect { provider: String },
        /// Print the one-time setup link for a fresh install.
        SetupUrl,
        /// Print health of crypto, database, provider and tools.
        Doctor,
        /// Run the security + maintenance daemon (separate user, CAP_NET_ADMIN).
        Steward {
            /// Print the running steward's status and exit.
            #[arg(long)]
            status: bool,
        },
        /// Encrypted, signed backups.
        Backup {
            #[command(subcommand)]
            cmd: BackupCmd,
        },
        /// Install, list or verify the pinned creative tools.
        Tools {
            #[command(subcommand)]
            cmd: ToolsCmd,
        },
        /// Seal the encryption + signing keys into systemd credentials (TPM2 when present).
        Keys {
            #[command(subcommand)]
            cmd: KeysCmd,
        },
        /// Export the signed audit log, or verify an export offline.
        Audit {
            #[command(subcommand)]
            cmd: AuditCmd,
        },
        /// Print the nftables ruleset for this box.
        Firewall {
            /// Allow SSH only from these addresses/ranges (e.g. 100.64.0.0/10 for Tailscale).
            #[arg(long = "ssh-allow")]
            ssh_allow: Vec<String>,
            #[arg(long = "ssh-port", default_value_t = 22)]
            ssh_port: u16,
        },
    }

    #[derive(Subcommand)]
    enum AuditCmd {
        /// Write the whole audit log + signed checkpoints as JSON to stdout.
        Export,
        /// Check an export with nothing but the file (works on any machine).
        Verify {
            file: std::path::PathBuf,
            /// The box signing-key fingerprint from `juggernaught doctor`, recorded somewhere safe.
            #[arg(long)]
            fingerprint: Option<String>,
        },
    }

    #[derive(Subcommand)]
    enum KeysCmd {
        /// Show whether the keys are sealed and where they come from.
        Status,
        /// Seal every key with systemd-creds and destroy the plaintext files (root).
        Seal,
        /// Turn sealed credentials back into key files (root; to move hardware).
        Unseal,
    }

    #[derive(Subcommand)]
    enum ToolsCmd {
        /// Show what is installed.
        Status,
        /// Install tools (all of them with --all).
        Install {
            names: Vec<String>,
            #[arg(long)]
            all: bool,
        },
        /// Re-run every installed tool's smoke test.
        Verify,
    }

    #[derive(Subcommand)]
    enum BackupCmd {
        /// Back up the database, assets, site and config now.
        Create,
        /// Decrypt the newest backup with this box's key and check every file + the database.
        Test {
            file: Option<std::path::PathBuf>,
        },
        /// Generate the owner's offline recovery key (store it somewhere safe, off this machine).
        InitRecovery {
            #[arg(long, default_value = "juggernaught-recovery-key.json")]
            out: std::path::PathBuf,
        },
        /// Restore onto a fresh machine using the owner's recovery key.
        Restore {
            file: std::path::PathBuf,
            #[arg(long)]
            recovery_key: std::path::PathBuf,
        },
    }

    // Outside `cargo leptos`, tell Leptos where the built site lives.
    if std::env::var_os("LEPTOS_OUTPUT_NAME").is_none() {
        std::env::set_var("LEPTOS_OUTPUT_NAME", "juggernaught");
    }
    if std::env::var_os("LEPTOS_SITE_ROOT").is_none() {
        let beside_binary = std::env::current_exe().ok().and_then(|p| p.parent().and_then(|d| d.parent()).map(|d| d.join("site"))).filter(|p| p.join("pkg").exists());
        std::env::set_var("LEPTOS_SITE_ROOT", beside_binary.unwrap_or_else(|| "target/site".into()));
    }
    let cli = Cli::parse();
    // The relay runs inside the agent sandbox: no config, no keys, no logging setup.
    if let Some(Cmd::McpProxy { socket }) = &cli.cmd {
        let token = std::env::var("JUGG_MCP_TOKEN").unwrap_or_default();
        if let Err(e) = jugg_tools::mcp::relay(socket, &token, tokio::io::stdin(), tokio::io::stdout()).await {
            eprintln!("mcp-proxy: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(Cmd::Audit { cmd: AuditCmd::Verify { file, fingerprint } }) = &cli.cmd {
        let x: jugg_db::audit::Export = match std::fs::read(file).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string())) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("{}: {e}", file.display());
                std::process::exit(2);
            }
        };
        match jugg_db::audit::verify_export(&x, fingerprint.as_deref()) {
            Ok(v) => {
                println!("audit export ok: {} entries, {} signed checkpoints, signed at {} by key {}", v.entries, v.checkpoints, x.exported_at, v.fingerprint);
                if fingerprint.is_none() {
                    println!("note: no --fingerprint given, so this proves internal consistency only; pin the key to prove it came from your box.");
                }
            }
            Err(e) => {
                eprintln!("audit export FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let cfg = match &cli.config {
        Some(p) => jugg_core::Config::load(p).unwrap_or_else(|e| panic!("config: {e}")),
        None => jugg_core::Config::dev(),
    };
    if cli.config.is_none() {
        // Children (provider CLIs → `juggernaught mcp`) must resolve the same dev data dir.
        std::env::set_var("JUGG_DEV_ROOT", cfg.data_dir.parent().unwrap_or(&cfg.data_dir));
    }

    let quiet = matches!(cli.cmd, Some(Cmd::Mcp { .. }) | Some(Cmd::SetupUrl) | Some(Cmd::Doctor) | Some(Cmd::Connect { .. }) | Some(Cmd::Firewall { .. }) | Some(Cmd::Keys { .. }) | Some(Cmd::Audit { .. }) | Some(Cmd::Steward { status: true }) | Some(Cmd::Tools { .. }));
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

    // Commands that must not touch the app's keys or database.
    match &cli.cmd {
        Some(Cmd::Keys { cmd }) => {
            let p = juggernaught::keys::Paths {
                keys_dir: cfg.keys_dir.clone(),
                cred_dir: cli.config.as_ref().and_then(|c| c.parent()).map(|d| d.join("creds")).unwrap_or_else(|| cfg.data_dir.join("creds")),
                unit_dir: "/etc/systemd/system".into(),
            };
            let r = match cmd {
                KeysCmd::Status => Ok(juggernaught::keys::status(&p)),
                KeysCmd::Seal => juggernaught::keys::seal(&p, &cfg.backup.recovery_public_key),
                KeysCmd::Unseal => juggernaught::keys::unseal(&p, "jugg"),
            };
            match r {
                Ok(msg) => println!("{}", msg.trim_end()),
                Err(e) => {
                    eprintln!("keys: {e}");
                    std::process::exit(1);
                }
            }
            return;
        }
        Some(Cmd::Firewall { ssh_allow, ssh_port }) => {
            for a in ssh_allow {
                if a.parse::<std::net::IpAddr>().is_err() && !a.split_once('/').is_some_and(|(ip, bits)| ip.parse::<std::net::IpAddr>().is_ok() && bits.parse::<u8>().is_ok()) {
                    eprintln!("--ssh-allow {a}: not an IP address or CIDR range");
                    std::process::exit(2);
                }
            }
            print!("{}", jugg_steward::firewall::base_ruleset(ssh_allow, *ssh_port));
            return;
        }
        Some(Cmd::Tools { cmd }) => {
            let lock = std::fs::read_to_string(cfg.tools_dir.join("tools.lock.toml")).ok().and_then(|s| jugg_tools::lock::ToolsLock::parse(&s).ok()).unwrap_or_else(jugg_tools::lock::ToolsLock::embedded);
            match cmd {
                ToolsCmd::Status => {
                    for t in jugg_tools::install::status(&lock, &cfg.tools_dir) {
                        println!("{:<12} {:<20} {}", t.name, t.version, t.note);
                    }
                }
                ToolsCmd::Install { names, all } => {
                    let wanted: Vec<String> = if *all { lock.tool.iter().filter(|(_, t)| t.install != "unavailable").map(|(n, _)| n.clone()).collect() } else { names.clone() };
                    if wanted.is_empty() {
                        eprintln!("Name the tools to install, or use --all.");
                        std::process::exit(2);
                    }
                    let mut failed = 0;
                    for n in wanted {
                        match lock.tool.get(&n) {
                            None => {
                                eprintln!("{n}: not in tools.lock.toml");
                                failed += 1;
                            }
                            Some(t) => match jugg_tools::install::install(&cfg.tools_dir, &n, t) {
                                Ok(m) => println!("✓ {m}"),
                                Err(e) => {
                                    eprintln!("✗ {n}: {e}");
                                    failed += 1;
                                }
                            },
                        }
                    }
                    if failed > 0 {
                        std::process::exit(1);
                    }
                }
                ToolsCmd::Verify => {
                    let mut bad = 0;
                    for (n, r) in jugg_tools::install::verify(&lock, &cfg.tools_dir) {
                        match r {
                            Ok(line) => println!("✓ {n}: {line}"),
                            Err(e) => {
                                bad += 1;
                                println!("✗ {n}: {e}");
                            }
                        }
                    }
                    if bad > 0 {
                        std::process::exit(1);
                    }
                }
            }
            return;
        }
        Some(Cmd::Steward { status: true }) => {
            match jugg_steward::client::StewardClient::spawn(cfg.steward_socket.clone()).status().await {
                Some(s) => println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default()),
                None => {
                    eprintln!("Steward is not running at {}", cfg.steward_socket.display());
                    std::process::exit(1);
                }
            }
            return;
        }
        Some(Cmd::Steward { status: false }) => {
            if let Err(e) = jugg_steward::daemon::run(cfg).await {
                eprintln!("steward: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some(Cmd::Backup { cmd: BackupCmd::InitRecovery { out } }) => {
            let (file, public) = jugg_backup::new_recovery_key().expect("keygen");
            let json = serde_json::to_string_pretty(&file).expect("json");
            {
                use std::io::Write;
                let mut o = std::fs::OpenOptions::new();
                o.write(true).create_new(true);
                #[cfg(unix)]
                std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
                match o.open(out) {
                    Ok(mut f) => f.write_all(json.as_bytes()).expect("write recovery key"),
                    Err(e) => {
                        eprintln!("{}: {e} (refusing to overwrite)", out.display());
                        std::process::exit(1);
                    }
                }
            }
            let pub_path = &cfg.backup.recovery_public_key;
            if let Some(d) = pub_path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            std::fs::write(pub_path, serde_json::to_string_pretty(&public).expect("json")).expect("write public key");
            println!("Recovery key written to {}.", out.display());
            println!("Move it OFF this machine now (password manager, USB key, printed QR). Without it, a dead server cannot be restored elsewhere.");
            println!("Public half installed at {}; every backup from now on is escrowed to it.", pub_path.display());
            return;
        }
        Some(Cmd::Backup { cmd: BackupCmd::Restore { file, recovery_key } }) => {
            let rk: jugg_backup::RecoveryKeyFile = serde_json::from_slice(&std::fs::read(recovery_key).expect("read recovery key")).expect("recovery key file");
            let sk = rk.secret().expect("recovery key");
            if cfg.db_path().exists() || cfg.keys_dir.join("kek-1.key").exists() {
                eprintln!("This machine already has data at {}. Restore only onto a fresh install.", cfg.data_dir.display());
                std::process::exit(1);
            }
            let stage = cfg.data_dir.join(".restore");
            let (header, index) = jugg_backup::open(file, jugg_backup::Unlock::Recovery(&sk), &[], Some(&stage)).unwrap_or_else(|e| panic!("restore: {e}"));
            let keks = jugg_backup::escrowed_keks(&header, &sk).unwrap_or_else(|e| panic!("keys: {e}"));
            jugg_crypto::Keyring::restore_from_escrow(&cfg.keys_dir, &keks).unwrap_or_else(|e| panic!("keys: {e}"));
            std::fs::rename(stage.join("db/juggernaught.db"), cfg.db_path()).expect("database");
            for (from, to) in [("assets", cfg.assets_dir()), ("site", cfg.data_dir.join("site"))] {
                if stage.join(from).exists() {
                    let _ = std::fs::remove_dir_all(&to);
                    std::fs::rename(stage.join(from), &to).expect("restore files");
                }
            }
            let _ = std::fs::remove_dir_all(&stage);
            println!("Restored backup {} ({} files). Start the service and sign in as before.", header.id, index.len());
            return;
        }
        _ => {}
    }

    let conf = get_configuration(None).expect("leptos configuration");
    let mut leptos_options = conf.leptos_options;
    if cfg.dev {
        leptos_options.site_addr = cfg.listen_http.parse().expect("listen address");
    }
    #[cfg(unix)]
    if !cfg.dev && !matches!(cli.cmd, None | Some(Cmd::Serve)) && juggernaught::keys::should_reexec(&cfg.keys_dir) {
        if let Some(conf) = &cli.config {
            let creds = conf.parent().map(|d| d.join("creds")).unwrap_or_default();
            let e = juggernaught::keys::reexec_with_credentials(conf, &creds, "jugg");
            eprintln!("keys are sealed and systemd-run failed ({e}); run this with sudo");
            std::process::exit(1);
        }
    }
    let state = AppState::init(cfg, leptos_options.clone()).await.unwrap_or_else(|e| panic!("startup: {e}"));

    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::SetupUrl | Cmd::Keys { .. } | Cmd::Firewall { .. } | Cmd::Steward { .. } | Cmd::Tools { .. } | Cmd::McpProxy { .. } => unreachable!(),
        Cmd::Backup { cmd } => match cmd {
            BackupCmd::Create => {
                let tmp = state.cfg.data_dir.join(".backup-db.sqlite");
                jugg_backup::snapshot_db(&state.pool, &tmp).await.unwrap_or_else(|e| panic!("snapshot: {e}"));
                let recovery: Option<jugg_crypto::HybridPublicKey> = std::fs::read(&state.cfg.backup.recovery_public_key).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let mut roots: Vec<(&str, std::path::PathBuf)> = vec![("db/juggernaught.db", tmp.clone()), ("assets", state.cfg.assets_dir()), ("site", state.cfg.data_dir.join("site"))];
                if let Some(c) = &cli.config {
                    roots.push(("config/juggernaught.toml", c.clone()));
                }
                let files = jugg_backup::collect(&roots);
                let report = jugg_backup::create(&state.cfg.backup.dir, &files, &state.ring, &state.signer, recovery.as_ref());
                let _ = std::fs::remove_file(&tmp);
                let report = report.unwrap_or_else(|e| panic!("backup: {e}"));
                let pruned = jugg_backup::prune(&state.cfg.backup.dir, state.cfg.backup.keep).unwrap_or_default();
                if let Some(remote) = &state.cfg.backup.rclone_remote {
                    let ok = std::process::Command::new("rclone").args(["copy", "--immutable"]).arg(&report.file).arg(remote).status().map(|s| s.success()).unwrap_or(false);
                    println!("off-box copy to {remote}: {}", if ok { "ok" } else { "FAILED" });
                }
                let _ = jugg_db::audit::append(&state.pool, Some(&state.signer), "steward", "backup.created", &serde_json::json!({ "id": report.id, "files": report.entries, "bytes": report.bytes })).await;
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                if !report.recovery_escrow {
                    eprintln!("warning: no recovery key installed; this backup can only be restored on THIS machine. Run `juggernaught backup init-recovery`.");
                }
                if !pruned.is_empty() {
                    println!("pruned {} old backup(s)", pruned.len());
                }
            }
            BackupCmd::Test { file } => {
                let Some(file) = file.or_else(|| jugg_backup::latest(&state.cfg.backup.dir)) else {
                    eprintln!("no backups in {}", state.cfg.backup.dir.display());
                    std::process::exit(1);
                };
                let scratch = state.cfg.data_dir.join(".restore-test");
                let _ = std::fs::remove_dir_all(&scratch);
                let result = jugg_backup::open(&file, jugg_backup::Unlock::Box(&state.ring), &[state.signer.public_key()], Some(&scratch));
                let verdict = match result {
                    Ok((h, idx)) => {
                        let db = scratch.join("db/juggernaught.db");
                        let integrity = match jugg_db::sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect(&format!("sqlite://{}?mode=ro", db.display())).await {
                            Ok(p) => jugg_db::sqlx::query_scalar::<_, String>("PRAGMA integrity_check").fetch_one(&p).await.unwrap_or_else(|e| e.to_string()),
                            Err(e) => e.to_string(),
                        };
                        if integrity == "ok" {
                            Ok(format!("backup {} restores cleanly: {} files, database integrity ok", h.id, idx.len()))
                        } else {
                            Err(format!("database in backup {} is damaged: {integrity}", h.id))
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = std::fs::remove_dir_all(&scratch);
                match verdict {
                    Ok(m) => println!("{m}"),
                    Err(e) => {
                        eprintln!("RESTORE TEST FAILED: {e}");
                        std::process::exit(1);
                    }
                }
            }
            BackupCmd::InitRecovery { .. } | BackupCmd::Restore { .. } => unreachable!(),
        },
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
                    // On servers the login lives in the agent's own home (what the sandbox sees).
                    let mut c = std::process::Command::new(bin);
                    c.args(&args);
                    if !state.cfg.dev {
                        let home = agent_home(&state.cfg);
                        let _ = std::fs::create_dir_all(&home);
                        c.env("HOME", &home);
                    }
                    let status = c.status();
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
        Cmd::Audit { cmd } => match cmd {
            AuditCmd::Export => {
                let x = jugg_db::audit::export(&state.pool, &state.signer).await.unwrap_or_else(|e| panic!("export: {e}"));
                println!("{}", serde_json::to_string_pretty(&x).expect("json"));
            }
            AuditCmd::Verify { .. } => unreachable!(),
        },
        Cmd::Doctor => {
            let provider = state.provider().await;
            println!("database     ok  {}", state.cfg.db_path().display());
            println!("keyring      ok  KEK #{} ({})", state.ring.active_id(), if jugg_crypto::is_sealed(&state.cfg.keys_dir) { "sealed, from systemd credentials" } else { "plaintext key files; seal with `juggernaught keys seal`" });
            println!("signing key  ok  ML-DSA-65, fingerprint {}", jugg_db::audit::fingerprint(&state.signer.public_key()));
            println!("tls groups   {}", tls_provider_groups().join(", "));
            println!("admin        {}", admin_origin(&state.cfg));
            println!("mail         {}", if state.mailer.is_some() { "SMTP configured" } else { "not configured (magic links only logged in dev)" });
            println!("provider     {} ({})", provider.label(), provider.health().await.unwrap_or_else(|e| e.to_string()));
            for t in state.registry.specs() {
                println!("tool         {:<14} {:?}", t.name, t.risk);
            }
            match jugg_db::audit::verify_chain(&state.pool).await {
                Ok(None) => match jugg_db::audit::export(&state.pool, &state.signer).await.map_err(|e| e.to_string()).and_then(|x| jugg_db::audit::verify_export(&x, None)) {
                    Ok(v) => println!("audit chain  ok  {} entries, {} signed checkpoints verified", v.entries, v.checkpoints),
                    Err(e) => println!("audit chain  BROKEN {e}"),
                },
                Ok(Some(seq)) => println!("audit chain  BROKEN at seq {seq}"),
                Err(e) => println!("audit chain  error {e}"),
            }
        }
        Cmd::Serve => {
            let addr = leptos_options.site_addr;
            let st_cfg = state.cfg.clone();
            let routes = generate_route_list(App);
            tokio::spawn(juggernaught::assets::worker(state.clone()));
            {
                // Agent bridge: sandboxed CLIs reach the tools only through this socket.
                let st = state.clone();
                let registry = state.registry.clone();
                tokio::spawn(async move {
                    let tokens = st.mcp_tokens.clone();
                    let r = jugg_tools::mcp::serve_socket(st.mcp_socket(), registry, move |t| {
                        let now = jugg_core::now_unix();
                        tokens.lock().ok().and_then(|m| m.get(t).filter(|(_, exp)| *exp > now).map(|(ctx, _)| ctx.clone()))
                    })
                    .await;
                    if let Err(e) = r {
                        tracing::error!(error = %e, "agent bridge socket stopped");
                    }
                });
            }
            let app = Router::new()
                .route("/brand.css", get(brand_css))
                .route("/brand-mark.svg", get(brand_mark))
                .route("/auth/magic/{token}", get(magic_redeem))
                .route("/auth/passkey/register/start", post(passkey_register_start))
                .route("/auth/passkey/register/finish", post(passkey_register_finish))
                .route("/auth/passkey/login/start", post(passkey_login_start))
                .route("/auth/passkey/login/finish", post(passkey_login_finish))
                .route("/auth/logout", post(logout))
                .route("/api/chat/stream/{turn_id}", get(chat_stream))
                .route("/files/asset/{id}", get(juggernaught::assets::serve))
                .route("/files/handoff/{id}", post(juggernaught::images::upload).layer(axum::extract::DefaultBodyLimit::max(juggernaught::images::MAX_UPLOAD_BYTES + 1024)))
                .route("/_preview", get(|st: axum::extract::State<AppState>, req: axum::extract::Request| async move { juggernaught::site::preview(st, axum::extract::Path(String::new()), req).await }))
                .route("/_preview/{*rest}", get(juggernaught::site::preview))
                .route("/files/asset/{id}/{variant}", get(juggernaught::assets::serve))
                .leptos_routes(&state, routes, {
                    let opts = leptos_options.clone();
                    move || shell(opts.clone())
                })
                .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
                .layer(axum::middleware::from_fn_with_state(state.clone(), auth_gate))
                .layer(axum::middleware::from_fn_with_state(state.clone(), juggernaught::web::host_router))
                .layer(axum::middleware::from_fn_with_state(state.clone(), security_layer))
                .layer(tower_http::compression::CompressionLayer::new())
                .with_state(state);
            let tls = !st_cfg.dev && st_cfg.domain.is_some() && st_cfg.tls.mode == jugg_core::config::TlsMode::Acme;
            if !tls {
                let addr: SocketAddr = if st_cfg.dev { addr } else { st_cfg.listen_http.parse().expect("listen_http") };
                tracing::info!("Juggernaught listening on http://{addr}");
                let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
                axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("server");
                return;
            }
            // Production: Let's Encrypt via TLS-ALPN-01 on :443, PQ-hybrid key exchange first.
            use futures::StreamExt;
            let mut acme = rustls_acme::AcmeConfig::new(st_cfg.hostnames())
                .contact(st_cfg.acme_email.iter().map(|e| format!("mailto:{e}")))
                .cache(rustls_acme::caches::DirCache::new(st_cfg.acme_cache()))
                .directory_lets_encrypt(!st_cfg.tls.staging)
                .state();
            let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mut server_cfg = (*acme.default_rustls_config_with_provider(provider)).clone();
            server_cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            let acceptor = acme.axum_acceptor(std::sync::Arc::new(server_cfg));
            tokio::spawn(async move {
                while let Some(ev) = acme.next().await {
                    match ev {
                        Ok(ok) => tracing::info!(?ok, "acme"),
                        Err(err) => tracing::error!(%err, "acme"),
                    }
                }
            });
            let http_addr: SocketAddr = st_cfg.listen_http.parse().expect("listen_http");
            tokio::spawn(async move {
                let redirect = Router::new().fallback(juggernaught::web::redirect_to_https);
                let l = tokio::net::TcpListener::bind(http_addr).await.expect("bind :80");
                let _ = axum::serve(l, redirect).await;
            });
            let https_addr: SocketAddr = st_cfg.listen_https.parse().expect("listen_https");
            tracing::info!(hosts = ?st_cfg.hostnames(), "Juggernaught listening on https://{https_addr}");
            axum_server::bind(https_addr).acceptor(acceptor).serve(app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("server");
        }
    }
}

#[cfg(not(feature = "ssr"))]
pub fn main() {}
