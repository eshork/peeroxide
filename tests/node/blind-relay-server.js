'use strict'

const DHT = require('hyperdht')
const relay = require('blind-relay')
const b4a = require('b4a')

async function main () {
  const dht = new DHT()
  await dht.fullyBootstrapped()

  const relayServer = new relay.Server({
    createStream (opts) {
      return dht.rawStreams.add(opts)
    }
  })

  const server = dht.createServer(function (socket) {
    relayServer.accept(socket, { id: socket.remotePublicKey })
  })

  const keyPair = DHT.keyPair()
  await server.listen(keyPair)

  const addr = dht.address()

  process.stdout.write(JSON.stringify({
    ready: true,
    publicKey: b4a.toString(keyPair.publicKey, 'hex'),
    host: addr.host,
    port: addr.port
  }) + '\n')

  process.stdin.resume()
  process.stdin.on('end', async () => {
    await relayServer.close()
    await server.close()
    await dht.destroy()
    process.exit(0)
  })

  process.on('SIGTERM', async () => {
    await relayServer.close()
    await server.close()
    await dht.destroy()
    process.exit(0)
  })
}

main().catch((err) => {
  process.stderr.write(err.stack + '\n')
  process.exit(1)
})
