//! Man page generation with rich descriptions, examples, and cross-references.

use clap::CommandFactory;
use std::io::Write;

const CONSOLIDATED: &[&str] = &["peeroxide-cp", "peeroxide-dd"];

/// Generate all man pages and return them as (filename_stem, content) pairs.
pub fn generate_all() -> Vec<(String, Vec<u8>)> {
    let cmd = super::Cli::command();
    let mut pages = Vec::new();
    collect_pages(cmd, "peeroxide", &mut pages);
    pages
}

fn collect_pages(cmd: clap::Command, prefix: &str, pages: &mut Vec<(String, Vec<u8>)>) {
    let page = render_page(cmd.clone(), prefix);
    pages.push((prefix.to_string(), page));

    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let sub_prefix = format!("{prefix}-{}", sub.get_name());
        if CONSOLIDATED.contains(&sub_prefix.as_str()) {
            let page = render_consolidated_page(sub.clone(), &sub_prefix);
            pages.push((sub_prefix, page));
        } else {
            collect_pages(sub.clone(), &sub_prefix, pages);
        }
    }
}

fn render_page(cmd: clap::Command, name: &str) -> Vec<u8> {
    let cmd = cmd
        .display_name(name.to_owned())
        .bin_name(name.to_owned());
    let cmd = apply_long_about(cmd, name);

    let man = clap_mangen::Man::new(cmd.clone())
        .title(name.to_string())
        .manual("peeroxide manual".to_string())
        .source(format!(
            "peeroxide {}",
            cmd.get_version().unwrap_or_default()
        ));

    let mut buf: Vec<u8> = Vec::new();
    man.render_title(&mut buf).unwrap();
    man.render_name_section(&mut buf).unwrap();
    man.render_synopsis_section(&mut buf).unwrap();
    man.render_description_section(&mut buf).unwrap();
    man.render_options_section(&mut buf).unwrap();

    if cmd.has_subcommands() {
        man.render_subcommands_section(&mut buf).unwrap();
    }

    if let Some(examples) = examples_for(name) {
        write_examples_section(&mut buf, examples);
    }

    if let Some(exit) = exit_status_for(name) {
        write_section(&mut buf, "EXIT STATUS", exit);
    }

    if let Some(see_also) = see_also_for(name) {
        write_see_also_section(&mut buf, see_also);
    }

    buf
}

fn render_consolidated_page(cmd: clap::Command, name: &str) -> Vec<u8> {
    let cmd = cmd
        .display_name(name.to_owned())
        .bin_name(name.to_owned());
    let cmd = apply_long_about(cmd, name);

    let man = clap_mangen::Man::new(cmd.clone())
        .title(name.to_string())
        .manual("peeroxide manual".to_string())
        .source(format!(
            "peeroxide {}",
            cmd.get_version().unwrap_or_default()
        ));

    let mut buf: Vec<u8> = Vec::new();
    man.render_title(&mut buf).unwrap();
    man.render_name_section(&mut buf).unwrap();

    write_consolidated_synopsis(&mut buf, &cmd, name);
    man.render_description_section(&mut buf).unwrap();
    write_consolidated_commands(&mut buf, &cmd, name);

    if let Some(examples) = examples_for(name) {
        write_examples_section(&mut buf, examples);
    }

    if let Some(exit) = exit_status_for(name) {
        write_section(&mut buf, "EXIT STATUS", exit);
    }

    if let Some(see_also) = see_also_for(name) {
        write_see_also_section(&mut buf, see_also);
    }

    buf
}

