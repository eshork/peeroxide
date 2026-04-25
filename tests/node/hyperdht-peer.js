/**
 * HyperDHT interop test peer.
 *
 * Starts a local testnet (1 bootstrap + 2 DHT nodes), then exposes a
 * command interface over stdin/stdout so the Rust integration test can
 * drive it.
 *
 * Protocol:
 *   stdout line 1: { "ready": true, "port": <bootstrap_port> }
 *
 *   stdin commands (one JSON per line):
 *     { "cmd": "announce", "topic": "<hex32>", "id": <n> }
 *     { "cmd": "lookup",   "topic": "<hex32>", "id": <n> }
 *     { "cmd": "shutdown" }
 *
 *   stdout replies (one JSON per line):
 *     { "id": <n>, "ok": true }                          // announce done
 *     { "id": <n>, "ok": true, "peers": [<hex32>, ...] } // lookup done
 *     { "id": <n>, "ok": false, "error": "<msg>" }       // any error
 */

'use strict'

const DHT = require('hyperdht')
const b4a = require('b4a')
const readline = require('readline')

async function main () {
  const bootstrap = new DHT({
    ephemeral: false,
    firewalled: false,
    host: '127.0.0.1',
    port: 0,
    bootstrap: []
  })
  await bootstrap.fullyBootstrapped()
  const bsPort = bootstrap.address().port
  const bsAddr = [{ host: '127.0.0.1', port: bsPort }]

  const node1 = new DHT({ ephemeral: false, firewalled: false, host: '127.0.0.1', bootstrap: bsAddr })
  const node2 = new DHT({ ephemeral: false, firewalled: false, host: '127.0.0.1', bootstrap: bsAddr })
  await node1.fullyBootstrapped()
  await node2.fullyBootstrapped()

  const peer = new DHT({ ephemeral: true, host: '127.0.0.1', bootstrap: bsAddr })
  await peer.ready()

  process.stdout.write(JSON.stringify({ ready: true, port: bsPort }) + '\n')

  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity })

  rl.on('line', async (line) => {
    let req
    try {
      req = JSON.parse(line.trim())
    } catch (e) {
      return
    }

    const { cmd, id } = req

    try {
      if (cmd === 'announce') {
        const topic = b4a.from(req.topic, 'hex')
        const keyPair = DHT.keyPair()
        await peer.announce(topic, keyPair, []).finished()
        process.stdout.write(JSON.stringify({ id, ok: true }) + '\n')

      } else if (cmd === 'lookup') {
        const topic = b4a.from(req.topic, 'hex')
        const peers = []
        for await (const data of peer.lookup(topic)) {
          for (const p of data.peers) {
            peers.push(b4a.toString(p.publicKey, 'hex'))
          }
        }
        process.stdout.write(JSON.stringify({ id, ok: true, peers }) + '\n')

      } else if (cmd === 'shutdown') {
        await cleanup()
        process.exit(0)
      }
    } catch (err) {
      process.stdout.write(JSON.stringify({ id, ok: false, error: err.message }) + '\n')
    }
  })

  rl.on('close', async () => {
    await cleanup()
    process.exit(0)
  })

  async function cleanup () {
    try { await peer.destroy() } catch (_) {}
    try { await node2.destroy() } catch (_) {}
    try { await node1.destroy() } catch (_) {}
    try { await bootstrap.destroy() } catch (_) {}
  }
}

main().catch((err) => {
  process.stderr.write(err.stack + '\n')
  process.exit(1)
})
