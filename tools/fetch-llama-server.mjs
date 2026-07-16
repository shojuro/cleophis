// Downloads the latest llama.cpp Windows Vulkan x64 release zip and extracts
// llama-server.exe + DLLs into src-tauri/resources/llama/. Run with WSL node from tools/.
//
// Adaptation: the reference implementation shells out to `unzip -j`, but the
// executor WSL environment has no `unzip` binary and no passwordless sudo to
// install one. Extraction is done via `python3 -m zipfile` (stdlib, always
// present) piped through a small inline script that flattens paths the same
// way `unzip -j` would and keeps only llama-server.exe + *.dll, matching the
// original glob intent.
import { execSync } from 'node:child_process';
import { writeFileSync, mkdirSync } from 'node:fs';

const rel = await (await fetch('https://api.github.com/repos/ggml-org/llama.cpp/releases/latest', {
  headers: { 'User-Agent': 'cleophis-build' },
})).json();
const assets = rel.assets.map(a => a.name);
const asset = rel.assets.find(a => /win/i.test(a.name) && /vulkan/i.test(a.name) && /x64/i.test(a.name) && a.name.endsWith('.zip'));
if (!asset) {
  console.error('No win/vulkan/x64 zip found. Assets were:\n' + assets.join('\n'));
  process.exit(1);
}
console.log('downloading', asset.name, `(${(asset.size / 1e6).toFixed(0)} MB)`);
const buf = Buffer.from(await (await fetch(asset.browser_download_url)).arrayBuffer());
writeFileSync('/tmp/llama-server.zip', buf);
mkdirSync('../src-tauri/resources/llama', { recursive: true });
try {
  execSync(`unzip -o -j /tmp/llama-server.zip 'llama-server.exe' '*.dll' -d ../src-tauri/resources/llama/`, { stdio: 'inherit' });
} catch (e) {
  // Fallback: no `unzip` on this host. Extract with python3's stdlib zipfile
  // module instead, flattening paths and filtering to the same file set.
  console.log('unzip unavailable, falling back to python3 zipfile');
  const py = `
import zipfile, os
z = zipfile.ZipFile('/tmp/llama-server.zip')
dest = '../src-tauri/resources/llama/'
for name in z.namelist():
    base = os.path.basename(name)
    if base == 'llama-server.exe' or base.endswith('.dll'):
        with z.open(name) as src, open(os.path.join(dest, base), 'wb') as out:
            out.write(src.read())
        print('extracted', base)
`;
  execSync(`python3 -c "${py.replace(/"/g, '\\"')}"`, { stdio: 'inherit' });
}
console.log('done');