fn write_consolidated_synopsis(buf: &mut Vec<u8>, cmd: &clap::Command, parent_name: &str) {
    buf.write_all(b".SH SYNOPSIS\n").unwrap();
    let invocation_base = parent_name.replace('-', " ");
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        writeln!(buf, ".B {invocation_base} {}", sub.get_name()).unwrap();
        let mut opts = Vec::new();
        for arg in sub.get_arguments().filter(|a| !a.is_hide_set() && !is_global_arg(a)) {
            if arg.is_positional() {
                if arg.is_required_set() {
                    opts.push(format!("<{}>", arg.get_id()));
                } else {
                    opts.push(format!("[{}]", arg.get_id()));
                }
            } else if let Some(long) = arg.get_long() {
                if arg.get_action().takes_values() {
                    opts.push(format!("[--{long} {val}]", val = arg.get_id().as_str().to_uppercase()));
                } else {
                    opts.push(format!("[--{long}]"));
                }
            }
        }
        if !opts.is_empty() {
            writeln!(buf, "{}", opts.join(" ")).unwrap();
        }
        buf.write_all(b".br\n").unwrap();
    }
}

fn write_consolidated_commands(buf: &mut Vec<u8>, cmd: &clap::Command, parent_name: &str) {
    buf.write_all(b".SH COMMANDS\n").unwrap();
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        let sub_key = format!("{parent_name}-{}", sub.get_name());
        writeln!(buf, ".SS {}", sub.get_name()).unwrap();

        if let Some(long) = long_about_for(&sub_key) {
            for line in long.lines() {
                if line.trim().is_empty() {
                    buf.write_all(b".PP\n").unwrap();
                } else {
                    writeln!(buf, "{line}").unwrap();
                }
            }
        } else if let Some(about) = sub.get_about() {
            writeln!(buf, "{about}").unwrap();
        }

        let args: Vec<_> = sub
            .get_arguments()
            .filter(|a| !a.is_hide_set() && !is_global_arg(a))
            .collect();
        if !args.is_empty() {
            buf.write_all(b".PP\n").unwrap();
            for arg in args {
                write_arg_tp(buf, arg);
            }
        }
    }
}

fn is_global_arg(arg: &clap::Arg) -> bool {
    matches!(
        arg.get_id().as_str(),
        "config"
            | "no_default_config"
            | "public"
            | "no_public"
            | "bootstrap"
            | "verbose"
            | "help"
    )
}

fn write_arg_tp(buf: &mut Vec<u8>, arg: &clap::Arg) {
    buf.write_all(b".TP\n").unwrap();
    if arg.is_positional() {
        let name = arg.get_id().as_str().to_uppercase();
        if arg.is_required_set() {
            writeln!(buf, "\\fB<{name}>\\fR").unwrap();
        } else {
            writeln!(buf, "\\fB[{name}]\\fR").unwrap();
        }
    } else {
        let mut label = String::new();
        if let Some(short) = arg.get_short() {
            label.push_str(&format!("\\fB-{short}\\fR"));
            if arg.get_long().is_some() {
                label.push_str(", ");
            }
        }
        if let Some(long) = arg.get_long() {
            label.push_str(&format!("\\fB--{long}\\fR"));
        }
        if arg.get_action().takes_values() {
            let val = arg
                .get_value_names()
                .and_then(|v| v.first().map(|s| s.to_string()))
                .unwrap_or_else(|| arg.get_id().as_str().to_uppercase());
            label.push_str(&format!(" \\fI{val}\\fR"));
        }
        writeln!(buf, "{label}").unwrap();
    }
    if let Some(help) = arg.get_help() {
        writeln!(buf, "{help}").unwrap();
    }
}

fn write_examples_section(buf: &mut Vec<u8>, examples: &[(&str, &str)]) {
    buf.write_all(b".SH EXAMPLES\n").unwrap();
    for (cmd, desc) in examples {
        buf.write_all(b".PP\n").unwrap();
        writeln!(buf, "{desc}").unwrap();
        buf.write_all(b".RS 4\n.nf\n").unwrap();
        writeln!(buf, "\\fB{cmd}\\fR").unwrap();
        buf.write_all(b".fi\n.RE\n").unwrap();
    }
}

fn write_see_also_section(buf: &mut Vec<u8>, refs: &[&str]) {
    buf.write_all(b".SH SEE ALSO\n").unwrap();
    let formatted: Vec<String> = refs
        .iter()
        .map(|r| format!("\\fB{r}\\fR(1)"))
        .collect();
    writeln!(buf, "{}", formatted.join(",\n")).unwrap();
}

