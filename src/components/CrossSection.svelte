<script lang="ts">
  /**
   * Single cross-section canvas for a CPR needle position.
   *
   * If `batchImageData` is provided (from parent batch computation as raw
   * Float32Array), renders it directly. Otherwise falls back to invoking
   * Rust individually (legacy base64 path).
   */
  import { invoke } from '@tauri-apps/api/core';

  type Props = {
    centerlineMm: [number, number, number][];
    positionFraction: number;
    rotationDeg: number;
    label: string;
    color: string;
    windowCenter?: number;
    windowWidth?: number;
    /** Pre-computed raw f32 image data from batch call. */
    batchImageData?: Float32Array | null;
    /** Pixel size of the batch image. */
    batchPixels?: number | null;
    /** Pre-computed arc-length in mm from batch call. */
    arcMmProp?: number | null;
    /** Whether to show FAI color overlay. */
    showFaiOverlay?: boolean;
  };

  let {
    centerlineMm,
    positionFraction,
    rotationDeg,
    label,
    color,
    windowCenter = 40,
    windowWidth = 400,
    batchImageData = null,
    batchPixels = null,
    arcMmProp = null,
    showFaiOverlay = false,
  }: Props = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let arcMm = $state<number | null>(null);
  let loading = $state(false);
  let pixels = $state(128);
  let vesselDiameterMm = $state<number | null>(null);
  // Measurement endpoints in pixel coords for visual overlay
  let measH = $state<{ left: number; right: number; y: number } | null>(null);
  let measV = $state<{ top: number; bottom: number; x: number } | null>(null);
  /**
   * 16-point polygon of the detected lumen boundary. Stored in canvas pixel
   * coords. Null when measurement is rejected.
   */
  let lumenContour = $state<Array<{ x: number; y: number }> | null>(null);

  type CrossSectionResult = {
    image_base64: string;
    pixels: number;
    arc_mm: number;
  };

  function decodeBase64Float32(b64: string): Float32Array {
    const binary = atob(b64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return new Float32Array(bytes.buffer);
  }

  function renderToCanvas(
    cvs: HTMLCanvasElement,
    data: Float32Array,
    w: number,
    h: number,
    wc: number,
    ww: number,
  ) {
    cvs.width = w;
    cvs.height = h;
    const ctx = cvs.getContext('2d')!;
    const imgData = ctx.createImageData(w, h);
    const lo = wc - ww / 2;
    const hi = wc + ww / 2;
    for (let i = 0; i < data.length; i++) {
      const raw = data[i];
      const gray = Math.max(0, Math.min(255, Math.round(((raw - lo) / (hi - lo)) * 255)));
      if (showFaiOverlay && raw >= -190 && raw <= -30) {
        const t = (raw - (-190)) / ((-30) - (-190));
        const r = Math.round(t < 0.5 ? t * 2 * 255 : 255);
        const g = Math.round(t < 0.5 ? 255 : (1 - (t - 0.5) * 2) * 255);
        imgData.data[i * 4]     = r;
        imgData.data[i * 4 + 1] = g;
        imgData.data[i * 4 + 2] = 20;
      } else {
        imgData.data[i * 4] = gray;
        imgData.data[i * 4 + 1] = gray;
        imgData.data[i * 4 + 2] = gray;
      }
      imgData.data[i * 4 + 3] = 255;
    }
    ctx.putImageData(imgData, 0, 0);
  }

  /**
   * Region-grow lumen detection + area-equivalent diameter.
   *
   * The input is a monoenergetic keV reconstruction, so absolute HU
   * cut-offs are unusable — the same anatomy reads hundreds of HU
   * differently at 40 keV vs 180 keV. Everything here is expressed in
   * units of "distance from outer-ring background, measured in outer-
   * ring noise σ", so the algorithm behaves identically at any keV the
   * user reconstructs at.
   *
   *   1. Anchor on the brightest pixel in the central ~3 mm window.
   *   2. Characterise background from the outer image ring: median gives
   *      bgHU, 1.4826·MAD gives a Gaussian-consistent noise σ.
   *   3. Contrast check: require peak − bg ≥ 5 σ. This is the keV-
   *      invariant replacement for "peak must be ≥ 120 HU". If the
   *      slice doesn't have real vessel enhancement we refuse to
   *      measure.
   *   4. Half-max threshold = bg + 0.55·(peak − bg). Pure ratio — no
   *      HU constants. The slight asymmetry above 0.5 keeps thin
   *      myocardial rims out of the mask at all keV levels.
   *   5. 4-connected BFS from the peak, capped at 4 mm radial distance
   *      (coronary lumen almost never > 4 mm radius in cross-section).
   *   6. Morphological opening (erode-dilate) of the blob. Erosion
   *      severs 1–2-pixel-wide bridges that leak the region-grow into
   *      adjacent enhanced myocardium; a re-BFS keeps only the peak's
   *      component; dilation restores the true lumen size.
   *   7. Diameter = 2 · √(area / π). Area-equivalent diameter is
   *      naturally immune to any single ray leaking — it's driven by
   *      total blob area.
   *   8. Sanity-clip to 1–6 mm (a physical constraint on coronary
   *      arteries, not an HU one — so it's keV-invariant). Outside
   *      that, return null.
   *
   * Why region growing? Connectivity excludes distant bright blobs
   * (calcified plaque, an adjacent artery, chamber blood two mm away)
   * that a plain threshold would include.
   *
   * Conceptually this is the 2-D-per-slice simplification of VMTK's
   * colliding-fronts level set; we skip the PDE solve because the
   * cross-section geometry is planar and area is all we need.
   */
  function measureVesselDiameter(data: Float32Array, sz: number): number | null {
    const widthMm = 15.0;
    const mmPerPixel = (2 * widthMm) / sz;
    const center = Math.floor(sz / 2);

    const reject = (): null => {
      measH = null;
      measV = null;
      lumenContour = null;
      return null;
    };

    // --- 1. Peak search across the whole crop, with a gentle centre bias ---
    // A cubic-spline centerline drifts off the true lumen between manually
    // placed seeds; on tightly curved segments the drift can be 6–8 mm.
    // A small search window around the image centre will miss a visibly
    // bright lumen entirely and we reject a vessel the user can clearly
    // see. Conversely, naively picking the single brightest pixel in the
    // FOV would latch onto a neighbouring artery or partial-volume
    // chamber when both are present.
    //
    // Compromise: score every pixel as (HU − k · distance_from_centre_mm)
    // with k = 15 HU/mm. A 500 HU pixel at the image centre scores 500;
    // at 8 mm offset it still scores 380 and beats a 250 HU tissue pixel
    // at centre. If two bright coronaries both appear in the 30 mm crop,
    // the one closer to the centerline wins — that's the one the user
    // seeded. Cheap: ~16 k pixels per call, sub-millisecond.
    const CENTER_BIAS_HU_PER_MM = 15.0;
    let peakHU = -Infinity;
    let peakScore = -Infinity;
    let peakRow = center;
    let peakCol = center;
    for (let r = 0; r < sz; r++) {
      const dr = r - center;
      const dr2 = dr * dr;
      for (let c = 0; c < sz; c++) {
        const v = data[r * sz + c];
        if (!Number.isFinite(v)) continue;
        const dc = c - center;
        const distMm = Math.sqrt(dr2 + dc * dc) * mmPerPixel;
        const score = v - CENTER_BIAS_HU_PER_MM * distMm;
        if (score > peakScore) {
          peakScore = score;
          peakHU = v;
          peakRow = r;
          peakCol = c;
        }
      }
    }
    if (!Number.isFinite(peakHU)) return reject();

    // --- 2. Background from annulus AROUND THE PEAK (keV-invariant) ---
    // Absolute HU cut-offs are unusable because the input is a
    // monoenergetic reconstruction: at 40 keV lumen is 800+ HU and
    // myocardium 250 HU; at 180 keV the same anatomy reads ~120 and ~60.
    // A "200 HU floor" works only at ~70 keV / 120 kVp and is wrong
    // elsewhere. Instead characterise the *local* HU distribution.
    //
    // The old version sampled the outer image border. That's poisoned by
    // partial-volume lung, air, or an adjacent vessel at the crop edge —
    // MAD explodes, the contrast test fails, and we reject a perfectly
    // good slice. Sampling an annulus *around the peak* (not the image
    // centre) lands squarely in the perivascular tissue (fat / muscle)
    // regardless of where the lumen sits in the crop.
    //
    // Annulus 5–8 mm from peak: outside any plausible coronary lumen,
    // inside the typical myocardial/fat neighbourhood.
    const annulusInnerPx = Math.max(3, Math.round(5.0 / mmPerPixel));
    const annulusOuterPx = Math.max(annulusInnerPx + 2, Math.round(8.0 / mmPerPixel));
    const annulusInnerSq = annulusInnerPx * annulusInnerPx;
    const annulusOuterSq = annulusOuterPx * annulusOuterPx;
    const annulus: number[] = [];
    const ar0 = Math.max(0, peakRow - annulusOuterPx);
    const ar1 = Math.min(sz - 1, peakRow + annulusOuterPx);
    const ac0 = Math.max(0, peakCol - annulusOuterPx);
    const ac1 = Math.min(sz - 1, peakCol + annulusOuterPx);
    for (let r = ar0; r <= ar1; r++) {
      const dr = r - peakRow;
      const dr2 = dr * dr;
      for (let c = ac0; c <= ac1; c++) {
        const dc = c - peakCol;
        const dsq = dr2 + dc * dc;
        if (dsq < annulusInnerSq || dsq > annulusOuterSq) continue;
        const v = data[r * sz + c];
        if (Number.isFinite(v)) annulus.push(v);
      }
    }
    if (annulus.length < 20) return reject();
    annulus.sort((a, b) => a - b);
    const bgHU = annulus[Math.floor(annulus.length / 2)];
    // 1.4826·MAD is the Gaussian-consistent σ estimator; floor at 10 HU
    // so super-clean images don't set an absurdly small noise level.
    const absDev = annulus.map((v) => Math.abs(v - bgHU)).sort((a, b) => a - b);
    const bgMad = absDev[Math.floor(absDev.length / 2)];
    const bgNoise = Math.max(bgMad * 1.4826, 10);

    // --- 3. Contrast check (keV-invariant) ---
    // Require the lumen peak to stand at least 3 σ above background.
    // 3 σ means the peak is very unlikely to be tissue noise (≈ 99.7 %
    // confidence for Gaussian noise). At 40 keV the contrast is hundreds
    // of HU, at 180 keV only dozens, and both clear a 3 σ bar when real
    // vessel enhancement is present. Below 3 σ the "peak" is probably
    // just the local tissue's loudest pixel.
    const contrast = peakHU - bgHU;
    if (contrast < 3 * bgNoise) return reject();

    // --- 4. Pure half-max threshold ---
    // Classic FWHM: boundary at the midpoint between lumen peak and
    // background. No HU constants — the threshold tracks keV.
    //
    // We previously used 0.55 × contrast (slightly above 0.5) to keep
    // enhanced myocardium out of the mask, but that was too strict for
    // small lumens (1.5–2 mm): partial-volume smoothing gives them a
    // Gaussian-ish profile, so a 0.55 threshold only captures a 3-pixel
    // core and the area falls below the minAreaPx cutoff. The
    // morphological opening below already defends against muscle
    // leakage, so we can keep the threshold at the canonical 0.5.
    const halfMax = bgHU + 0.5 * contrast;

    // --- 4. 4-connected region growing from the peak ---
    const MAX_RADIUS_MM = 4.0;
    const maxRadiusPx = Math.min(sz / 2 - 1, Math.ceil(MAX_RADIUS_MM / mmPerPixel));
    const maxRadiusSqPx = maxRadiusPx * maxRadiusPx;

    const mask = new Uint8Array(sz * sz);
    const visited = new Uint8Array(sz * sz);
    const seedIdx = peakRow * sz + peakCol;
    // Index-based circular buffer beats `Array.shift()` on long BFS.
    const queue = new Int32Array(sz * sz);
    let qHead = 0;
    let qTail = 0;
    queue[qTail++] = seedIdx;
    visited[seedIdx] = 1;
    let maskArea = 0;

    while (qHead < qTail) {
      const idx = queue[qHead++];
      const r = (idx / sz) | 0;
      const c = idx - r * sz;
      const dr = r - peakRow;
      const dc = c - peakCol;
      if (dr * dr + dc * dc > maxRadiusSqPx) continue;
      const v = data[idx];
      if (!Number.isFinite(v) || v < halfMax) continue;

      mask[idx] = 1;
      maskArea++;

      // 4-connected neighbours
      if (r > 0) {
        const n = idx - sz;
        if (!visited[n]) { visited[n] = 1; queue[qTail++] = n; }
      }
      if (r < sz - 1) {
        const n = idx + sz;
        if (!visited[n]) { visited[n] = 1; queue[qTail++] = n; }
      }
      if (c > 0) {
        const n = idx - 1;
        if (!visited[n]) { visited[n] = 1; queue[qTail++] = n; }
      }
      if (c < sz - 1) {
        const n = idx + 1;
        if (!visited[n]) { visited[n] = 1; queue[qTail++] = n; }
      }
    }

    // At least ~0.5 mm² of connected bright region (≈ 10 pixels at 0.23
    // mm/pixel) is required. Smaller blobs are noise or a stray bright
    // pixel, not a lumen.
    const minAreaPx = Math.max(10, Math.ceil(0.5 / (mmPerPixel * mmPerPixel)));
    if (maskArea < minAreaPx) return reject();

    // --- 4a. Morphological opening (erode then dilate, 4-connected) ---
    // Even with a stricter half-max, a narrow bridge (1–2 pixels wide)
    // between the lumen and adjacent enhanced myocardium can still slip
    // through. Opening severs these bridges: erosion removes boundary
    // pixels (thin bridges disappear first), then we re-extract the
    // connected component containing the peak, then dilate it back. The
    // final mask is the lumen without any thin attached protrusions.
    const eroded = new Uint8Array(sz * sz);
    for (let r = 1; r < sz - 1; r++) {
      const base = r * sz;
      for (let c = 1; c < sz - 1; c++) {
        const i = base + c;
        if (mask[i] && mask[i - 1] && mask[i + 1]
            && mask[i - sz] && mask[i + sz]) {
          eroded[i] = 1;
        }
      }
    }

    // If the peak itself didn't survive erosion, the blob is so thin that
    // the whole thing is effectively a ribbon — not a lumen. Reject.
    if (!eroded[seedIdx]) return reject();

    // BFS on the eroded mask from the peak to isolate *its* connected
    // component. Opening can fragment the original mask into disjoint
    // pieces (main lumen + severed myocardial blob); we keep only the
    // piece containing the peak.
    const peakComp = new Uint8Array(sz * sz);
    qHead = 0;
    qTail = 0;
    queue[qTail++] = seedIdx;
    peakComp[seedIdx] = 1;
    while (qHead < qTail) {
      const idx = queue[qHead++];
      const r = (idx / sz) | 0;
      const c = idx - r * sz;
      if (r > 0) {
        const n = idx - sz;
        if (eroded[n] && !peakComp[n]) { peakComp[n] = 1; queue[qTail++] = n; }
      }
      if (r < sz - 1) {
        const n = idx + sz;
        if (eroded[n] && !peakComp[n]) { peakComp[n] = 1; queue[qTail++] = n; }
      }
      if (c > 0) {
        const n = idx - 1;
        if (eroded[n] && !peakComp[n]) { peakComp[n] = 1; queue[qTail++] = n; }
      }
      if (c < sz - 1) {
        const n = idx + 1;
        if (eroded[n] && !peakComp[n]) { peakComp[n] = 1; queue[qTail++] = n; }
      }
    }

    // Dilate the peak-component back by 1 pixel to restore the lumen to
    // roughly its original size (minus any severed bridges).
    const finalMask = new Uint8Array(sz * sz);
    let finalArea = 0;
    for (let r = 0; r < sz; r++) {
      const base = r * sz;
      for (let c = 0; c < sz; c++) {
        const i = base + c;
        if (peakComp[i]) {
          if (!finalMask[i]) { finalMask[i] = 1; finalArea++; }
          if (r > 0 && !finalMask[i - sz]) { finalMask[i - sz] = 1; finalArea++; }
          if (r < sz - 1 && !finalMask[i + sz]) { finalMask[i + sz] = 1; finalArea++; }
          if (c > 0 && !finalMask[i - 1]) { finalMask[i - 1] = 1; finalArea++; }
          if (c < sz - 1 && !finalMask[i + 1]) { finalMask[i + 1] = 1; finalArea++; }
        }
      }
    }

    // Reassign mask to the opened mask so the contour scan below picks
    // this up too.
    for (let i = 0; i < sz * sz; i++) mask[i] = finalMask[i];
    maskArea = finalArea;
    if (maskArea < minAreaPx) return reject();

    // --- 5. Area-equivalent diameter ---
    const areaMmSq = maskArea * mmPerPixel * mmPerPixel;
    const diameterMm = 2 * Math.sqrt(areaMmSq / Math.PI);
    if (diameterMm < 1.0 || diameterMm > 6.0) return reject();

    // --- 6. Extract contour: radial scan of the binary mask at 32 angles ---
    // Scanning the mask (not the HU data) guarantees the contour wraps the
    // connected blob and never jumps to a distant bright spot. 32 rays
    // gives a visually smooth polygon without noticeable faceting.
    const N_RAYS = 32;
    const boundaryPts: Array<{ x: number; y: number }> = new Array(N_RAYS);
    const radiiPx: number[] = new Array(N_RAYS);
    for (let k = 0; k < N_RAYS; k++) {
      const theta = (k / N_RAYS) * 2 * Math.PI;
      const dx = Math.cos(theta);
      const dy = Math.sin(theta);
      let lastInMask = 0;
      for (let r = 0.5; r <= maxRadiusPx + 0.5; r += 0.5) {
        const xi = Math.round(peakCol + r * dx);
        const yi = Math.round(peakRow + r * dy);
        if (xi < 0 || xi >= sz || yi < 0 || yi >= sz) break;
        if (mask[yi * sz + xi]) {
          lastInMask = r;
        } else {
          break;
        }
      }
      radiiPx[k] = lastInMask;
      boundaryPts[k] = {
        x: peakCol + lastInMask * dx,
        y: peakRow + lastInMask * dy,
      };
    }

    // --- 7. H / V calipers for legacy compatibility ---
    // Index 0 is +x (right), N/4 is +y (down), N/2 is -x (left), 3N/4 is
    // -y (up) in canvas coords.
    const qtr = N_RAYS / 4;
    measH = {
      left: peakCol - radiiPx[N_RAYS / 2],
      right: peakCol + radiiPx[0],
      y: peakRow,
    };
    measV = {
      top: peakRow - radiiPx[3 * qtr],
      bottom: peakRow + radiiPx[qtr],
      x: peakCol,
    };
    lumenContour = boundaryPts;

    return diameterMm;
  }

  // --- Render pre-computed batch data when provided ---
  $effect(() => {
    const imgData = batchImageData;
    const sz = batchPixels;
    const am = arcMmProp;
    const wc = windowCenter;
    const ww = windowWidth;

    if (imgData && sz && canvas) {
      pixels = sz;
      arcMm = am;
      renderToCanvas(canvas, imgData, sz, sz, wc, ww);
      vesselDiameterMm = measureVesselDiameter(imgData, sz);
    }
  });

  // --- Fallback: invoke individually if no batch data (legacy base64 path) ---
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    // Only self-invoke if no batch data provided
    if (batchImageData) return;

    const cl = centerlineMm;
    const pf = positionFraction;
    const rd = rotationDeg;

    if (!cl || cl.length < 2) return;

    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(async () => {
      loading = true;
      try {
        const centerlineZyx = cl.map(([x, y, z]) => [z, y, x] as [number, number, number]);

        const result = await invoke<CrossSectionResult>(
          'compute_cross_section_image',
          {
            centerlineMm: centerlineZyx,
            positionFraction: pf,
            rotationDeg: rd,
            widthMm: 15.0,
            pixels: 128,
          },
        );

        pixels = result.pixels;
        arcMm = result.arc_mm;

        if (canvas) {
          const data = decodeBase64Float32(result.image_base64);
          renderToCanvas(canvas, data, pixels, pixels, windowCenter, windowWidth);
        }
      } catch (e) {
        console.error(`CrossSection ${label}: computation failed`, e);
      } finally {
        loading = false;
      }
    }, 150);

    return () => clearTimeout(debounceTimer);
  });
