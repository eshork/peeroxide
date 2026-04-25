#!/usr/bin/env node
const net = require('net')
const SecretStream = require('@hyperswarm/secret-stream')
const Protomux = require('protomux')
const c = require('compact-encoding')

const PORT = parseInt(process.env.PORT || '0', 10)

const server = net.createServer((socket) => {
  const ss = new SecretStream(false, socket)
  const mux = Protomux.from(ss)

  const channel = mux.createChannel({
    protocol: 'peeroxide-interop-test',
    id: null,
    messages: [
      {
        encoding: c.raw,
        onmessage(data) {
          const msg = data.toString()
          if (msg === 'hello from rust') {
            channel.messages[0].send(Buffer.from('echo: hello from rust'))
            channel.messages[0].send(Buffer.from('goodbye'))
            channel.close()
          }
        }
      }
    ],
    onopen() {
      channel.messages[0].send(Buffer.from('hello from node'))
    },
    onclose() {
      server.close()
    }
  })

  channel.open()

  ss.on('error', (err) => {
    process.stderr.write(`SecretStream error: ${err.message}\n`)
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
