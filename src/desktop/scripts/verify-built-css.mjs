import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const assetsDir = path.resolve(desktopDir, '../desktop-dist/assets')
const cssFiles = fs.readdirSync(assetsDir).filter((name) => /^index-.*\.css$/.test(name))

if (cssFiles.length !== 1) {
  throw new Error(`Expected one main CSS bundle, found ${cssFiles.length}`)
}

const cssPath = path.join(assetsDir, cssFiles[0])
const css = fs.readFileSync(cssPath, 'utf8')
const requiredUtilities = ['.flex{', '.grid{', '.h-screen{', '.items-center{', '.p-4{']
const missing = requiredUtilities.filter((utility) => !css.includes(utility))

if (missing.length > 0) {
  throw new Error(`Tailwind output is incomplete; missing ${missing.join(', ')}`)
}

console.log(`[verify-built-css] OK ${path.basename(cssPath)} (${css.length} bytes)`)
