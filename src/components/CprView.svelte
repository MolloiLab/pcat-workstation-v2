<script lang="ts">
  /**
   * Compound CPR view: straightened CPR image (left 70%) with 3 cross-sections
   * (right 30%) at needle positions A, B, C.
   *
   * Layout:
   *   +-------------------------+----------+
   *   |                         |  A (xs)  |
   *   |   Straightened CPR      |----------|
   *   |   3 needle lines A/B/C  |  B (xs)  |
   *   |                         |----------|
   *   |                         |  C (xs)  |
   *   +-------------------------+----------+
   *            70%                  30%
   */
  import { invoke } from '@tauri-apps/api/core';
  import { seedStore } from '$lib/stores/seedStore.svelte';
  import CrossSection from './CrossSection.svelte';

  // ---- Types ----
  type CprCommandResult = {
    image_base64: string;
    shape: [number, number]; // [height, width]
    arclengths: number[];
  };

  // ---- Reactive state ----
  let cprCanvas: HTMLCanvasElement | undefined = $state();
  let rotationDeg = $state(0);
  let windowCenter = $state(40);
  let windowWidth = $state(400);

  // Needle B position as fraction (0..1); A and C are offset
  let needleBFraction = $state(0.5);
  const needleOffset = 0.05; // 5% of total arc

  let needleAFraction = $derived(Math.max(0, needleBFraction - needleOffset));
  let needleCFraction = $derived(Math.min(1, needleBFraction + needleOffset));

  // Image dimensions from last CPR result
  let cprWidth = $state(512);
  let cprHeight = $state(256);
  let arclengths = $state<number[]>([]);
  let cprImageData = $state<Float32Array | null>(null);
  let loading = $state(false);

  // Current centerline from seed store
  let centerline = $derived(seedStore.activeVesselData?.centerline ?? null);

  // ---- Helpers ----

  /** Downsample an array of points to at most maxPts, always keeping first and last. */
  function downsample(
    pts: [number, number, number][],
    maxPts: number,
  ): [number, number, number][] {
    if (pts.length <= maxPts) return pts;
    const step = (pts.length - 1) / (maxPts - 1);
    const result: [number, number, number][] = [];
    for (let i = 0; i < maxPts - 1; i++) {
      result.push(pts[Math.round(i * step)]);
    }
    result.push(pts[pts.length - 1]);
    return result;
  }

  function decodeBase64Float32(b64: string): Float32Array {
    const binary = atob(b64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return new Float32Array(bytes.buffer);
  }

  function renderCprToCanvas(
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
    const range = ww;
    for (let i = 0; i < data.length; i++) {
      const v = Math.round(((data[i] - lo) / range) * 255);
      const clamped = Math.max(0, Math.min(255, v));
      imgData.data[i * 4] = clamped;
      imgData.data[i * 4 + 1] = clamped;
      imgData.data[i * 4 + 2] = clamped;
      imgData.data[i * 4 + 3] = 255;
    }
    ctx.putImageData(imgData, 0, 0);
  }

  /** Draw needle lines and arclength ticks as overlay. */
  function drawOverlays(cvs: HTMLCanvasElement) {
    const ctx = cvs.getContext('2d')!;
    const w = cvs.width;
    const h = cvs.height;

    // Draw needle lines (vertical)
    const drawNeedle = (fraction: number, color: string, lineWidth: number) => {
      const x = Math.round(fraction * w);
      ctx.beginPath();
      ctx.strokeStyle = color;
      ctx.lineWidth = lineWidth;
      ctx.setLineDash([]);
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
      ctx.stroke();
    };

    // A (yellow, dashed)
    const xA = Math.round(needleAFraction * w);
    ctx.beginPath();
    ctx.strokeStyle = '#ffee00';
    ctx.lineWidth = 1;
    ctx.setLineDash([4, 3]);
    ctx.moveTo(xA, 0);
    ctx.lineTo(xA, h);
    ctx.stroke();
    ctx.setLineDash([]);

    // B (cyan, solid, thicker)
    drawNeedle(needleBFraction, '#00ffcc', 2);

    // C (yellow, dashed)
    const xC = Math.round(needleCFraction * w);
    ctx.beginPath();
    ctx.strokeStyle = '#ffee00';
    ctx.lineWidth = 1;
    ctx.setLineDash([4, 3]);
    ctx.moveTo(xC, 0);
    ctx.lineTo(xC, h);
    ctx.stroke();
    ctx.setLineDash([]);

    // Labels
    ctx.font = 'bold 11px -apple-system, sans-serif';
    ctx.textAlign = 'center';

    ctx.fillStyle = '#ffee00';
    ctx.fillText('A', xA, 14);

    ctx.fillStyle = '#00ffcc';
    ctx.fillText('B', Math.round(needleBFraction * w), 14);

    ctx.fillStyle = '#ffee00';
    ctx.fillText('C', xC, 14);

    // Arclength ticks every 10mm along the bottom
    if (arclengths.length > 0) {
      const totalArc = arclengths[arclengths.length - 1];
      ctx.font = '9px -apple-system, sans-serif';
      ctx.fillStyle = '#98989d';
      ctx.textAlign = 'center';
      ctx.strokeStyle = '#98989d';
      ctx.lineWidth = 0.5;

      for (let mm = 0; mm <= totalArc; mm += 10) {
        const frac = mm / totalArc;
        const x = Math.round(frac * w);
        // tick mark
        ctx.beginPath();
        ctx.moveTo(x, h - 10);
        ctx.lineTo(x, h - 2);
        ctx.stroke();
        // label
        ctx.fillText(`${mm}`, x, h - 12);
      }
    }
  }

  /** Full re-render: image + overlays. */
  function repaintCanvas() {
    if (!cprCanvas || !cprImageData) return;
    renderCprToCanvas(
      cprCanvas,
      cprImageData,
      cprWidth,
      cprHeight,
      windowCenter,
      windowWidth,
    );
    drawOverlays(cprCanvas);
  }

  // ---- CPR Computation (debounced) ----

  let cprDebounce: ReturnType<typeof setTimeout> | undefined;
  let computingCpr = false;

  $effect(() => {
    // Track reactive deps: centerline, rotation
    const cl = centerline;
    const rot = rotationDeg;

    if (!cl || cl.length < 2) {
      cprImageData = null;
      return;
    }

    clearTimeout(cprDebounce);
    cprDebounce = setTimeout(async () => {
      if (computingCpr) return;
      computingCpr = true;
      loading = true;
      try {
        // The spline centerline is in cornerstone3D world coords [x, y, z]
        // Rust expects [z, y, x]
        // Downsample to max 50 points to avoid slow IPC serialization
        const sampled = downsample(cl, 50);
        const centerlineZyx = sampled.map(
          ([x, y, z]) => [z, y, x] as [number, number, number],
        );

        const result = await invoke<CprCommandResult>('compute_cpr_image', {
          centerlineMm: centerlineZyx,
          rotationDeg: rot,
          widthMm: 25.0,
          slabMm: 3.0,
          pixelsWide: 512,
          pixelsHigh: 256,
        });

        cprWidth = result.shape[1];
        cprHeight = result.shape[0];
        arclengths = result.arclengths;
        cprImageData = decodeBase64Float32(result.image_base64);
      } catch (e) {
        console.error('CprView: CPR computation failed', e);
      } finally {
        computingCpr = false;
        loading = false;
      }
    }, 300);

    return () => clearTimeout(cprDebounce);
  });

  // Re-render when image data, W/L, or needle positions change
  $effect(() => {
    // Touch reactive deps so Svelte tracks them for this effect
    void cprImageData;
    void windowCenter;
    void windowWidth;
    void needleAFraction;
    void needleBFraction;
    void needleCFraction;
    void arclengths;

    repaintCanvas();
  });

  // ---- Seed selection → needle B navigation ----

  $effect(() => {
    const selectedIdx = seedStore.selectedSeedIndex;
    if (selectedIdx === null) return;
    const data = seedStore.activeVesselData;
    if (!data || selectedIdx >= data.seeds.length || !data.centerline) return;

    const seedPos = data.seeds[selectedIdx].position;
    const cl = data.centerline;
    if (cl.length < 2) return;

    // Find closest centerline point to the selected seed
    let minDist = Infinity;
    let closestIdx = 0;
    for (let i = 0; i < cl.length; i++) {
      const dx = cl[i][0] - seedPos[0];
      const dy = cl[i][1] - seedPos[1];
      const dz = cl[i][2] - seedPos[2];
      const d = dx * dx + dy * dy + dz * dz;
      if (d < minDist) {
        minDist = d;
        closestIdx = i;
      }
    }

    needleBFraction = closestIdx / (cl.length - 1);
  });

  // ---- Needle dragging ----

  let dragging = $state(false);

  function onCanvasMouseDown(event: MouseEvent) {
    if (event.button !== 0 || !cprCanvas) return;

    const rect = cprCanvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const fraction = x / rect.width;

    // Check if click is near needle B (within ~8px)
    const bPixel = needleBFraction * rect.width;
    if (Math.abs(x - bPixel) < 8) {
      dragging = true;
      event.preventDefault();
    } else {
      // Click elsewhere: snap B to that position
      needleBFraction = Math.max(0, Math.min(1, fraction));
    }
  }

  function onCanvasMouseMove(event: MouseEvent) {
    if (!dragging || !cprCanvas) return;
    const rect = cprCanvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    needleBFraction = Math.max(0, Math.min(1, x / rect.width));
  }

  function onCanvasMouseUp() {
    dragging = false;
  }

  // ---- W/L adjustment via right-drag on CPR canvas ----

  let wlDragging = $state(false);
  let wlStartX = $state(0);
  let wlStartY = $state(0);
  let wlStartCenter = $state(40);
  let wlStartWidth = $state(400);

  function onCanvasContextMenu(event: MouseEvent) {
    event.preventDefault();
  }

  function onCanvasRightDown(event: MouseEvent) {
    if (event.button !== 2) return;
    wlDragging = true;
    wlStartX = event.clientX;
    wlStartY = event.clientY;
    wlStartCenter = windowCenter;
    wlStartWidth = windowWidth;
    event.preventDefault();
  }

  function onCanvasRightMove(event: MouseEvent) {
    if (!wlDragging) return;
    const dx = event.clientX - wlStartX;
    const dy = event.clientY - wlStartY;
    windowWidth = Math.max(1, wlStartWidth + dx * 2);
    windowCenter = wlStartCenter - dy;
  }

  function onCanvasRightUp() {
    wlDragging = false;
  }

  // Combined mouse handlers
  function handleMouseDown(event: MouseEvent) {
    if (event.button === 0) onCanvasMouseDown(event);
    else if (event.button === 2) onCanvasRightDown(event);
  }

  function handleMouseMove(event: MouseEvent) {
    if (dragging) onCanvasMouseMove(event);
    if (wlDragging) onCanvasRightMove(event);
  }

  function handleMouseUp(event: MouseEvent) {
    if (event.button === 0) onCanvasMouseUp();
    else if (event.button === 2) onCanvasRightUp();
  }
</script>

<!-- Attach global listeners for drag continuation outside canvas -->
<svelte:window
  onmousemove={handleMouseMove}
  onmouseup={handleMouseUp}
/>

<div class="flex h-full w-full flex-col bg-surface-secondary">
  <!-- Main layout: CPR + cross-sections -->
  <div class="flex min-h-0 flex-1">
    <!-- Left: Straightened CPR (70%) -->
    <div class="relative flex min-h-0 flex-[7] flex-col">
      <!-- CPR label -->
      <span
        class="pointer-events-none absolute left-2 top-1.5 z-10 text-[11px] font-semibold tracking-wider text-text-secondary/60"
      >
        CPR — {seedStore.activeVessel}
      </span>

      <!-- W/L display -->
      <span
        class="pointer-events-none absolute right-2 top-1.5 z-10 text-[10px] font-mono tabular-nums text-text-secondary/50"
      >
        C:{windowCenter} W:{windowWidth}
      </span>

      <!-- Loading indicator -->
      {#if loading}
        <div class="absolute inset-0 z-20 flex items-center justify-center bg-black/30">
          <span class="text-xs text-text-secondary">Computing CPR...</span>
        </div>
      {/if}

      {#if !centerline || centerline.length < 2}
        <div class="flex flex-1 items-center justify-center">
          <p class="text-xs text-text-secondary/60">
            Place 2+ seeds to generate CPR
          </p>
        </div>
      {:else}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <canvas
          bind:this={cprCanvas}
          class="min-h-0 flex-1 cursor-crosshair"
          style="image-rendering: pixelated; width: 100%; height: 100%;"
          onmousedown={handleMouseDown}
          oncontextmenu={onCanvasContextMenu}
        ></canvas>
      {/if}
    </div>

    <!-- Right: 3 cross-sections (30%) -->
    <div
      class="flex min-h-0 flex-[3] flex-col gap-px border-l border-border bg-border"
    >
      {#if centerline && centerline.length >= 2}
        <div class="flex min-h-0 flex-1 flex-col bg-black">
          <CrossSection
            centerlineMm={centerline}
            positionFraction={needleAFraction}
            {rotationDeg}
            label="A"
            color="#ffee00"
            {windowCenter}
            {windowWidth}
          />
        </div>
        <div class="flex min-h-0 flex-1 flex-col bg-black">
          <CrossSection
            centerlineMm={centerline}
            positionFraction={needleBFraction}
            {rotationDeg}
            label="B"
            color="#00ffcc"
            {windowCenter}
            {windowWidth}
          />
        </div>
        <div class="flex min-h-0 flex-1 flex-col bg-black">
          <CrossSection
            centerlineMm={centerline}
            positionFraction={needleCFraction}
            {rotationDeg}
            label="C"
            color="#ffee00"
            {windowCenter}
            {windowWidth}
          />
        </div>
      {:else}
        <div class="flex flex-1 items-center justify-center bg-surface-secondary">
          <p class="text-[10px] text-text-secondary/40">No cross-sections</p>
        </div>
      {/if}
    </div>
  </div>

  <!-- Bottom toolbar: rotation slider -->
  <div
    class="flex h-8 shrink-0 items-center gap-3 border-t border-border bg-surface-secondary px-3"
  >
    <label class="flex items-center gap-2 text-[10px] text-text-secondary">
      <span>Rot</span>
      <input
        type="range"
        min="0"
        max="360"
        step="1"
        bind:value={rotationDeg}
        class="h-1 w-24 cursor-pointer accent-accent"
      />
      <span class="w-7 text-right tabular-nums">{rotationDeg}&deg;</span>
    </label>

    <span class="text-[10px] text-text-secondary/40">|</span>

    <span class="text-[10px] tabular-nums text-text-secondary/50">
      B: {(needleBFraction * 100).toFixed(0)}%
      {#if arclengths.length > 0}
        ({(needleBFraction * arclengths[arclengths.length - 1]).toFixed(1)} mm)
      {/if}
    </span>

    <span class="text-[10px] text-text-secondary/40">|</span>

    <span class="text-[10px] text-text-secondary/40">
      Right-drag: W/L
    </span>
  </div>
</div>
