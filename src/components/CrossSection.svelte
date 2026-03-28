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
   * Estimate vessel diameter from cross-section image.
   * Scans from the center outward for bright lumen (HU > 150).
   * Limitations: assumes vessel is centered, may overestimate if
   * calcified plaque (also bright) is present. For display only.
   */
  function measureVesselDiameter(data: Float32Array, sz: number): number | null {
    const widthMm = 15.0;
    const lumenThreshold = 150;
    const center = Math.floor(sz / 2);

    let left = center;
    let right = center;
    while (left > 0 && data[center * sz + left] > lumenThreshold) left--;
    while (right < sz - 1 && data[center * sz + right] > lumenThreshold) right++;
    const hDiamPx = right - left;

    let top = center;
    let bottom = center;
    while (top > 0 && data[top * sz + center] > lumenThreshold) top--;
    while (bottom < sz - 1 && data[bottom * sz + center] > lumenThreshold) bottom++;
    const vDiamPx = bottom - top;

    const avgDiamPx = (hDiamPx + vDiamPx) / 2;
    if (avgDiamPx < 2) {
      measH = null;
      measV = null;
      return null;
    }

    // Store pixel positions for visual overlay
    measH = { left, right, y: center };
    measV = { top, bottom, x: center };

    const mmPerPixel = (2 * widthMm) / sz;
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
    {#if vesselDiameterMm !== null}
      <span class="text-[10px] tabular-nums text-accent">
        {vesselDiameterMm.toFixed(1)} mm
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
        stroke="#00ffcc" stroke-width="1" stroke-opacity="0.8" />
      <line x1={measH.left} y1={measH.y - 3} x2={measH.left} y2={measH.y + 3}
        stroke="#00ffcc" stroke-width="1" stroke-opacity="0.8" />
      <line x1={measH.right} y1={measH.y - 3} x2={measH.right} y2={measH.y + 3}
        stroke="#00ffcc" stroke-width="1" stroke-opacity="0.8" />

      <!-- Vertical caliper -->
      <line x1={measV.x} y1={measV.top} x2={measV.x} y2={measV.bottom}
        stroke="#00ffcc" stroke-width="1" stroke-opacity="0.8" />
      <line x1={measV.x - 3} y1={measV.top} x2={measV.x + 3} y2={measV.top}
        stroke="#00ffcc" stroke-width="1" stroke-opacity="0.8" />
      <line x1={measV.x - 3} y1={measV.bottom} x2={measV.x + 3} y2={measV.bottom}
        stroke="#00ffcc" stroke-width="1" stroke-opacity="0.8" />

      <!-- Diameter label -->
      <text x={measH.right + 3} y={measH.y - 2}
        fill="#00ffcc" font-size="9" font-family="-apple-system, sans-serif" font-weight="bold">
        {vesselDiameterMm.toFixed(1)}
      </text>
    </svg>
  {/if}
</div>
