// Generates app-icon.png (1024x1024), the source for `npm run tauri icon`.
// Placeholder branding: rounded navy tile with a teal database cylinder.
// Pure Node — no dependencies.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 1024;
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "..", "app-icon.png");

// ---------- tiny PNG encoder ----------
const CRC_TABLE = new Int32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  CRC_TABLE[n] = c;
}

function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function encodePng(rgba, width, height) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 4 + 1)] = 0; // filter: none
    rgba.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---------- drawing ----------
const px = Buffer.alloc(SIZE * SIZE * 4);

function put(x, y, [r, g, b], a) {
  if (a <= 0) return;
  const i = (y * SIZE + x) * 4;
  const ia = 1 - a;
  px[i] = Math.round(px[i] * ia + r * a);
  px[i + 1] = Math.round(px[i + 1] * ia + g * a);
  px[i + 2] = Math.round(px[i + 2] * ia + b * a);
  px[i + 3] = Math.min(255, px[i + 3] + Math.round(a * 255));
}

// signed distance to a rounded box centered in the canvas
function sdRoundedBox(x, y, halfW, halfH, radius) {
  const qx = Math.abs(x - 512) - halfW + radius;
  const qy = Math.abs(y - 512) - halfH + radius;
  const outside = Math.hypot(Math.max(qx, 0), Math.max(qy, 0));
  const inside = Math.min(Math.max(qx, qy), 0);
  return outside + inside - radius;
}

// normalized ellipse distance (<0 inside), scaled for antialiasing
function ellipseDist(x, y, cx, cy, rx, ry) {
  const dx = (x - cx) / rx;
  const dy = (y - cy) / ry;
  return (Math.hypot(dx, dy) - 1) * Math.min(rx, ry);
}

const BG_TOP = [15, 23, 42]; // slate-900
const BG_BOTTOM = [30, 41, 82]; // indigo-ish navy
const FACE = [94, 234, 212]; // teal-300
const BODY = [20, 184, 166]; // teal-500
const LINE = [13, 111, 100]; // darker teal separators

const CX = 512;
const TOP_CY = 336;
const BOTTOM_CY = 704;
const RX = 232;
const RY = 80;

for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const tile = sdRoundedBox(x + 0.5, y + 0.5, 502, 502, 180);
    const tileA = Math.min(Math.max(0.5 - tile, 0), 1);
    if (tileA <= 0) continue;

    const t = y / SIZE;
    const bg = BG_TOP.map((c, i) => Math.round(c + (BG_BOTTOM[i] - c) * t));
    put(x, y, bg, tileA);
    if (tile > 0.5) continue; // outside the tile: nothing else draws

    const inBody =
      x >= CX - RX &&
      x <= CX + RX &&
      y >= TOP_CY &&
      y <= BOTTOM_CY &&
      ellipseDist(x, y, CX, BOTTOM_CY, RX, RY) <= 0;
    const topFace = ellipseDist(x, y, CX, TOP_CY, RX, RY);
    const bottomCap = ellipseDist(x, y, CX, BOTTOM_CY, RX, RY);

    if (topFace <= 0) {
      put(x, y, FACE, Math.min(1, -topFace + 0.5));
    } else if (inBody) {
      // separator rings where the body crosses "shelf" ellipses
      const ring1 = Math.abs(ellipseDist(x, y, CX, 458, RX, RY));
      const ring2 = Math.abs(ellipseDist(x, y, CX, 581, RX, RY));
      const color = ring1 < 4 || ring2 < 4 ? LINE : BODY;
      put(x, y, color, 1);
    } else if (bottomCap <= 0 && y >= BOTTOM_CY) {
      put(x, y, BODY, Math.min(1, -bottomCap + 0.5));
    }
  }
}

writeFileSync(OUT, encodePng(px, SIZE, SIZE));
console.log(`wrote ${OUT}`);
