#![deny(clippy::all)]

use clap::{CommandFactory, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod cmd;
mod config;
mod manpage;

#[derive(Parser)]
#[command(name = "peeroxide", version, about = "P2P networking CLI for the Hyperswarm-compatible network")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Use this config file instead of the default location
    #[arg(long, global = true)]
    config: Option<String>,

    /// Ignore default config file entirely
    #[arg(long, global = true)]
    no_default_config: bool,

    /// Mark this node as publicly reachable
    #[arg(long, global = true, conflicts_with = "no_public")]
    public: bool,

    /// Mark this node as NOT publicly reachable (override config)
    #[arg(long, global = true, conflicts_with = "public")]
    no_public: bool,

    /// Force this node to report as firewalled (FIREWALL_CONSISTENT).
    /// Useful for testing firewall-specific connection paths.
    #[arg(long, global = true, conflicts_with = "public")]
    firewalled: bool,

    /// Bootstrap node addresses (host:port or ip:port), repeatable
    #[arg(long, global = true, action = clap::ArgAction::Append)]
    bootstrap: Vec<String>,

    /// Generate man pages to the specified directory
    #[arg(long, value_name = "DIR")]
    generate_man: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a long-running DHT coordination (bootstrap) node
    Node(cmd::node::NodeArgs),
    /// Query the DHT for peers announcing a topic
    Lookup(cmd::lookup::LookupArgs),
    /// Announce presence on a topic
    Announce(cmd::announce::AnnounceArgs),
    /// Diagnose reachability of a DHT node or peer
    Ping(cmd::ping::PingArgs),
    /// Copy files between peers over the swarm
    Cp {
        #[command(subcommand)]
        command: cmd::cp::CpCommands,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        command: cmd::config::ConfigCommands,
    },
    /// Anonymous store-and-forward via the DHT
    Deaddrop {
        #[command(subcommand)]
        command: cmd::deaddrop::DeaddropCommands,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    if let Some(dir) = cli.generate_man {
        std::process::exit(generate_manpages(&dir));
    }

    let Some(command) = cli.command else {
        Cli::command().print_help().ok();
        eprintln!();
        std::process::exit(2);
    };

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let exit_code = rt.block_on(async {
        match command {
            Commands::Config { command } => cmd::config::run(command).await,
            command => {
                let global = config::GlobalFlags {
                    config_path: cli.config,
                    no_default_config: cli.no_default_config,
                    public: if cli.public {
                        Some(true)
                    } else if cli.no_public {
                        Some(false)
                    } else {
                        None
                    },
                    firewalled: cli.firewalled,
                    bootstrap: if cli.bootstrap.is_empty() {
                        None
                    } else {
                        Some(cli.bootstrap)
                    },
                };

                let cfg = match config::load_config(&global) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("error: failed to load config: {e}");
                        return 1;
                    }
                };

                match command {
                    Commands::Node(args) => cmd::node::run(args, &cfg).await,
                    Commands::Lookup(args) => cmd::lookup::run(args, &cfg).await,
                    Commands::Announce(args) => cmd::announce::run(args, &cfg).await,
                    Commands::Ping(args) => cmd::ping::run(args, &cfg).await,
                    Commands::Cp { command } => cmd::cp::run(command, &cfg).await,
                    Commands::Deaddrop { command } => cmd::deaddrop::run(command, &cfg).await,
                    Commands::Config { .. } => unreachable!(),
                }
            }
        }
    });
    std::process::exit(exit_code);
}

fn generate_manpages(dir: &std::path::Path) -> i32 {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("error: cannot create directory {}: {e}", dir.display());
        return 1;
    }

    let pages = manpage::generate_all();
    for (name, content) in &pages {
        let path = dir.join(format!("{name}.1"));
        if let Err(e) = std::fs::write(&path, content) {
            eprintln!("error: failed to write {}: {e}", path.display());
            return 1;
        }
        eprintln!("{}", path.display());
    }

    eprintln!("Generated {} man page(s) in {}", pages.len(), dir.display());
    0
}
