'use strict'

const DHT = require('hyperdht')
const b4a = require('b4a')
const readline = require('readline')

async function main () {
  const node = new DHT({ ephemeral: false, host: '0.0.0.0', port: 0 })
  await node.fullyBootstrapped()

  const kp = DHT.keyPair()
  const publicKeyHex = b4a.toString(kp.publicKey, 'hex')

  process.stdout.write(JSON.stringify({ ready: true, publicKey: publicKeyHex }) + '\n')

  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity })

  rl.on('line', async (line) => {
    let req
    try { req = JSON.parse(line.trim()) } catch (e) { return }

    const { cmd, id } = req

    try {
      if (cmd === 'announce') {
        const topic = b4a.from(req.topic, 'hex')
        await node.announce(topic, kp, []).finished()
        process.stdout.write(JSON.stringify({ id, ok: true }) + '\n')

      } else if (cmd === 'lookup') {
        const topic = b4a.from(req.topic, 'hex')
        const peers = []
        for await (const data of node.lookup(topic)) {
          for (const p of data.peers) {
            peers.push(b4a.toString(p.publicKey, 'hex'))
          }
        }
        process.stdout.write(JSON.stringify({ id, ok: true, peers }) + '\n')

      } else if (cmd === 'shutdown') {
        await node.destroy()
        process.exit(0)
      }
    } catch (err) {
      process.stdout.write(JSON.stringify({ id, ok: false, error: err.message }) + '\n')
    }
  })

  rl.on('close', async () => {
    try { await node.destroy() } catch (_) {}
    process.exit(0)
  })
}

main().catch((err) => {
  process.stderr.write(err.stack + '\n')
  process.exit(1)
})
