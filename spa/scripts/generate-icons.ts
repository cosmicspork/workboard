import { Buffer } from 'node:buffer';
import { readFileSync, writeFileSync } from 'node:fs';
import { Resvg } from '@resvg/resvg-js';

const svg = readFileSync(new URL('../public/workboard.svg', import.meta.url));

function render(size: number): Buffer {
  const image = new Resvg(svg, {
    fitTo: { mode: 'width', value: size },
    font: { loadSystemFonts: false }
  }).render();
  if (image.width !== size || image.height !== size) throw new Error(`Unexpected ${size}px icon dimensions`);
  return image.asPng();
}

const smallIcons = [16, 32].map(size => {
  const png = render(size);
  writeFileSync(new URL(`../public/favicon-${size}.png`, import.meta.url), png);
  return { size, png };
});
writeFileSync(new URL('../public/apple-touch-icon.png', import.meta.url), render(180));

// ICO directory entries can contain PNG images; browsers use the 16px or 32px entry.
const header = Buffer.alloc(6 + smallIcons.length * 16);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(smallIcons.length, 4);
let offset = header.length;
for (const [index, { size, png }] of smallIcons.entries()) {
  const entry = 6 + index * 16;
  header.writeUInt8(size, entry);
  header.writeUInt8(size, entry + 1);
  header.writeUInt16LE(1, entry + 4);
  header.writeUInt16LE(32, entry + 6);
  header.writeUInt32LE(png.length, entry + 8);
  header.writeUInt32LE(offset, entry + 12);
  offset += png.length;
}
writeFileSync(new URL('../public/favicon.ico', import.meta.url), Buffer.concat([header, ...smallIcons.map(icon => icon.png)]));
