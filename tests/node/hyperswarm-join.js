'use strict'

// Hyperswarm join — Node.js counterpart for `swarm_join` example.
//
// Usage:
//   node hyperswarm-join.js <hex-topic> [message]
//
// Connects to a peer on the given topic, sends a message,
// prints the echo reply, then exits.

const Hyperswarm = require('hyperswarm')
const b4a = require('b4a')

async function main () {
  const topicHex = process.argv[2]
  if (!topicHex) {
    console.error('usage: node hyperswarm-join.js <hex-topic> [message]')
    process.exit(1)
  }
  const message = process.argv[3] || 'hello from node'
  const topic = b4a.from(topicHex, 'hex')

  const swarm = new Hyperswarm()

  const connected = new Promise((resolve) => {
    swarm.on('connection', (socket, info) => {
      resolve({ socket, info })
    })
  })

  const discovery = swarm.join(topic, { server: false, client: true })
  await discovery.flushed()
  console.log(`topic: ${topicHex}`)
  console.log('flushed — waiting for connection')

  const { socket, info } = await connected
  const remote = b4a.toString(info.publicKey, 'hex')
  console.log(`connected: ${remote} (initiator=${info.client})`)

  const reply = new Promise((resolve, reject) => {
    socket.once('data', (data) => resolve(data.toString()))
    socket.once('error', reject)
    setTimeout(() => reject(new Error('timeout waiting for reply')), 15000)
  })

  console.log(`sending: ${message}`)
  socket.write(message)

  const replyMsg = await reply
  console.log(`received echo: ${replyMsg}`)

  await swarm.destroy()
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
