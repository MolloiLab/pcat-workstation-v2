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
   * Estimate vessel diameter from a cross-section image.
   *
   * Algorithm:
   *   1. Find the peak HU in a small central window → true lumen brightness.
   *      (Anchoring on center alone is unreliable when the centerline
   *      spline lands a pixel or two off the true lumen.)
   *   2. Compute a per-image half-max threshold = (peak + background) / 2.
   *      Background is estimated from the image's outer ring (perivascular
   *      fat / air). This adapts to each slice's contrast level instead of
   *      the old fixed 100 HU cut-off, which leaked measurements into
   *      adjacent enhanced myocardium or the aorta when their HU stayed
   *      above the absolute threshold.
   *   3. Scan outward along four axes through the peak; stop when HU drops
   *      below the half-max threshold. The diameter is the axis average,
   *      expressed in mm via the fixed 2·widthMm / sz pixel pitch.
   */
  function measureVesselDiameter(data: Float32Array, sz: number): number | null {
    const widthMm = 15.0;
    const center = Math.floor(sz / 2);
    const mmPerPixel = (2 * widthMm) / sz;

    // --- 1. Find the lumen peak in a small central window (~3 mm radius) ---
    const searchRadiusPx = Math.max(3, Math.round(3.0 / mmPerPixel));
    let peakHU = -Infinity;
    let peakRow = center;
    let peakCol = center;
    for (let r = Math.max(0, center - searchRadiusPx); r <= Math.min(sz - 1, center + searchRadiusPx); r++) {
      for (let c = Math.max(0, center - searchRadiusPx); c <= Math.min(sz - 1, center + searchRadiusPx); c++) {
        const v = data[r * sz + c];
        if (Number.isFinite(v) && v > peakHU) {
          peakHU = v;
          peakRow = r;
          peakCol = c;
        }
      }
    }

    // If the centre window has no bright structure, refuse to measure rather
    // than report nonsense. Contrast-enhanced coronary lumen is typically
    // 250-500 HU; a 150 HU floor skips dim/non-lumen slices.
    if (!Number.isFinite(peakHU) || peakHU < 150) {
      measH = null;
      measV = null;
      return null;
    }

    // --- 2. Background estimate from the outer ring (fat / air) ---
    const outerRing: number[] = [];
    for (let i = 0; i < sz; i++) {
      const top = data[i];
      const bot = data[(sz - 1) * sz + i];
      const lft = data[i * sz];
      const rgt = data[i * sz + (sz - 1)];
      for (const v of [top, bot, lft, rgt]) {
        if (Number.isFinite(v)) outerRing.push(v);
      }
    }
    outerRing.sort((a, b) => a - b);
    // 25th percentile tolerates bright corners (e.g. cardiac chamber).
    const bgHU = outerRing.length > 0
      ? outerRing[Math.floor(outerRing.length * 0.25)]
      : -80;
    const halfMax = (peakHU + bgHU) / 2;

    // --- 3. Scan outward from the peak along 4 axes; stop at half-max ---
    const findBoundary = (
      startIdx: number,
      step: number,
      getHU: (idx: number) => number,
      limit: number,
    ): number => {
      let idx = startIdx + step;
      while (idx >= 0 && idx < limit) {
        const hu = getHU(idx);
        if (!Number.isFinite(hu) || hu < halfMax) return idx;
        idx += step;
      }
      return Math.max(0, Math.min(limit - 1, idx));
    };

    const getHUh = (col: number) => data[peakRow * sz + col];
    const left = findBoundary(peakCol, -1, getHUh, sz);
    const right = findBoundary(peakCol, 1, getHUh, sz);
    const hDiamPx = right - left;

    const getHUv = (row: number) => data[row * sz + peakCol];
    const top = findBoundary(peakRow, -1, getHUv, sz);
    const bottom = findBoundary(peakRow, 1, getHUv, sz);
    const vDiamPx = bottom - top;

    // Safety: if the scan runs to the edge it is not measuring a vessel.
    const hitEdge =
      left <= 0 || right >= sz - 1 || top <= 0 || bottom >= sz - 1;
    if (hitEdge) {
      measH = null;
      measV = null;
      return null;
    }

    const avgDiamPx = (hDiamPx + vDiamPx) / 2;
    if (avgDiamPx < 2) {
      measH = null;
      measV = null;
      return null;
    }

    measH = { left, right, y: peakRow };
    measV = { top, bottom, x: peakCol };

    return avgDiamPx * mmPerPixel;
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
