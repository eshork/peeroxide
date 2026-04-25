'use strict'

const DHT = require('hyperdht')
const Hyperswarm = require('hyperswarm')
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

  const serverDht = new DHT({ host: '127.0.0.1', bootstrap: bsAddr })
  await serverDht.fullyBootstrapped()

  const swarm = new Hyperswarm({ dht: serverDht })

  let connResolve = null

  swarm.on('connection', (socket, info) => {
    const remotePk = b4a.toString(info.publicKey, 'hex')

    socket.on('data', (data) => {
      process.stdout.write(JSON.stringify({
        event: 'data',
        from: remotePk,
        payload: b4a.toString(data, 'hex')
      }) + '\n')
    })

    socket.on('error', (err) => {
      process.stderr.write('conn error: ' + err.message + '\n')
    })

    socket.on('close', () => {
      process.stdout.write(JSON.stringify({
        event: 'disconnected',
        from: remotePk
      }) + '\n')
    })

    socket.write(b4a.from('hello from node'))

    process.stdout.write(JSON.stringify({
      event: 'connected',
      remotePk,
      isInitiator: info.client
    }) + '\n')

    if (connResolve) {
      connResolve(socket)
      connResolve = null
    }
  })

  const serverPk = b4a.toString(swarm.keyPair.publicKey, 'hex')

  const serverDhtPort = swarm.dht.address().port

  process.stdout.write(JSON.stringify({
    ready: true,
    port: bsPort,
    publicKey: serverPk,
    node1Port: node1.address().port,
    node2Port: node2.address().port,
    serverDhtPort
  }) + '\n')

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
      if (cmd === 'join') {
        const topic = b4a.from(req.topic, 'hex')
        const discovery = swarm.join(topic, { server: true, client: false })
        await discovery.flushed()
        process.stdout.write(JSON.stringify({ id, ok: true }) + '\n')

      } else if (cmd === 'join_client') {
        const topic = b4a.from(req.topic, 'hex')
        const discovery = swarm.join(topic, { server: false, client: true })
        await discovery.flushed()
        process.stdout.write(JSON.stringify({ id, ok: true }) + '\n')

      } else if (cmd === 'wait_connection') {
        const socket = await new Promise((resolve) => { connResolve = resolve })
        process.stdout.write(JSON.stringify({
          id,
          ok: true,
          remotePk: b4a.toString(socket.remotePublicKey, 'hex')
        }) + '\n')

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
    try { await swarm.destroy() } catch (_) {}
    try { await node2.destroy() } catch (_) {}
    try { await node1.destroy() } catch (_) {}
    try { await bootstrap.destroy() } catch (_) {}
  }
}

main().catch((err) => {
  process.stderr.write(err.stack + '\n')
  process.exit(1)
})