fn write_section(buf: &mut Vec<u8>, heading: &str, body: &str) {
    write!(buf, ".SH {heading}\n{body}\n").unwrap();
}

fn apply_long_about(cmd: clap::Command, name: &str) -> clap::Command {
    match long_about_for(name) {
        Some(text) => cmd.long_about(text),
        None => cmd,
    }
}

fn long_about_for(name: &str) -> Option<&'static str> {
    match name {
        "peeroxide" => Some(
            "peeroxide is a command-line interface for the peeroxide P2P networking stack. \
             It provides tools for DHT-based peer discovery, direct connectivity testing, \
             file transfer, and anonymous messaging over the Hyperswarm-compatible network.\n\n\
             The tool connects to the public Hyperswarm DHT by default, or to custom \
             bootstrap nodes specified via --bootstrap flags or the configuration file. \
             All subcommands share a common set of global options for network configuration.\n\n\
             Use --public to include the public HyperDHT bootstrap nodes, or --no-public \
             to exclude them. If no bootstrap nodes are configured and --no-public is not \
             given, the public bootstrap is used automatically.",
        ),
        "peeroxide-node" => Some(
            "Run a long-lived DHT coordination (bootstrap) node that participates in the \
             distributed hash table routing layer. Bootstrap nodes help new peers discover \
             the network and facilitate Kademlia routing table population.\n\n\
             A node listens for incoming DHT RPC requests and maintains routing state. \
             Use --public to include the public HyperDHT bootstrap nodes (required for \
             production bootstrap nodes to join the network). The --port flag binds to a \
             specific UDP port for consistent addressing.\n\n\
             The node runs until terminated by SIGTERM or SIGINT.",
        ),
        "peeroxide-lookup" => Some(
            "Query the DHT for peers that have announced a given topic. Topics can be \
             specified as a human-readable string (hashed with BLAKE2b to produce a \
             discovery key) or as a 64-character hex-encoded raw topic.\n\n\
             Results are printed as they arrive from the DHT. Each result includes the \
             peer's public key and relay addresses (if hole-punching assistance is needed). \
             Use --json for machine-parseable NDJSON output. Use --with-data to include \
             any opaque data blobs peers attached during their announcement.\n\n\
             The lookup runs until the DHT query completes or SIGINT is received.",
        ),
        "peeroxide-announce" => Some(
            "Announce this peer's presence on a topic in the DHT so other peers can \
             discover it via lookup. The announcement is periodically refreshed to remain \
             active in the DHT.\n\n\
             Topics can be specified as a human-readable string (hashed with BLAKE2b to \
             produce a discovery key) or as a 64-character hex-encoded raw topic.\n\n\
             With --ping, the peer also listens for incoming encrypted connections and \
             responds to echo (PING/PONG) probes, useful for verifying end-to-end \
             connectivity. With --data, an opaque blob (up to 64 bytes) is attached to \
             the announcement and returned in lookup results.\n\n\
             The announcement continues until the specified --duration elapses, or until \
             terminated by SIGTERM or SIGINT.",
        ),
        "peeroxide-ping" => Some(
            "Diagnose reachability of a DHT node or peer. Supports three target forms, \
             plus a no-target bootstrap check mode:\n\n\
             (no target) — Ping all configured bootstrap nodes and report your public \
             address, NAT type, and the number of DHT peers each bootstrap knows about.\n\n\
             host:port — Send a DHT-level UDP ping to the given address.\n\n\
             @<pubkey> — Look up the peer by public key in the DHT, then attempt a \
             Noise-encrypted connection and PING/PONG echo exchange.\n\n\
             <topic> — Look up the topic in the DHT, then ping all discovered peers.\n\n\
             In bootstrap check mode (no target), the resolved bootstrap list comes from \
             the config file, --bootstrap flags, or public defaults (with --public or by \
             default when no other bootstrap is configured). The \
             output includes per-node reachability and routing table size, your reflexive \
             public address, a NAT type classification (open, consistent, random, or \
             multi-homed), and the total unique peers discovered across all bootstraps.\n\n\
             For address pings, RTT is measured from the UDP request/response. For peer \
             pings, the full connection setup (DHT lookup + Noise handshake + echo) is timed.\n\n\
             Use --count to send multiple probes with summary statistics. Use --connect with \
             address targets to perform a full encrypted connection test instead of just UDP. \
             Use --json for machine-parseable NDJSON output.\n\n\
             Exits 0 if all probes succeed, 1 if any fail, 130 on SIGINT.",
        ),
        "peeroxide-cp" => Some(
            "Copy files between peers over the Hyperswarm network using encrypted \
             peer-to-peer connections. The sender and receiver rendezvous on a shared \
             topic string.\n\n\
             The transfer uses a streaming protocol with 64KB chunks and includes \
             metadata (filename, size) exchanged before the payload. Files are written \
             atomically using a temporary file that is renamed on completion.",
        ),
        "peeroxide-cp-send" => Some(
            "Send a file to a peer over the Hyperswarm network. The sender announces on \
             the specified topic and waits for a receiver to connect. Once connected, the \
             file metadata (name, size) is sent as a JSON header followed by the file \
             contents in 64KB streaming chunks.\n\n\
             The file path may be '-' to read from stdin (requires specifying a filename \
             in the topic or metadata). Use --keep-alive to remain available for additional \
             receivers after the first transfer completes.\n\n\
             Prints the resolved topic to stdout so it can be communicated to the receiver.",
        ),
        "peeroxide-cp-recv" => Some(
            "Receive a file from a peer over the Hyperswarm network. The receiver looks \
             up the specified topic in the DHT, connects to the sender, and downloads the \
             file. The output path specifies where to write the received file.\n\n\
             If the output path is a directory, the filename from the sender's metadata is \
             used. If the file already exists, the transfer is aborted unless --force is \
             specified. Use --yes to skip all confirmation prompts.\n\n\
             The file is written atomically: data goes to a temporary file first, which is \
             renamed to the final path only after the full transfer succeeds and the size \
             is validated.",
        ),
        "peeroxide-dd" => Some(
            "Dead Drop: anonymous store-and-forward messaging via the DHT's mutable record \
             storage. Messages are encrypted with a passphrase-derived key and stored as mutable \
             DHT records that any peer can retrieve without knowing the sender's identity.\n\n\
             The dead drop uses a chunked binary format with CRC32c integrity checks. \
             Messages are limited to approximately 1000 bytes per chunk (with multi-chunk \
             support for larger payloads).",
        ),
        "peeroxide-dd-put" => Some(
            "Store an anonymous message at a dead drop location in the DHT. The message \
             is encrypted with a passphrase-derived keypair and stored as a mutable DHT \
             record.\n\n\
             The passphrase can be provided inline with --passphrase or prompted \
             interactively (hidden input) with --interactive-passphrase. The keypair \
             is deterministically derived from the passphrase, so both sender and \
             receiver must use the same passphrase to exchange messages.\n\n\
             The positional argument is a file path to read, or '-' to read from stdin. \
             The DHT record is stored with acknowledgement confirmation from routing table \
             peers. Records persist in the DHT as long as nodes cache them (typically hours \
             to days depending on network conditions).",
        ),
        "peeroxide-dd-get" => Some(
            "Retrieve a message from a dead drop location in the DHT. The pickup key \
             can be a 64-character hex public key, a passphrase string (if less than 64 \
             hex chars), or derived interactively.\n\n\
             Use --passphrase to supply the passphrase inline, or --interactive-passphrase \
             to be prompted with hidden input. If a positional key argument is given that \
             is not valid hex, it is treated as a passphrase.\n\n\
             The retrieved message is written to stdout (or to a file with --output). If \
             no message is found at the specified location, or if decryption fails (wrong \
             passphrase), an error is reported.\n\n\
             The get operation is read-only and does not modify or consume the stored \
             record -- the same message can be retrieved multiple times by different peers.",
        ),

        "peeroxide-init" => Some(
            "Initialize a peeroxide config file or install man pages. This command has \
             two mutually exclusive modes:\n\n\
             Config mode (default): Creates a commented TOML config file with sane defaults \
             at ~/.config/peeroxide/config.toml (or the path given by --config). Use --force \
             to overwrite an existing config, or --update to patch specific fields without \
             disturbing other settings.\n\n\
             Man page mode (--man-pages): Generates and installs roff man pages into the \
             specified directory (default: /usr/local/share/man/man1/). No config is touched \
             in this mode.",
        ),
        _ => None,
    }
}

