import sharp from 'sharp';
import { mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <rect x="40" y="40" width="944" height="944" rx="220" fill="#35D0BA"/>
  <path d="M512 236 L788 512 L512 788 L236 512 Z" fill="#06332C"/>
</svg>`;

mkdirSync(new URL('./out/', import.meta.url), { recursive: true });
await sharp(Buffer.from(svg)).png().toFile(fileURLToPath(new URL('./out/icon-1024.png', import.meta.url)));
console.log('wrote tools/covers/out/icon-1024.png');
