# Ping Output Formats

The `ping` tool provides human-readable output on `stderr` by default and machine-parseable NDJSON on `stdout` when the `--json` flag is used.

## Human-Readable Output (stderr)

### Bootstrap Check
```text
BOOTSTRAP CHECK (3 nodes)
  bootstrap1.example.com:49737  OK  12ms  (20 nodes)  node_id=ab12...
  bootstrap2.example.com:49737  TIMEOUT
--- bootstrap summary ---
3 nodes, 2 reachable, 1 unreachable
50 unique peers discovered via routing tables
public address: 1.2.3.4:5000 (consistent across 2 nodes)
NAT type: consistent (hole-punchable)
```

### Targeted Ping
```text
PING 1.2.3.4:49737 (direct)
  [1] OK  12ms  node_id=ab12...
--- 1.2.3.4:49737 ping statistics ---
1 probes, 1 responded, 0 timed out (0% probe loss)
rtt min/avg/max = 12.0/12.0/12.0 ms
```

## JSON Output (stdout)

JSON output is emitted as newline-delimited objects.

### Resolve Events
Emitted before probing starts for PubKey and Topic targets.

```json
{"type":"resolve","method":"find_peer","public_key":"<64-hex>"}
{"type":"resolve","status":"found","addresses":2}
```

### Probe Events
Emitted for each individual probe. UDP probes use `rtt_ms`, while encrypted echo probes use `latency_ms`.

```json
// UDP probe (direct/bootstrap)
{"type":"probe","seq":1,"status":"ok","rtt_ms":12.3,"node_id":"<hex>"}

// Echo probe (connect mode)
{"type":"probe","seq":1,"status":"ok","latency_ms":48.3}
```

### Summary Events
Emitted at the end of a session if `count > 1` or `count == 0`.

```json
// Bootstrap summary
{
  "type": "bootstrap_summary",
  "nodes": 3,
  "reachable": 2,
  "unreachable": 1,
  "nat_type": "consistent",
  "closer_nodes_total": 50,
  "public_host": "1.2.3.4",
  "public_port": 5000,
  "port_consistent": true
}

// Targeted summary
{
  "type": "summary",
  "target": "1.2.3.4:49737",
  "probes_sent": 5,
  "probes_responded": 4,
  "probes_timed_out": 1,
  "rtt_min_ms": 11.0,
  "rtt_avg_ms": 20.5,
  "rtt_max_ms": 45.0
}
```