fn examples_for(name: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match name {
        "peeroxide" => Some(&[
            (
                "peeroxide --public ping",
                "Verify bootstrap connectivity and discover your public address:",
            ),
            (
                "peeroxide ping 1.2.3.4:49737",
                "Ping a known DHT bootstrap node:",
            ),
            (
                "peeroxide lookup my-service --json",
                "Look up peers on a topic with JSON output:",
            ),
            (
                "peeroxide announce my-service --ping",
                "Announce on a topic and respond to echo probes:",
            ),
            (
                "peeroxide cp send ./file.tar.gz my-transfer",
                "Send a file to a peer:",
            ),
            (
                "peeroxide node --public --port 49737",
                "Run a public bootstrap node:",
            ),
        ]),
        "peeroxide-node" => Some(&[
            (
                "peeroxide node",
                "Run a bootstrap node with default settings:",
            ),
            (
                "peeroxide node --port 49737 --public",
                "Run a public bootstrap node on the standard port:",
            ),
            (
                "peeroxide node --bootstrap other-node.example.com:49737",
                "Run a node bootstrapping from an existing peer:",
            ),
        ]),
        "peeroxide-lookup" => Some(&[
            (
                "peeroxide lookup my-service",
                "Look up peers announcing a human-readable topic:",
            ),
            (
                "peeroxide lookup a1b2c3...64chars --json",
                "Look up a hex topic with JSON output:",
            ),
            (
                "peeroxide lookup chat-room --with-data",
                "Include announcement data in results:",
            ),
        ]),
        "peeroxide-announce" => Some(&[
            (
                "peeroxide announce my-service",
                "Announce on a topic indefinitely:",
            ),
            (
                "peeroxide announce my-service --ping",
                "Announce and respond to connectivity probes:",
            ),
            (
                "peeroxide announce my-service --duration 3600 --data hello",
                "Announce for one hour with attached data:",
            ),
            (
                "peeroxide announce my-service --seed 0a1b2c...64chars",
                "Announce with a specific keypair seed:",
            ),
        ]),
        "peeroxide-ping" => Some(&[
            (
                "peeroxide --public ping",
                "Check bootstrap connectivity and discover your public address:",
            ),
            (
                "peeroxide ping 1.2.3.4:49737",
                "Send a UDP DHT ping to an address:",
            ),
            (
                "peeroxide ping @a1b2c3...64chars",
                "Ping a peer by public key (Noise handshake + echo):",
            ),
            (
                "peeroxide ping @a1b2c3...64chars --count 5 --interval 2",
                "Send 5 probes at 2-second intervals:",
            ),
            (
                "peeroxide ping 1.2.3.4:49737 --connect",
                "Full encrypted connection test to an address:",
            ),
            (
                "peeroxide ping my-service --json",
                "Ping all peers on a topic with JSON output:",
            ),
        ]),
        "peeroxide-cp" => Some(&[
            (
                "peeroxide cp send ./report.pdf my-transfer",
                "Send a file, printing the topic for the receiver:",
            ),
            (
                "cat data.bin | peeroxide cp send - my-transfer",
                "Stream from stdin:",
            ),
            (
                "peeroxide cp send ./file.txt my-transfer --keep-alive",
                "Send to multiple receivers (stays alive after first):",
            ),
            (
                "peeroxide cp recv my-transfer ./downloads/",
                "Receive into a directory (uses sender's filename):",
            ),
            (
                "peeroxide cp recv my-transfer ./output.bin --force",
                "Receive and overwrite an existing file:",
            ),
            (
                "peeroxide cp recv my-transfer - > file.bin",
                "Receive to stdout:",
            ),
        ]),
        "peeroxide-dd" => Some(&[
            (
                "echo 'secret message' | peeroxide dd put - --passphrase s3cret",
                "Put a message at a dead drop with an inline passphrase (read from stdin):",
            ),
            (
                "peeroxide dd put ./msg.txt --interactive-passphrase",
                "Put a file at a dead drop with a prompted passphrase (hidden input):",
            ),
            (
                "peeroxide dd get --passphrase s3cret",
                "Get a message from a dead drop using the same passphrase:",
            ),
            (
                "peeroxide dd get --interactive-passphrase --output ./msg.txt",
                "Get with prompted passphrase, write to file:",
            ),
            (
                "peeroxide dd get a1b2c3...64chars",
                "Get using a raw hex public key:",
            ),
        ]),

        "peeroxide-init" => Some(&[
            (
                "peeroxide init",
                "Create a default config file at ~/.config/peeroxide/config.toml:",
            ),
            (
                "peeroxide init --public --bootstrap node1.example.com:49737",
                "Create a config with public mode and custom bootstrap:",
            ),
            (
                "peeroxide init --force",
                "Overwrite an existing config file:",
            ),
            (
                "peeroxide init --update --public",
                "Enable public mode in an existing config without changing other settings:",
            ),
            (
                "peeroxide init --man-pages",
                "Install man pages to /usr/local/share/man/man1/:",
            ),
            (
                "peeroxide init --man-pages ~/.local/share/man/",
                "Install man pages to a custom directory:",
            ),
        ]),
        _ => None,
    }
}

