import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const rootDir = path.resolve(__dirname, '..')

// 1. 读取 package.json 获取当前版本
const packageJsonPath = path.join(rootDir, 'package.json')
const pkg = JSON.parse(fs.readFileSync(packageJsonPath, 'utf-8'))
const currentVersion = pkg.version

// 2. 解析目标版本号
const arg = (process.argv[2] || 'patch').trim().toLowerCase()
let targetVersion = ''

const [major, minor, patch] = currentVersion.split('.').map(n => parseInt(n, 10) || 0)

if (arg === 'patch') {
  targetVersion = `${major}.${minor}.${patch + 1}`
} else if (arg === 'minor') {
  targetVersion = `${major}.${minor + 1}.0`
} else if (arg === 'major') {
  targetVersion = `${major + 1}.0.0`
} else if (/^\d+\.\d+\.\d+/.test(arg)) {
  targetVersion = arg.replace(/^v/, '')
} else {
  console.error(`[bump-version] 无效的版本更新参数: ${arg}，请使用 patch, minor, major 或明确版本号如 0.2.2`)
  process.exit(1)
}

// 计算 Android versionCode: major * 10000 + minor * 100 + patch
const [tMajor, tMinor, tPatch] = targetVersion.split('.').map(n => parseInt(n, 10) || 0)
const targetVersionCode = tMajor * 10000 + tMinor * 100 + tPatch

console.log(`[bump-version] 开始同步升级版本: v${currentVersion} -> v${targetVersion} (Android versionCode: ${targetVersionCode})`)

// 3. 更新 package.json
pkg.version = targetVersion
fs.writeFileSync(packageJsonPath, JSON.stringify(pkg, null, 2) + '\n', 'utf-8')
console.log(`  ✓ 更新 package.json: ${targetVersion}`)

// 4. 更新 src-tauri/tauri.conf.json
const tauriConfPath = path.join(rootDir, 'src-tauri', 'tauri.conf.json')
if (fs.existsSync(tauriConfPath)) {
  const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, 'utf-8'))
  tauriConf.version = targetVersion
  fs.writeFileSync(tauriConfPath, JSON.stringify(tauriConf, null, 2) + '\n', 'utf-8')
  console.log(`  ✓ 更新 src-tauri/tauri.conf.json: ${targetVersion}`)
}

// 5. 更新 src-tauri/Cargo.toml
const cargoTomlPath = path.join(rootDir, 'src-tauri', 'Cargo.toml')
if (fs.existsSync(cargoTomlPath)) {
  let cargoContent = fs.readFileSync(cargoTomlPath, 'utf-8')
  cargoContent = cargoContent.replace(/^version = ".*?"/m, `version = "${targetVersion}"`)
  fs.writeFileSync(cargoTomlPath, cargoContent, 'utf-8')
  console.log(`  ✓ 更新 src-tauri/Cargo.toml: ${targetVersion}`)
}

// 6. 更新 src-tauri/gen/android/app/tauri.properties
const tauriPropPath = path.join(rootDir, 'src-tauri', 'gen', 'android', 'app', 'tauri.properties')
if (fs.existsSync(tauriPropPath)) {
  let propContent = fs.readFileSync(tauriPropPath, 'utf-8')
  propContent = propContent.replace(/tauri\.android\.versionName=.*/, `tauri.android.versionName=${targetVersion}`)
  propContent = propContent.replace(/tauri\.android\.versionCode=.*/, `tauri.android.versionCode=${targetVersionCode}`)
  fs.writeFileSync(tauriPropPath, propContent, 'utf-8')
  console.log(`  ✓ 更新 src-tauri/gen/android/app/tauri.properties: versionName=${targetVersion}, versionCode=${targetVersionCode}`)
}

// 7. 更新 README.md 中的版本号和包名称
const readmePath = path.join(rootDir, 'README.md')
if (fs.existsSync(readmePath)) {
  let readme = fs.readFileSync(readmePath, 'utf-8')
  // 替换形如 Version-v0.2.1-ff69b4.svg
  readme = readme.replace(new RegExp(`Version-v${currentVersion.replace(/\./g, '\\.')}-`, 'g'), `Version-v${targetVersion}-`)
  // 替换形如 _v0.2.1_ 或 _0.2.1_
  readme = readme.replace(new RegExp(`_v${currentVersion.replace(/\./g, '\\.')}_`, 'g'), `_v${targetVersion}_`)
  readme = readme.replace(new RegExp(`_${currentVersion.replace(/\./g, '\\.')}_`, 'g'), `_${targetVersion}_`)
  // 替换形如 （v0.2.1）
  readme = readme.replace(new RegExp(`（v${currentVersion.replace(/\./g, '\\.')}）`, 'g'), `（v${targetVersion}）`)
  readme = readme.replace(new RegExp(`\\(v${currentVersion.replace(/\./g, '\\.')}\\)`, 'g'), `(v${targetVersion})`)
  readme = readme.replace(new RegExp(`alt="Version ${currentVersion.replace(/\./g, '\\.')}"`, 'g'), `alt="Version ${targetVersion}"`)
  fs.writeFileSync(readmePath, readme, 'utf-8')
  console.log(`  ✓ 更新 README.md`)
}

console.log(`\n🎉 全局版本已成功同步至 v${targetVersion}！`)
