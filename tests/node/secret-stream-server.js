#!/usr/bin/env node
// TCP server that wraps connections with @hyperswarm/secret-stream (responder).
// Protocol: after encrypted stream is established, expects "ping" → replies "pong",
// then expects "hello from rust" → replies "hello from node",
// then expects "multi 0" through "multi 4" → replies "ack 0" through "ack 4",
// then closes.

const net = require('net')
const SecretStream = require('@hyperswarm/secret-stream')

const PORT = parseInt(process.env.PORT || '0', 10)

const server = net.createServer((socket) => {
  const ss = new SecretStream(false, socket)
  let msgIndex = 0
  const expected = [
    { recv: 'ping', send: 'pong' },
    { recv: 'hello from rust', send: 'hello from node' },
    { recv: 'multi 0', send: 'ack 0' },
    { recv: 'multi 1', send: 'ack 1' },
    { recv: 'multi 2', send: 'ack 2' },
    { recv: 'multi 3', send: 'ack 3' },
    { recv: 'multi 4', send: 'ack 4' },
  ]

  ss.on('data', (data) => {
    const msg = data.toString()
    if (msgIndex < expected.length) {
      const step = expected[msgIndex]
      if (msg !== step.recv) {
        process.stderr.write(`Expected "${step.recv}" but got "${msg}"\n`)
        process.exit(1)
      }
      ss.write(Buffer.from(step.send))
      msgIndex++
      if (msgIndex === expected.length) {
        ss.end()
      }
    }
  })

  ss.on('error', (err) => {
    process.stderr.write(`SecretStream error: ${err.message}\n`)
  })

  ss.on('close', () => {
    server.close()
  })
})

server.listen(PORT, '127.0.0.1', () => {
  const addr = server.address()
  process.stdout.write(`LISTENING:${addr.port}\n`)
})

server.on('error', (err) => {
  process.stderr.write(`Server error: ${err.message}\n`)
  process.exit(1)
})