fn exit_status_for(name: &str) -> Option<&'static str> {
    match name {
        "peeroxide" | "peeroxide-init" | "peeroxide-node" | "peeroxide-lookup"
        | "peeroxide-announce" | "peeroxide-cp" | "peeroxide-dd" => Some(
            ".TP\n\\fB0\\fR\nSuccess.\n\
             .TP\n\\fB1\\fR\nFailure or partial failure.\n\
             .TP\n\\fB2\\fR\nUsage error (invalid arguments).\n\
             .TP\n\\fB130\\fR\nInterrupted by SIGINT.",
        ),
        "peeroxide-ping" => Some(
            ".TP\n\\fB0\\fR\nAll probes succeeded.\n\
             .TP\n\\fB1\\fR\nOne or more probes failed.\n\
             .TP\n\\fB2\\fR\nUsage error (invalid arguments).\n\
             .TP\n\\fB130\\fR\nInterrupted by SIGINT.",
        ),
        _ => None,
    }
}

fn see_also_for(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "peeroxide" => Some(&[
            "peeroxide-init",
            "peeroxide-node",
            "peeroxide-lookup",
            "peeroxide-announce",
            "peeroxide-ping",
            "peeroxide-cp",
            "peeroxide-dd",
        ]),
        "peeroxide-init" => Some(&["peeroxide"]),
        "peeroxide-node" => Some(&["peeroxide"]),
        "peeroxide-lookup" => Some(&["peeroxide-announce", "peeroxide"]),
        "peeroxide-announce" => Some(&["peeroxide-lookup", "peeroxide-ping", "peeroxide"]),
        "peeroxide-ping" => Some(&[
            "peeroxide-node",
            "peeroxide-announce",
            "peeroxide-lookup",
            "peeroxide",
        ]),
        "peeroxide-cp" => Some(&["peeroxide-dd", "peeroxide"]),

        "peeroxide-dd" => Some(&["peeroxide-cp", "peeroxide"]),
        _ => None,
    }
}
