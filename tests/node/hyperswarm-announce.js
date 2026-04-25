'use strict'

// Hyperswarm announce — Node.js counterpart for `swarm_announce` example.
//
// Usage:
//   node hyperswarm-announce.js [hex-topic]
//
// If no topic is given, a random one is generated.
// Echoes received data back with "echo: " prefix.

const Hyperswarm = require('hyperswarm')
const crypto = require('crypto')
const b4a = require('b4a')

async function main () {
  const topicArg = process.argv[2]
  const topic = topicArg
    ? b4a.from(topicArg, 'hex')
    : crypto.randomBytes(32)

  const swarm = new Hyperswarm()

  swarm.on('connection', (socket, info) => {
    const remote = b4a.toString(info.publicKey, 'hex')
    console.log(`connected: ${remote} (initiator=${info.client})`)

    socket.on('data', (data) => {
      const msg = data.toString()
      console.log(`received: ${msg}`)
      socket.write('echo: ' + msg)
    })

    socket.on('error', (err) => {
      console.error('conn error:', err.message)
    })

    socket.on('close', () => {
      console.log(`disconnected: ${remote}`)
    })
  })

  const discovery = swarm.join(topic, { server: true, client: false })
  await discovery.flushed()

  console.log(`topic: ${b4a.toString(topic, 'hex')}`)
  console.log('announced — waiting for connections (Ctrl-C to stop)')

  process.on('SIGINT', async () => {
    console.log('shutting down...')
    await swarm.destroy()
    process.exit(0)
  })
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
