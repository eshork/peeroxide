const dgram = require('dgram')
const UDX = require('udx-native')

async function main () {
  const packets = []

  const captureSocket = dgram.createSocket('udp4')
  captureSocket.bind(0, '127.0.0.1')
  await new Promise((resolve) => captureSocket.once('listening', resolve))
  const capturePort = captureSocket.address().port

  captureSocket.on('message', (msg) => {
    packets.push(Buffer.from(msg))
  })

  const u = new UDX()
  const socket = u.createSocket()
  socket.bind(0, '127.0.0.1')

  const REMOTE_ID = 42
  const LOCAL_ID = 99
  const stream = u.createStream(LOCAL_ID)
  stream.on('error', () => {})
  stream.connect(socket, REMOTE_ID, capturePort, '127.0.0.1')

  stream.write(Buffer.from('hello'))
  await new Promise((resolve) => setTimeout(resolve, 200))

  stream.end()
  await new Promise((resolve) => setTimeout(resolve, 200))

  const fixtures = packets.map((buf, i) => {
    const magic = buf[0]
    const version = buf[1]
    const typeFlags = buf[2]
    const dataOffset = buf[3]
    const remoteId = buf.readUInt32LE(4)
    const recvWindow = buf.readUInt32LE(8)
    const seq = buf.readUInt32LE(12)
    const ack = buf.readUInt32LE(16)

    const flags = []
    if (typeFlags & 0x01) flags.push('DATA')
    if (typeFlags & 0x02) flags.push('END')
    if (typeFlags & 0x04) flags.push('SACK')
    if (typeFlags & 0x08) flags.push('MESSAGE')
    if (typeFlags & 0x10) flags.push('DESTROY')
    if (typeFlags & 0x20) flags.push('HEARTBEAT')

    return {
      index: i,
      hex: buf.toString('hex'),
      length: buf.length,
      header: {
        magic,
        version,
        typeFlags,
        typeFlagsNames: flags,
        dataOffset,
        remoteId,
        recvWindow,
        seq,
        ack
      },
      payloadHex: buf.length > 20 ? buf.slice(20).toString('hex') : null
    }
  })

  process.stdout.write(JSON.stringify({ fixtures, remoteId: REMOTE_ID, localId: LOCAL_ID }, null, 2) + '\n')

  stream.destroy()
  socket.close()
  captureSocket.close()
}

main().catch((err) => {
  process.stderr.write(err.stack + '\n')
  process.exit(1)
})
