import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const rootDir = path.resolve(__dirname, '..')

const targetDir = 'D:\\Toolkits\\JoyManga'
const srcExe = path.join(rootDir, 'src-tauri', 'target', 'release', 'joymanga.exe')

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

// Clean up any installer files in targetDir to ensure strictly green portable edition
const existingSetupFiles = fs.readdirSync(targetDir).filter(f => f.endsWith('-setup.exe') || f.endsWith('.msi'))
for (const file of existingSetupFiles) {
  try {
    fs.unlinkSync(path.join(targetDir, file))
    console.log(`[copy-release] Removed leftover installer: ${file}`)
  } catch (err) {
    console.warn(`[copy-release] Failed to remove ${file}:`, err)
  }
}
