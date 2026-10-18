import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const rootDir = path.resolve(__dirname, '..')

const srcExe = path.join(rootDir, 'src-tauri', 'target', 'release', 'joymanga.exe')

// 目标目录：优先取环境变量，其次是 `pnpm release -- <dir>` 传入的位置。
// 不再内置任何开发者本机的绝对路径——公开仓库里每个人都能直接运行。
const cliTarget = process.argv.slice(2).find((arg) => !arg.startsWith('-'))
const targetDir = path.resolve(cliTarget ?? process.env.JOYMANGA_RELEASE_DIR ?? path.join(rootDir, 'dist-release'))

if (!fs.existsSync(srcExe)) {
  console.error(`[copy-release] Source file not found: ${srcExe}`)
  process.exit(1)
}

if (!fs.existsSync(targetDir)) {
  fs.mkdirSync(targetDir, { recursive: true })
}

const destExe = path.join(targetDir, 'joymanga.exe')
try {
  fs.copyFileSync(srcExe, destExe)
  console.log(`[copy-release] Successfully copied ${srcExe} -> ${destExe}`)
} catch (err) {
  if (err.code === 'EBUSY') {
    console.error(`[copy-release] 目标文件正在运行被占用: ${destExe}\n请先关闭正在运行的程序窗口后再执行覆盖。`)
    process.exit(1)
  }
  throw err
}

// 清理目标目录里遗留的安装包，保证这里是纯绿色便携版
const existingSetupFiles = fs.readdirSync(targetDir).filter((f) => f.endsWith('-setup.exe') || f.endsWith('.msi'))
for (const file of existingSetupFiles) {
  try {
    fs.unlinkSync(path.join(targetDir, file))
    console.log(`[copy-release] Removed leftover installer: ${file}`)
  } catch (err) {
    console.warn(`[copy-release] Failed to remove ${file}:`, err)
  }
}
