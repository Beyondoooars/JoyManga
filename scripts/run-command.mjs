import { spawn } from 'node:child_process'

export function createRun({
  spawnFn = spawn,
  stdout = process.stdout,
  stderr = process.stderr,
  env = process.env,
  platform = process.platform,
} = {}) {
  return function run(program, args, cwd) {
    return new Promise((resolve, reject) => {
      const limit = 4 * 1024 * 1024
      let chunks = []
      let length = 0
      const child = spawnFn(program, args, {
        cwd,
        env,
        shell: platform === 'win32' && !/\.(exe|com)$/i.test(program),
        windowsHide: true,
        stdio: ['inherit', 'pipe', 'pipe'],
      })
      // 只保留最后 4MB，边收边丢，避免长构建日志反复拼接整串（O(n²)）。
      const record = (stream) => (chunk) => {
        stream.write(chunk)
        chunks.push(chunk)
        length += chunk.length
        while (length > limit && chunks.length > 0) {
          const excess = length - limit
          if (chunks[0].length <= excess) {
            length -= chunks.shift().length
          } else {
            chunks[0] = chunks[0].subarray(excess)
            length -= excess
          }
        }
      }
      child.stdout.on('data', record(stdout))
      child.stderr.on('data', record(stderr))
      child.on('error', reject)
      child.on('close', (code) =>
        resolve({
          code: code ?? 1,
          output: Buffer.concat(chunks).toString('utf8'),
        }),
      )
    })
  }
}
