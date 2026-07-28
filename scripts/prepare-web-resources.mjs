import { cp, mkdir, readdir, rm } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const appDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const distDir = path.join(appDir, 'dist')
const webResourcesDir = path.join(appDir, 'src-tauri', 'resources', 'web')

const distEntries = await readdir(distDir)
if (!distEntries.includes('index.html')) {
  throw new Error(`Web build output is missing ${path.join(distDir, 'index.html')}`)
}

await mkdir(webResourcesDir, { recursive: true })
for (const entry of await readdir(webResourcesDir)) {
  if (entry !== '.gitignore') {
    await rm(path.join(webResourcesDir, entry), { recursive: true, force: true })
  }
}

await cp(distDir, webResourcesDir, { recursive: true })
console.log(`Prepared WebUI resources in ${webResourcesDir}`)
