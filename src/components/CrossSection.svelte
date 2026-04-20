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
   * Previous radial-FWHM attempt failed on real data: one ray wandering
   * into an adjacent bright structure (chamber blood, enhanced
   * myocardium, aorta at the ostium) made the contour a 4- or 6-petal
   * star with the diameter dominated by the outliers. Even with
   * median + MAD rejection, cardinal-axis leaks gave convex-hull-like
   * over-estimates (e.g. an 8 mm report for a 3 mm RCA lumen).
   *
   * Switch to connected-component / region growing:
   *   1. Anchor on the brightest pixel in the central ~3 mm window.
   *   2. Estimate background as the 25th percentile of the outer ring.
   *   3. Half-max threshold = max(180, (peak + bg) / 2). The 180 HU floor
   *      is conservative — it stops measurements from leaking into
   *      enhanced myocardium (typically 80–150 HU) when a nearby chamber
   *      pushes the background high. Lumen is reliably ≥ 250 HU with
   *      standard contrast so this cutoff does not miss real vessels.
   *   4. 4-connected BFS from the peak; only pixels with HU ≥ half-max
   *      get added, capped at 4 mm radial distance (coronary lumen is
   *      almost never > 4 mm radius).
   *   5. Diameter = 2 · √(area / π). Equivalent-circle diameter is
   *      naturally immune to any individual ray leaking — it's driven by
   *      total blob area.
   *   6. Sanity-clip to 1–6 mm. Outside that, return null rather than a
   *      bad number.
   *
   * Why not a simple threshold? Region growing requires connectivity to
   * the seed, so a separate bright blob 2 mm away (calcified plaque,
   * vein, neighbouring artery) is ignored. A plain threshold would
   * include all of those in the area count.
   *
   * This matches the canonical "colliding-fronts" family from VMTK
   * (ITK's ConnectedThreshold + distance map + area), but done in 2D
   * per-slice so it runs in ~1 ms without a level-set solve.
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

    // --- 1. Peak search in ~3 mm central window ---
    const searchRadiusPx = Math.max(3, Math.round(3.0 / mmPerPixel));
    let peakHU = -Infinity;
    let peakRow = center;
    let peakCol = center;
    const r0 = Math.max(0, center - searchRadiusPx);
    const r1 = Math.min(sz - 1, center + searchRadiusPx);
    for (let r = r0; r <= r1; r++) {
      for (let c = r0; c <= r1; c++) {
        const v = data[r * sz + c];
        if (Number.isFinite(v) && v > peakHU) {
          peakHU = v;
          peakRow = r;
          peakCol = c;
        }
      }
    }
    // 120 HU: lenient enough to catch a hypo-enhanced RCA (e.g. low-kV
    // scan, thinner IV contrast bolus) while still rejecting slices where
    // the centerline has drifted into pure myocardium.
    if (!Number.isFinite(peakHU) || peakHU < 120) return reject();

    // --- 2. Background: 25th percentile of outer ring ---
    const outerRing: number[] = [];
    for (let i = 0; i < sz; i++) {
      const candidates = [
        data[i],
        data[(sz - 1) * sz + i],
        data[i * sz],
        data[i * sz + (sz - 1)],
      ];
      for (const v of candidates) {
        if (Number.isFinite(v)) outerRing.push(v);
      }
    }
    outerRing.sort((a, b) => a - b);
    const bgHU = outerRing.length > 0
      ? outerRing[Math.floor(outerRing.length * 0.25)]
      : -80;

    // --- 3. Per-slice half-max threshold with a conservative floor ---
    // 180 HU floor prevents leaks into enhanced myocardium (typically
    // 80–150 HU) even when a neighbouring chamber pushes the background
    // up. A real enhanced coronary lumen is ≥ 250 HU so this cutoff does
    // not miss true lumens in any clinically reasonable scan.
    const halfMax = Math.max(180, (peakHU + bgHU) / 2);

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
