import { spawn } from 'node:child_process'
import { copyFile, mkdir, realpath, lstat, unlink, readdir } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const compact = process.argv.includes('--test')
const debug = compact || process.argv.includes('--debug')
if (process.argv.slice(2).some((arg) => !['--debug', '--release', '--test'].includes(arg))) throw new Error('仅支持 --debug / --release / --test')
const env = { ...process.env }
if (process.platform === 'win32' && !env.ANDROID_HOME && env.LOCALAPPDATA) env.ANDROID_HOME = path.join(env.LOCALAPPDATA, 'Android', 'Sdk')
if (compact) {
  // 运行调试签名变体，但使用体积优化的 Rust 构建，避免发出数百 MB 的调试包。
  Object.assign(env, {
    CARGO_PROFILE_DEV_DEBUG: '0', CARGO_PROFILE_DEV_OPT_LEVEL: 's',
    CARGO_PROFILE_DEV_LTO: 'true', CARGO_PROFILE_DEV_CODEGEN_UNITS: '1',
    CARGO_PROFILE_DEV_STRIP: 'symbols',
  })
}

function run(program, args, cwd) {
  return new Promise((resolve, reject) => {
    let output = ''
    const child = spawn(program, args, { cwd, env, shell: process.platform === 'win32' && !/\.(exe|com)$/i.test(program), windowsHide: true, stdio: ['inherit', 'pipe', 'pipe'] })
    const record = (stream) => (chunk) => { stream.write(chunk); output = (output + chunk.toString('utf8')).slice(-4 * 1024 * 1024) }
    child.stdout.on('data', record(process.stdout)); child.stderr.on('data', record(process.stderr))
    child.on('error', reject); child.on('exit', (code) => resolve({ code: code ?? 1, output }))
  })
}

const args = ['tauri', 'android', 'build', ...(debug ? ['--debug'] : []), '--apk', '--target', 'aarch64']
const result = await run('pnpm', args, root)
if (result.code === 0 && !compact) process.exit(0)
const linkPermissionFailure = process.platform === 'win32' && result.output.includes('Creation symbolic link is not allowed for this system.') && result.output.includes('symlinking lib')
if (result.code !== 0 && !linkPermissionFailure) process.exit(result.code)

// 仅在 CLI 已完成 Rust 编译、失败点明确为链接权限时回退，不掩盖编译错误。
const profile = debug ? 'debug' : 'release'
const source = path.join(root, 'src-tauri', 'target', 'aarch64-linux-android', profile, 'libjoymanga_lib.so')
const targetDir = path.join(root, 'src-tauri', 'gen', 'android', 'app', 'src', 'main', 'jniLibs', 'arm64-v8a')
await mkdir(targetDir, { recursive: true })
const resolvedRoot = await realpath(root)
for (const candidate of [await realpath(source), await realpath(targetDir)]) {
  const relative = path.relative(resolvedRoot, candidate)
  if (relative.startsWith('..') || path.isAbsolute(relative)) throw new Error('构建路径离开项目目录，拒绝复制')
}
const target = path.join(targetDir, path.basename(source))
const stat = await lstat(target).catch((error) => { if (error.code === 'ENOENT') return null; throw error })
if (stat?.isSymbolicLink()) await unlink(target)
await copyFile(source, target)
if (compact) {
  // 测试包仍使用 debug 签名，只去除副本的 DWARF 信息；保留原调试库供诊断。
  let ndk = env.NDK_HOME || env.ANDROID_NDK_ROOT
  if (!ndk && env.ANDROID_HOME) {
    const ndkRoot = path.join(env.ANDROID_HOME, 'ndk')
    const versions = (await readdir(ndkRoot, { withFileTypes: true })).filter((entry) => entry.isDirectory() && /^\d+\.\d+\.\d+$/.test(entry.name)).map((entry) => entry.name)
    versions.sort((a, b) => b.localeCompare(a, undefined, { numeric: true }))
    if (versions.length) ndk = path.join(ndkRoot, versions[0])
  }
  if (!ndk) throw new Error('生成小体积测试包需要 NDK_HOME')
  const platform = process.platform === 'win32' ? 'windows-x86_64' : process.platform === 'darwin' ? 'darwin-x86_64' : 'linux-x86_64'
  const strip = path.join(ndk, 'toolchains', 'llvm', 'prebuilt', platform, 'bin', process.platform === 'win32' ? 'llvm-strip.exe' : 'llvm-strip')
  const stripped = await run(strip, ['--strip-all', target], root)
  if (stripped.code !== 0) process.exit(stripped.code)
}
console.log('Rust 编译已完成，使用已准备的本地文件打包 Android。')
const variant = debug ? 'Debug' : 'Release'
if (compact) {
  // 删除本脚本刚生成的 debug 构建输出，避免增量 ZIP 更新遗留旧数据空洞；不清理其他产物。
  const apk = path.join(root, 'src-tauri', 'gen', 'android', 'app', 'build', 'outputs', 'apk', 'arm64', 'debug', 'app-arm64-debug.apk')
  const previous = await lstat(apk).catch((error) => { if (error.code === 'ENOENT') return null; throw error })
  if (previous) {
    if (!previous.isFile() || previous.isSymbolicLink()) throw new Error('测试 APK 输出不是普通文件，拒绝替换')
    const resolved = await realpath(apk)
    if (path.relative(resolvedRoot, resolved).startsWith('..')) throw new Error('测试 APK 输出离开项目目录')
    await unlink(apk)
  }
}
const gradle = await run(process.platform === 'win32' ? 'gradlew.bat' : './gradlew', [':app:assembleArm64' + variant, '-x', 'rustBuildArm64' + variant, '-Pkotlin.incremental=false', '--no-daemon'], path.join(root, 'src-tauri', 'gen', 'android'))
if (compact && gradle.code === 0) console.log('小体积测试包已生成；debug 签名，不作为正式发布产物。')
process.exit(gradle.code)
