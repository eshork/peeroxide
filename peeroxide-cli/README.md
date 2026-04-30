# peeroxide-cli

Command-line interface for the peeroxide P2P networking stack. Wire-compatible with the existing Hyperswarm/HyperDHT network.

## Install

```sh
cargo install peeroxide-cli
```

The binary is named `peeroxide`.

## Commands

| Command | Description |
|---------|-------------|
| `node` | Run a long-running DHT coordination (bootstrap) node |
| `lookup` | Query the DHT for peers announcing a topic |
| `announce` | Announce presence on a topic |
| `ping` | Diagnose reachability of a DHT node or peer |
| `cp` | Copy files between peers over the swarm |
| `deaddrop` | Anonymous store-and-forward via the DHT |

Run `peeroxide <command> --help` for detailed usage of each command.

## Man Pages

Generate and install man pages after `cargo install`:

```sh
# Generate all pages
peeroxide --generate-man ~/.local/share/man/man1/

# Or to a custom location
peeroxide --generate-man /tmp/peeroxide-man/
```

This produces man pages for the main command and all subcommands:

```
peeroxide(1)
peeroxide-node(1)
peeroxide-lookup(1)
peeroxide-announce(1)
peeroxide-ping(1)
peeroxide-cp(1)
peeroxide-deaddrop(1)
```

After installation, use `man peeroxide` or `man peeroxide-ping`, etc.

## Configuration

peeroxide looks for a config file at `~/.config/peeroxide/config.toml` (or `$PEEROXIDE_CONFIG`).

```toml
[network]
bootstrap = ["bootstrap1.example.com:49737"]
public = true
```

Override with CLI flags:

- `--config <FILE>` — use a specific config file
- `--no-default-config` — ignore the default config entirely
- `--bootstrap <ADDR>` — add bootstrap nodes (repeatable)
- `--public` / `--no-public` — override network reachability

## Examples

```sh
# Ping a known DHT node
peeroxide ping 1.2.3.4:49737

# Ping a peer by public key with 5 probes
peeroxide ping @<64-char-hex-pubkey> --count 5

# Announce on a topic and serve echo
peeroxide announce my-service --ping

# Look up who's on a topic
peeroxide lookup my-service --json

# Send a file
peeroxide cp send ./report.pdf my-transfer-topic

# Receive (on another machine)
peeroxide cp recv my-transfer-topic ./downloads/

# Run a bootstrap node
peeroxide node --public --port 49737
```

## License

MIT OR Apache-2.0
