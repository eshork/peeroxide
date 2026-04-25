const UDX = require('udx-native')
const readline = require('readline')

const u = new UDX()
const socket = u.createSocket()
socket.bind(0, '127.0.0.1')

const { port } = socket.address()
const LOCAL_ID = 1
const REMOTE_ID = 2

process.stdout.write(JSON.stringify({ port, localId: LOCAL_ID, remoteId: REMOTE_ID }) + '\n')

const rl = readline.createInterface({ input: process.stdin })
rl.once('line', (line) => {
  const info = JSON.parse(line)
  const remotePort = info.port

  const stream = u.createStream(LOCAL_ID)
  stream.connect(socket, REMOTE_ID, remotePort, '127.0.0.1')

  process.stdout.write(JSON.stringify({ ready: true }) + '\n')

  stream.on('error', () => {})

  stream.on('data', (data) => {
    stream.write(data)
  })

  stream.on('close', () => {
    socket.close()
    process.exit(0)
  })

  stream.on('end', () => {
    stream.end()
  })
})
