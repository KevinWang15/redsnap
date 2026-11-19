// Packages RedSnap Settings for one platform into out/, for placing next to
// RedSnap's executable (as `settings/`; on macOS inside the app bundle).
//   node scripts/package.mjs [win32|linux|darwin] [x64|arm64]
// The renderer must be built first (npm run build).
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { packager } from '@electron/packager'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const platform = process.argv[2] ?? process.platform
const arch = process.argv[3] ?? 'x64'
if (!fs.existsSync(path.join(root, 'dist', 'index.html'))) {
  console.error('Build the renderer first: npm run build')
  process.exit(1)
}

const paths = await packager({
  dir: root,
  name: 'RedSnap Settings',
  executableName: platform === 'linux' ? 'redsnap-settings' : 'RedSnap Settings',
  appBundleId: 'com.kevinwang15.redsnap.settings',
  platform,
  arch,
  out: path.join(root, 'out'),
  overwrite: true,
  asar: true,
  prune: true,
  // Only the main process and the built renderer ship.
  ignore: [/^\/node_modules($|\/)/, /^\/\./, /^\/src($|\/)/, /^\/scripts($|\/)/, /^\/out($|\/)/, /^\/public($|\/)/, /\.(ts|tsx|md)$/, /^\/tsconfig/, /^\/vite\.config/, /^\/index\.html$/],
  win32metadata: {
    CompanyName: 'Ke Wang',
    FileDescription: 'RedSnap Settings',
    ProductName: 'RedSnap Settings',
    InternalName: 'RedSnap Settings',
  },
})
// Chromium UI strings: English and Simplified Chinese only (about 45 MB less).
const KEEP = new Set(['en-US.pak', 'zh-CN.pak'])
for (const dir of paths) {
  const locales = path.join(dir, 'locales')
  if (!fs.existsSync(locales)) continue
  for (const f of fs.readdirSync(locales)) if (!KEEP.has(f)) fs.rmSync(path.join(locales, f))
}
console.log(paths.join('\n'))
