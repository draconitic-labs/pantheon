// Pantheon 3D: fetch and decode one image off the main thread, cut into tiles. WebKitGTK
// decodes on the calling thread, and an 8K JPEG there freezes a flight for most of a
// second; the tiles let the GPU upload spread over several frames as well.
const TILE_W = 2048, TILE_H = 1024;

self.onmessage = async (e) => {
  const { id, url, maxSize } = e.data;
  try {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const blob = await res.blob();
    const raw = { premultiplyAlpha: 'none', colorSpaceConversion: 'none' };
    let bmp = await createImageBitmap(blob, { imageOrientation: 'flipY', ...raw });
    if (bmp.width > maxSize) {
      const small = await createImageBitmap(bmp, {
        resizeWidth: maxSize, resizeHeight: Math.round(bmp.height * maxSize / bmp.width), resizeQuality: 'high', ...raw,
      });
      bmp.close();
      bmp = small;
    }
    const { width: w, height: h } = bmp;
    const tiles = [];
    for (let y = 0; y < h; y += TILE_H) {
      for (let x = 0; x < w; x += TILE_W) {
        const tw = Math.min(TILE_W, w - x), th = Math.min(TILE_H, h - y);
        tiles.push({ x, y, w: tw, h: th, bmp: await createImageBitmap(bmp, x, y, tw, th, raw) });
      }
    }
    bmp.close();
    self.postMessage({ id, w, h, tiles }, tiles.map(t => t.bmp));
  } catch (err) {
    self.postMessage({ id, error: String(err && err.message || err) });
  }
};
