use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Generate a config file with sane defaults and documentation
    Init(InitArgs),
}

#[derive(Args)]
pub struct InitArgs {
    /// Write to file instead of stdout
    #[arg(long)]
    output: Option<String>,
}

pub async fn run(cmd: ConfigCommands) -> i32 {
    match cmd {
        ConfigCommands::Init(args) => run_init(args).await,
    }
}

async fn run_init(args: InitArgs) -> i32 {
    let content = generate_default_config();

    if let Some(path) = args.output {
        let parent = std::path::Path::new(&path).parent();
        if let Some(dir) = parent {
            if !dir.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    eprintln!("error: cannot create directory: {e}");
                    return 1;
                }
            }
        }
        if let Err(e) = std::fs::write(&path, &content) {
            eprintln!("error: failed to write config: {e}");
            return 1;
        }
        eprintln!("Config written to {path}");
    } else {
        print!("{content}");
    }
    0
}

fn generate_default_config() -> String {
    r#"# Peeroxide configuration file
# Place at ~/.config/peeroxide/config.toml or set PEEROXIDE_CONFIG env var

[network]
# Whether this node is publicly reachable (not behind NAT/firewall)
# public = false

# Bootstrap node addresses (host:port). If empty and public=true, uses default public bootstrap.
# bootstrap = ["bootstrap1.example.com:49737"]

[node]
# Bind port for the DHT node (default: 49737)
# port = 49737

# Bind address (default: 0.0.0.0)
# host = "0.0.0.0"

# How often to log stats in seconds (default: 60)
# stats_interval = 60

# Max announcement records stored (default: 65536)
# max_records = 65536

# Max entries per LRU cache (default: 65536)
# max_lru_size = 65536

# Max peer announcements per topic (default: 20)
# max_per_key = 20

# TTL for announcement records in seconds (default: 1200)
# max_record_age = 1200

# TTL for LRU cache entries in seconds (default: 1200)
# max_lru_age = 1200

[announce]
# (No configurable options currently)

[cp]
# (No configurable options currently)
"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigFile;

    #[test]
    fn generated_config_is_valid_toml() {
        let content = generate_default_config();
        let parsed: ConfigFile = toml::from_str(&content).unwrap();
        assert!(parsed.network.public.is_none());
        assert!(parsed.network.bootstrap.is_none());
        assert!(parsed.node.port.is_none());
        assert!(parsed.node.host.is_none());
    }
}