</script>

<div class="relative flex flex-col items-center" style="min-height: 0; flex: 1;">
  <!-- Label badge -->
  <div
    class="absolute left-1.5 top-1.5 z-10 flex items-center gap-1"
  >
    <span
      class="inline-block h-3 w-3 rounded-sm text-center text-[10px] font-bold leading-3"
      style="background-color: {color}; color: #000;"
    >
      {label}
    </span>
    {#if arcMm !== null}
      <span class="text-[10px] tabular-nums text-text-secondary">
        {arcMm.toFixed(1)} mm
      </span>
    {/if}
  </div>

  <!-- Loading indicator -->
  {#if loading}
    <div class="absolute inset-0 z-20 flex items-center justify-center bg-black/40">
      <span class="text-[10px] text-text-secondary">...</span>
    </div>
  {/if}

  <!-- Canvas -->
  <canvas
    bind:this={canvas}
    class="h-full w-full object-contain"
    width={pixels}
    height={pixels}
    style="image-rendering: pixelated;"
  ></canvas>

  <!-- Diameter measurement overlay -->
  {#if measH && measV && vesselDiameterMm !== null}
    <svg class="pointer-events-none absolute inset-0 h-full w-full" viewBox="0 0 {pixels} {pixels}" preserveAspectRatio="xMidYMid meet">
      <!-- 16-ray lumen boundary (transparent fill + thin outline) -->
      {#if lumenContour && lumenContour.length >= 3}
        <polygon
          points={lumenContour.map((p) => `${p.x.toFixed(2)},${p.y.toFixed(2)}`).join(' ')}
          fill="#22d3ee"
          fill-opacity="0.1"
          stroke="#22d3ee"
          stroke-width="0.8"
          stroke-opacity="0.8"
        />
      {/if}

      <!-- Horizontal caliper -->
      <line x1={measH.left} y1={measH.y} x2={measH.right} y2={measH.y}
        stroke="#facc15" stroke-width="1" stroke-opacity="0.9" />
      <line x1={measH.left} y1={measH.y - 3} x2={measH.left} y2={measH.y + 3}
        stroke="#facc15" stroke-width="1" stroke-opacity="0.9" />
      <line x1={measH.right} y1={measH.y - 3} x2={measH.right} y2={measH.y + 3}
        stroke="#facc15" stroke-width="1" stroke-opacity="0.9" />

      <!-- Vertical caliper -->
      <line x1={measV.x} y1={measV.top} x2={measV.x} y2={measV.bottom}
        stroke="#facc15" stroke-width="1" stroke-opacity="0.9" />
      <line x1={measV.x - 3} y1={measV.top} x2={measV.x + 3} y2={measV.top}
        stroke="#facc15" stroke-width="1" stroke-opacity="0.9" />
      <line x1={measV.x - 3} y1={measV.bottom} x2={measV.x + 3} y2={measV.bottom}
        stroke="#facc15" stroke-width="1" stroke-opacity="0.9" />

      <!-- Diameter label -->
      <text x={measH.right + 3} y={measH.y - 2}
        fill="#facc15" font-size="9" font-family="-apple-system, sans-serif" font-weight="bold">
        {vesselDiameterMm.toFixed(1)} mm
      </text>
    </svg>
  {/if}
</div>
