const UDX = require('udx-native')
const readline = require('readline')

const u = new UDX()
const socket = u.createSocket()
socket.bind(0, '127.0.0.1')

const { port } = socket.address()
const maxStreams = Number.parseInt(process.env.MAX_STREAMS || '64', 10)

process.stdout.write(JSON.stringify({ port, maxStreams }) + '\n')

const rl = readline.createInterface({ input: process.stdin })

rl.once('line', (line) => {
  const info = JSON.parse(line)
  const remotePort = info.port
  const streamInfos = Array.isArray(info.streams) ? info.streams : []

  if (streamInfos.length === 0) {
    process.stdout.write(JSON.stringify({ ready: true }) + '\n')
    return
  }

  let closedStreams = 0
  const streams = new Set()

  for (const entry of streamInfos) {
    const stream = u.createStream(entry.localId)
    streams.add(stream)

    stream.on('error', () => {})

    stream.on('data', (data) => {
      stream.write(data)
    })

    stream.on('end', () => {
      stream.end()
    })

    stream.on('close', () => {
      closedStreams += 1
      if (closedStreams === streamInfos.length) {
        socket.close()
        process.exit(0)
      }
    })

    stream.connect(socket, entry.remoteId, remotePort, '127.0.0.1')
  }

  process.stdout.write(JSON.stringify({ ready: true }) + '\n')
})
