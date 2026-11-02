import { test } from 'node:test'
import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { PassThrough } from 'node:stream'
import { setImmediate } from 'node:timers'
import { createRun } from '../scripts/run-command.mjs'

test('run waits for close and retains stderr emitted after exit', async () => {
  const child = new EventEmitter()
  child.stdout = new PassThrough()
  child.stderr = new PassThrough()
  const run = createRun({
    spawnFn: () => child,
    stdout: { write() {} },
    stderr: { write() {} },
    env: {},
    platform: 'linux',
  })
  let settled = false
  const running = run('tool', [], '/work').then((result) => {
    settled = true
    return result
  })

  child.stdout.write('early stdout')
  child.emit('exit', 1)
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(settled, false)

  child.stderr.write('late stderr')
  child.emit('close', 1)
  const result = await running
  assert.equal(result.code, 1)
  assert.match(result.output, /early stdout[\s\S]*late stderr/)
})
