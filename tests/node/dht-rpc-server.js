/**
 * dht-rpc interop test server.
 *
 * Starts a DHT bootstrapper on 127.0.0.1 with a random port,
 * prints a JSON line to stdout once ready, and waits for stdin
 * to close before tearing down.
 *
 * Protocol:
 *   stdout line 1: { "ready": true, "port": <number> }
 *   stdin close  → graceful shutdown
 */

const DHT = require('dht-rpc')
const readline = require('readline')

async function main () {
  // Use port 0 to find an available port, then recreate with that port
  // so DHT.bootstrapper can compute a valid peer_id for 127.0.0.1:port.
  const probe = require('dgram').createSocket('udp4')
  await new Promise((resolve, reject) => {
    probe.bind(0, '127.0.0.1', (err) => (err ? reject(err) : resolve()))
  })
  const freePort = probe.address().port
  probe.close()

  const node = DHT.bootstrapper(freePort, '127.0.0.1')
  await node.ready()

  const addr = node.address()
  process.stdout.write(JSON.stringify({ ready: true, port: addr.port }) + '\n')

  // Keep alive until stdin closes (parent process signals shutdown).
  const rl = readline.createInterface({ input: process.stdin })
  rl.on('close', async () => {
    await node.destroy()
    process.exit(0)
  })
}

main().catch((err) => {
  process.stderr.write(err.stack + '\n')
  process.exit(1)
})
