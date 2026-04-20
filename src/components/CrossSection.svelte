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
   * Radial-FWHM lumen diameter with outlier rejection.
   *
   * The old 2-axis scan was fragile — one ray wandering into adjacent
   * enhanced myocardium or the aorta dominated the average and inflated the
   * reported diameter (e.g. 11.3 mm for an RCA that should read 3–4 mm).
   *
   * This version follows the standard radial-FWHM recipe used in research
   * coronary-lumen tools (see e.g. Çimen et al., "Reconstruction of
   * Coronary Arteries from X-ray Angiography", MedIA 2016; Wolterink et al.,
   * "Coronary artery centerline extraction in CCTA using a deep-learning
   * based orientation classifier", MedIA 2019). VMTK's colliding-fronts and
   * Horos's manual ROI both boil down to the same inside-out / threshold
   * idea, just wrapped differently; we do it in closed form per slice.
   *
   * Steps:
   *   1. Anchor on the brightest pixel inside a ~3 mm central window. The
   *      centerline spline can land a voxel or two off the true lumen;
   *      searching a small neighbourhood recovers the real lumen peak.
   *   2. Estimate background from the outer image ring (25th percentile to
   *      tolerate bright corners like a neighbouring chamber).
   *   3. Half-max threshold = (peak + background) / 2. Adaptive per slice
   *      instead of a fixed HU cutoff so contrast/noise level doesn't
   *      matter.
   *   4. Cast 16 rays from the peak at 22.5° steps; for each ray walk
   *      outward in 0.5-pixel increments and sub-pixel refine the boundary
   *      where HU first crosses half-max.
   *   5. Robust median + MAD (median absolute deviation) outlier reject.
   *      Rays that escape into adjacent enhanced tissue typically give
   *      radii 2–4× the median and get dropped. If fewer than half the
   *      rays survive, the slice is unreliable — return null instead of a
   *      bad number.
   *   6. Diameter = 2 × median(inlier radii). Sanity-clip to 1–8 mm
   *      (coronary range); outside that, treat as non-measurement.
   *
   * Returns the diameter in mm, or null when the slice cannot be measured
   * reliably. Also populates `measH` / `measV` (H and V calipers through
   * the peak) and `lumenContour` (16-point polygon of the detected
   * boundary) for the SVG overlay.
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
    // 150 HU is a lenient floor: any enhanced coronary lumen clears it even
    // at reduced kV / low iodine. Below this the centre is almost certainly
    // not inside a contrast-filled lumen.
    if (!Number.isFinite(peakHU) || peakHU < 150) return reject();

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

    // --- 3. Per-slice half-max threshold ---
    const halfMax = (peakHU + bgHU) / 2;

    // --- 4. Cast 16 rays and find sub-pixel half-max boundary per ray ---
    const N_RAYS = 16;
    // Coronary lumen rarely exceeds ~5 mm; cap the scan so a ray escaping
    // into adjacent tissue doesn't run across the whole FOV.
    const MAX_RADIUS_MM = 5.0;
    const maxRadiusPx = Math.min(sz / 2 - 1, Math.ceil(MAX_RADIUS_MM / mmPerPixel));

    const sampleBilinear = (x: number, y: number): number => {
      if (x < 0 || y < 0 || x > sz - 1 || y > sz - 1) return NaN;
      const x0 = Math.floor(x);
      const y0 = Math.floor(y);
      const x1 = Math.min(sz - 1, x0 + 1);
      const y1 = Math.min(sz - 1, y0 + 1);
      const fx = x - x0;
      const fy = y - y0;
      const v00 = data[y0 * sz + x0];
      const v01 = data[y0 * sz + x1];
      const v10 = data[y1 * sz + x0];
      const v11 = data[y1 * sz + x1];
      if (!Number.isFinite(v00) || !Number.isFinite(v01)
          || !Number.isFinite(v10) || !Number.isFinite(v11)) return NaN;
      return v00 * (1 - fx) * (1 - fy)
           + v01 * fx * (1 - fy)
           + v10 * (1 - fx) * fy
           + v11 * fx * fy;
    };

    const STEP_PX = 0.5;
    const radiiPx: number[] = new Array(N_RAYS);
    const boundaryPts: Array<{ x: number; y: number }> = new Array(N_RAYS);
    let rayCappedCount = 0;

    for (let k = 0; k < N_RAYS; k++) {
      const theta = (k / N_RAYS) * 2 * Math.PI;
      const dx = Math.cos(theta);
      const dy = Math.sin(theta);
      let prevV = peakHU;
      let hitR = maxRadiusPx;
      let crossed = false;
      for (let r = STEP_PX; r <= maxRadiusPx; r += STEP_PX) {
        const x = peakCol + r * dx;
        const y = peakRow + r * dy;
        const v = sampleBilinear(x, y);
        if (!Number.isFinite(v) || v <= halfMax) {
          // Sub-pixel refine: linear interp between the last above-half-max
          // sample and this below-half-max one. When `prevV - v` is tiny
          // (plateau), fall back to the half-step.
          const drop = prevV - v;
          const frac = Number.isFinite(v) && drop > 1e-6
            ? Math.min(1, Math.max(0, (prevV - halfMax) / drop))
            : 0;
          hitR = r - STEP_PX + frac * STEP_PX;
          crossed = true;
          break;
        }
        prevV = v;
      }
      if (!crossed) rayCappedCount++;
      radiiPx[k] = hitR;
      boundaryPts[k] = { x: peakCol + hitR * dx, y: peakRow + hitR * dy };
    }

    // If almost every ray never crossed half-max, the "vessel" fills the
    // whole FOV — almost always because we're inside the aorta or a
    // contrast-filled chamber, not a coronary. Bail out.
    if (rayCappedCount >= N_RAYS - 2) return reject();

    // --- 5. Median + MAD outlier rejection ---
    const sortedR = [...radiiPx].sort((a, b) => a - b);
    const median = (arr: number[]) => arr[Math.floor(arr.length / 2)];
    const medianR = median(sortedR);
    const mad = median([...radiiPx.map((r) => Math.abs(r - medianR))].sort((a, b) => a - b));
    // 3·σ-equivalent rejection (σ ≈ 1.4826·MAD); floor at 1 px so perfectly
    // circular lumens don't reject everything due to MAD=0.
    const threshold = Math.max(mad * 1.4826 * 3.0, 1.0);
    const inlierIdx: number[] = [];
    for (let k = 0; k < N_RAYS; k++) {
      if (Math.abs(radiiPx[k] - medianR) <= threshold) inlierIdx.push(k);
    }
    if (inlierIdx.length < N_RAYS / 2) return reject();

    // --- 6. Diameter from inlier median; sanity-clip to coronary range ---
    const inlierRadii = inlierIdx.map((k) => radiiPx[k]).sort((a, b) => a - b);
    const finalR = median(inlierRadii);
    const diameterMm = 2 * finalR * mmPerPixel;
    if (diameterMm < 1.0 || diameterMm > 8.0) return reject();

    // H / V calipers for visual continuity with the old overlay. Index 0 is
    // +x (right), N/4 is +y (down), N/2 is -x (left), 3N/4 is -y (up) —
    // matches canvas coordinate conventions.
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
