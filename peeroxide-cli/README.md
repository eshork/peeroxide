# peeroxide-cli

Command-line interface for the peeroxide P2P networking stack. Wire-compatible with the existing Hyperswarm/HyperDHT network.

## Install

### From crates.io

```sh
cargo install peeroxide-cli
```

### From source

```sh
git clone https://github.com/Rightbracket/peeroxide.git
cd peeroxide
cargo build --release -p peeroxide-cli
# Binary is at target/release/peeroxide
```

The binary is named `peeroxide`.

## Quick Start

```sh
# 1. Initialize a config file (optional but recommended)
peeroxide init

# 2. Install man pages
peeroxide init --man-pages ~/.local/share/man/

# 3. Verify network connectivity and discover your public address
peeroxide --public ping
```

## Commands

| Command | Description |
|---------|-------------|
| `init` | Initialize config file or install man pages |
| `node` | Run a long-running DHT coordination (bootstrap) node |
| `lookup` | Query the DHT for peers announcing a topic |
| `announce` | Announce presence on a topic |
| `ping` | Diagnose reachability; bootstrap check, NAT classification, or targeted ping |
| `cp` | Copy files between peers over the swarm |
| `deaddrop` | Anonymous store-and-forward via the DHT |

Run `peeroxide <command> --help` for detailed usage of each command.

## Man Pages

Generate and install man pages:

```sh
peeroxide init --man-pages ~/.local/share/man/
```

If `~/.local/share/man` is not in your `MANPATH`, add it:

```sh
export MANPATH="$HOME/.local/share/man:$MANPATH"
```

This produces 8 pages:

```
peeroxide(1)          — main command and global options
peeroxide-init(1)     — config initialization and man page installation
peeroxide-node(1)     — bootstrap node operation
peeroxide-lookup(1)   — DHT topic lookup
peeroxide-announce(1) — DHT topic announcement
peeroxide-ping(1)     — connectivity diagnostics
peeroxide-cp(1)       — file transfer (send + recv)
peeroxide-deaddrop(1) — anonymous messaging (leave + pickup)
```

## Configuration

### Generating a config file

```sh
# Create config at default location (~/.config/peeroxide/config.toml)
peeroxide init

# Create config with public mode enabled
peeroxide init --public

# Create config with custom bootstrap nodes
peeroxide init --bootstrap node1.example.com:49737

# Overwrite existing config
peeroxide init --force

# Update specific fields in existing config
peeroxide init --update --public
```

### Config file location

peeroxide looks for configuration at (in order):

1. Path given by `--config <FILE>`
2. `$PEEROXIDE_CONFIG` environment variable
3. `~/.config/peeroxide/config.toml`

Use `--no-default-config` to skip config file loading entirely.

### Example config

```toml
[network]
bootstrap = ["bootstrap1.example.com:49737"]
public = true

[node]
port = 49737
```

### Global CLI flags

These flags apply to all subcommands:

| Flag | Description |
|------|-------------|
| `--config <FILE>` | Use a specific config file |
| `--no-default-config` | Ignore the default config entirely |
| `--bootstrap <ADDR>` | Add bootstrap nodes (repeatable) |
| `--public` | Mark this node as publicly reachable |
| `--no-public` | Force NAT mode (override config) |
| `--firewalled` | Force firewalled status for testing |

## Examples

```sh
# Check bootstrap connectivity and discover public address / NAT type
peeroxide --public ping

# Check bootstraps with JSON output (machine-parseable)
peeroxide --public ping --json

# Ping a known DHT node
peeroxide ping 1.2.3.4:49737

# Ping a peer by public key with 5 probes
peeroxide ping @<64-char-hex-pubkey> --count 5

# Full encrypted connection test
peeroxide ping 1.2.3.4:49737 --connect

# Announce on a topic and serve echo probes
peeroxide announce my-service --ping

# Look up who's on a topic (JSON output)
peeroxide lookup my-service --json

# Send a file (prints topic for receiver)
peeroxide cp send ./report.pdf my-transfer-topic

# Receive into a directory (uses sender's filename)
peeroxide cp recv my-transfer-topic ./downloads/

# Stream from stdin
cat data.bin | peeroxide cp send - my-transfer-topic

# Leave a dead drop message
echo 'secret' | peeroxide deaddrop leave - --passphrase s3cret

# Pick up a dead drop message
peeroxide deaddrop pickup --passphrase s3cret

# Run a public bootstrap node
peeroxide node --public --port 49737
```

## License

MIT OR Apache-2.0
