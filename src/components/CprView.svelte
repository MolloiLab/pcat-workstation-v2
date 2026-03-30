<script lang="ts">
  /**
   * Compound CPR view: straightened or curved CPR image (left 70%) with
   * 3 cross-sections (right 30%) at needle positions A, B, C.
   *
   * Two-phase architecture:
   *   Phase 1: centerline changes -> build_cpr_frame (once, ~100ms)
   *   Phase 2: rotation/needle changes -> render_cpr_image + render_cross_sections (fast, ~10ms)
   *
   * All image IPC uses raw binary (tauri::ipc::Response) -- no base64.
   *
   * Layout:
   *   +-------------------------+----------+
   *   |                         |  A (xs)  |
   *   |   Straightened/Curved   |----------|
   *   |   3 needle lines A/B/C  |  B (xs)  |
   *   |                         |----------|
   *   |                         |  C (xs)  |
   *   +-------------------------+----------+
   *            70%                  30%
   */
  import { invoke } from '@tauri-apps/api/core';
  import { navigateToWorldPos } from '$lib/navigation';
  import { seedStore, VESSEL_COLORS } from '$lib/stores/seedStore.svelte';
  import {
    type CprProjectionInfo,
    worldToStraightenedCpr,
    worldToCurvedCpr,
    straightenedCprToWorld,
    curvedCprToWorld,
  } from '$lib/cprProjection';
  import CrossSection from './CrossSection.svelte';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import { pipelineStore } from '$lib/stores/pipelineStore.svelte';

  // ---- Constants ----
  const CPR_WIDTH_MM = 25.0;

  // ---- Reactive state ----
  let cprCanvas: HTMLCanvasElement | undefined = $state();
  let rotationDeg = $state(0);
  // Initialize W/L from DICOM metadata (same as MPR views)
  let windowCenter = $state(volumeStore.current?.windowCenter ?? 40);
  let windowWidth = $state(volumeStore.current?.windowWidth ?? 400);

  // CPR mode: straightened (classic) vs curved (natural vessel path)
  let cprMode: 'straightened' | 'curved' = $state('curved');

  // FAI overlay toggle
  let showFaiOverlay = $state(false);

  // Auto-enable FAI overlay when analysis completes
  $effect(() => {
    if (pipelineStore.status === 'complete') {
      showFaiOverlay = true;
    }
  });

  // Needle B position as fraction (0..1); A and C are offset
  let needleBFraction = $state(0.5);
  let needleOffset = $state(0.05); // adjustable A-C spread (fraction of total arc)

  let needleAFraction = $derived(Math.max(0, needleBFraction - needleOffset));
  let needleCFraction = $derived(Math.min(1, needleBFraction + needleOffset));

  // Image dimensions from last CPR result
  let cprWidth = $state(768);
  let cprHeight = $state(384);
  let arclengths = $state<number[]>([]);
  let cprImageData = $state<Float32Array | null>(null);
  let loading = $state(false);

  // Phase 1: frame readiness
  let frameReady = $state(false);

  // Projection info for seed overlay (fetched after each render)
  let projectionInfo = $state<CprProjectionInfo | null>(null);

  // Seed dragging state on CPR canvas
  let draggingSeedIndex = $state<number | null>(null);
  let hoverSeedIndex = $state<number | null>(null);

  // Zoom state for CPR canvas
  let cprZoom = $state(1);
  let cprPanX = $state(0);
  let cprPanY = $state(0);

  // Current centerline from seed store
  let centerline = $derived(seedStore.activeVesselData?.centerline ?? null);

  // Ostium fraction for the active vessel (used in overlays + toolbar)
  let activeOstiumFrac = $derived(seedStore.getOstiumFraction(seedStore.activeVessel));

  // Batch cross-section results (one per needle: A, B, C)
  type BatchCrossSectionItem = {
    imageData: Float32Array;
    pixels: number;
    arc_mm: number;
  };
  let batchXsA = $state<BatchCrossSectionItem | null>(null);
  let batchXsB = $state<BatchCrossSectionItem | null>(null);
  let batchXsC = $state<BatchCrossSectionItem | null>(null);

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

  /**
   * Decode the raw binary CPR response:
   *   [width: u32 LE][height: u32 LE][n_arclengths: u32 LE]
   *   [arclengths: n * f64 LE]
   *   [image: width*height * f32 LE]
   */
  function decodeCprBinary(buffer: ArrayBuffer): {
    width: number;
    height: number;
    arclengths: number[];
    image: Float32Array;
  } {
    const view = new DataView(buffer);
    const width = view.getUint32(0, true);
    const height = view.getUint32(4, true);
    const nArc = view.getUint32(8, true);

    const arcOffset = 12;
    const arclengths: number[] = [];
    for (let i = 0; i < nArc; i++) {
      arclengths.push(view.getFloat64(arcOffset + i * 8, true));
    }

    const imgOffset = arcOffset + nArc * 8;
    const image = new Float32Array(buffer, imgOffset, width * height);
    return { width, height, arclengths, image };
  }

  /**
   * Decode the raw binary cross-sections response:
   *   [n_sections: u32 LE]
   *   For each: [pixels: u32 LE][arc_mm: f64 LE][image: pixels*pixels * f32 LE]
   */
  function decodeCrossSectionsBinary(buffer: ArrayBuffer): BatchCrossSectionItem[] {
    const view = new DataView(buffer);
    const nSections = view.getUint32(0, true);
    const results: BatchCrossSectionItem[] = [];
    let offset = 4;

    for (let i = 0; i < nSections; i++) {
      const pixels = view.getUint32(offset, true);
      offset += 4;
      const arc_mm = view.getFloat64(offset, true);
      offset += 8;
      const imgLen = pixels * pixels;
      const imageData = new Float32Array(buffer, offset, imgLen);
      offset += imgLen * 4;
      results.push({ imageData, pixels, arc_mm });
    }
    return results;
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
      const raw = data[i];
      if (raw !== raw) {
        imgData.data[i * 4] = 0;
        imgData.data[i * 4 + 1] = 0;
        imgData.data[i * 4 + 2] = 0;
        imgData.data[i * 4 + 3] = 255;
        continue;
      }
      const gray = Math.max(0, Math.min(255, Math.round(((raw - lo) / range) * 255)));

      // FAI overlay: color fat-range pixels green→red (full color, no grayscale blend)
      if (showFaiOverlay && raw >= -190 && raw <= -30) {
        const t = (raw - (-190)) / ((-30) - (-190));
        const r = Math.round(t < 0.5 ? t * 2 * 255 : 255);
        const g = Math.round(t < 0.5 ? 255 : (1 - (t - 0.5) * 2) * 255);
        imgData.data[i * 4]     = r;
        imgData.data[i * 4 + 1] = g;
        imgData.data[i * 4 + 2] = 20;
      } else {
        imgData.data[i * 4]     = gray;
        imgData.data[i * 4 + 1] = gray;
        imgData.data[i * 4 + 2] = gray;
      }
      imgData.data[i * 4 + 3] = 255;
    }
    ctx.putImageData(imgData, 0, 0);
  }

  /** Draw needle lines and arclength ticks as overlay. */
  function drawOverlays(cvs: HTMLCanvasElement) {
    const ctx = cvs.getContext('2d')!;
    const w = cvs.width;
    const h = cvs.height;

    // In curved mode, needle lines are less meaningful (vessel is not straightened)
    // but we still draw them at the same fractional position for consistency.

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

    if (cprMode === 'straightened') {
      // Straightened mode: vertical needle lines
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
    } else if (projectionInfo) {
      // Curved mode: draw needle lines perpendicular to vessel tangent
      const nPos = projectionInfo.positions.length;
      const drawCurvedNeedle = (frac: number, color: string, label: string, isDashed: boolean) => {
        const idx = Math.round(frac * (nPos - 1));
        const clampedIdx = Math.min(idx, nPos - 1);
        const pos = projectionInfo!.positions[clampedIdx];
        const projected = worldToCurvedCpr(pos, projectionInfo!, w, h);
        if (!projected) return;
        const [cx, cy] = projected;

        // Compute tangent direction from adjacent projected points
        const prevIdx = Math.max(0, clampedIdx - 1);
        const nextIdx = Math.min(nPos - 1, clampedIdx + 1);
        const prevPos = projectionInfo!.positions[prevIdx];
        const nextPos = projectionInfo!.positions[nextIdx];
        const prevProj = worldToCurvedCpr(prevPos, projectionInfo!, w, h);
        const nextProj = worldToCurvedCpr(nextPos, projectionInfo!, w, h);

        if (prevProj && nextProj) {
          const tx = nextProj[0] - prevProj[0];
          const ty = nextProj[1] - prevProj[1];
          const tlen = Math.sqrt(tx * tx + ty * ty);
          if (tlen > 0.1) {
            // Perpendicular to tangent (the needle line direction)
            const px = -ty / tlen;
            const py = tx / tlen;
            const lineLen = h * 0.4; // extend across a good portion of the view

            ctx.beginPath();
            ctx.strokeStyle = color;
            ctx.lineWidth = isDashed ? 1 : 1.5;
            ctx.setLineDash(isDashed ? [4, 3] : []);
            ctx.moveTo(cx - px * lineLen, cy - py * lineLen);
            ctx.lineTo(cx + px * lineLen, cy + py * lineLen);
            ctx.stroke();
            ctx.setLineDash([]);
          }
        }

        // Label
        ctx.font = 'bold 11px -apple-system, sans-serif';
        ctx.textAlign = 'center';
        ctx.fillStyle = color;
        ctx.fillText(label, cx, cy - 10);
      };

      drawCurvedNeedle(needleAFraction, '#ffee00', 'A', true);
      drawCurvedNeedle(needleBFraction, '#00ffcc', 'B', false);
      drawCurvedNeedle(needleCFraction, '#ffee00', 'C', true);
    }

    // --- Centerline: subtle horizontal dashed line at vertical midpoint ---
    const midY = Math.round(h / 2);
    ctx.beginPath();
    ctx.strokeStyle = 'rgba(255,255,255,0.15)';
    ctx.lineWidth = 0.5;
    ctx.setLineDash([6, 4]);
    ctx.moveTo(0, midY);
    ctx.lineTo(w, midY);
    ctx.stroke();
    ctx.setLineDash([]);

    // Arclength ticks every 10mm along the bottom (straightened mode only)
    if (cprMode === 'straightened' && arclengths.length > 0) {
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

    // --- Ostium marker ---
    if (activeOstiumFrac !== null && cprMode === 'straightened') {
      const ox = Math.round(activeOstiumFrac * w);

      // Shaded region: proximal side (before ostium) is dimmed
      ctx.fillStyle = 'rgba(0,0,0,0.25)';
      ctx.fillRect(0, 0, ox, h);

      // Solid magenta vertical line
      ctx.beginPath();
      ctx.strokeStyle = '#ff00ff';
      ctx.lineWidth = 2;
      ctx.setLineDash([]);
      ctx.moveTo(ox, 0);
      ctx.lineTo(ox, h);
      ctx.stroke();

      // Label with background
      ctx.font = 'bold 10px -apple-system, sans-serif';
      ctx.textAlign = 'center';
      ctx.fillStyle = '#ff00ff';
      ctx.fillText('OSTIUM', ox, h - 6);
    } else if (activeOstiumFrac !== null && cprMode === 'curved' && projectionInfo) {
      // In curved mode, draw ostium marker at the projected position
      const nPos = projectionInfo.positions.length;
      const idx = Math.round(activeOstiumFrac * (nPos - 1));
      const clampedIdx = Math.min(idx, nPos - 1);
      const pos = projectionInfo.positions[clampedIdx];
      const projected = worldToCurvedCpr(pos, projectionInfo, w, h);
      if (projected) {
        const [cx, cy] = projected;

        // Diamond marker
        ctx.save();
        ctx.translate(cx, cy);
        ctx.rotate(Math.PI / 4);
        ctx.fillStyle = 'rgba(255,0,255,0.7)';
        ctx.fillRect(-5, -5, 10, 10);
        ctx.strokeStyle = 'white';
        ctx.lineWidth = 1.5;
        ctx.strokeRect(-5, -5, 10, 10);
        ctx.restore();

        // Label
        ctx.font = 'bold 10px -apple-system, sans-serif';
        ctx.textAlign = 'center';
        ctx.fillStyle = '#ff00ff';
        ctx.fillText('OSTIUM', cx, cy - 12);
      }
    }

    // --- Centerline polyline on CPR ---
    if (projectionInfo && cprMode === 'curved') {
      const color = VESSEL_COLORS[seedStore.activeVessel];
      const nPos = projectionInfo.positions.length;
      const step = Math.max(1, Math.floor(nPos / 200)); // sample every few points

      ctx.beginPath();
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.5;
      ctx.globalAlpha = 0.5;
      let started = false;
      for (let j = 0; j < nPos; j += step) {
        const projected = worldToCurvedCpr(projectionInfo.positions[j], projectionInfo, w, h);
        if (!projected) { started = false; continue; }
        if (!started) { ctx.moveTo(projected[0], projected[1]); started = true; }
        else { ctx.lineTo(projected[0], projected[1]); }
      }
      ctx.stroke();
      ctx.globalAlpha = 1.0;
    }

    // --- Seed markers on CPR ---
    if (projectionInfo) {
      const vessel = seedStore.activeVessel;
      const data = seedStore.activeVesselData;
      const color = VESSEL_COLORS[vessel];
      const selectedIdx = seedStore.selectedSeedIndex;

      for (let i = 0; i < data.seeds.length; i++) {
        const seedPos = data.seeds[i].position;
        // Seeds are in [x,y,z] world coords; projection expects [z,y,x]
        const seedZyx: [number, number, number] = [seedPos[2], seedPos[1], seedPos[0]];

        const projected = cprMode === 'curved'
          ? worldToCurvedCpr(seedZyx, projectionInfo, w, h)
          : worldToStraightenedCpr(seedZyx, projectionInfo, w, h);

        if (!projected) continue;
        const [cx, cy] = projected;

        const isSelected = i === selectedIdx;
        const isHovered = i === hoverSeedIndex;
        const radius = isSelected || isHovered ? 5 : 4;

        // Glow ring for selected/hovered
        if (isSelected) {
          ctx.beginPath();
          ctx.arc(cx, cy, radius + 3, 0, Math.PI * 2);
          ctx.strokeStyle = 'rgba(255,255,255,0.5)';
          ctx.lineWidth = 2;
          ctx.stroke();
        } else if (isHovered) {
          ctx.beginPath();
          ctx.arc(cx, cy, radius + 2, 0, Math.PI * 2);
          ctx.strokeStyle = 'rgba(255,255,255,0.3)';
          ctx.lineWidth = 1.5;
          ctx.stroke();
        }

        // Seed circle
        ctx.beginPath();
        ctx.arc(cx, cy, radius, 0, Math.PI * 2);
        ctx.fillStyle = color;
        ctx.fill();
        ctx.strokeStyle = 'white';
        ctx.lineWidth = 1.2;
        ctx.stroke();

        // Index label
        ctx.font = 'bold 9px -apple-system, sans-serif';
        ctx.textAlign = 'center';
        ctx.fillStyle = 'white';
        ctx.fillText(`${i}`, cx, cy - radius - 3);
      }
    }

    // Mode badge removed — toolbar already shows the mode.
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

  // Re-render when FAI overlay is toggled (uses cached image data)
  $effect(() => {
    showFaiOverlay; // track dependency
    repaintCanvas();
  });

  // ---- Phase 1: Build frame when centerline changes ----

  let frameDebounce: ReturnType<typeof setTimeout> | undefined;
  let buildingFrame = false;

  $effect(() => {
    const cl = centerline;

    if (!cl || cl.length < 2) {
      cprImageData = null;
      frameReady = false;
      return;
    }

    // Centerline changed -- rebuild frame
    frameReady = false;
    loading = true;
    clearTimeout(frameDebounce);
    frameDebounce = setTimeout(async () => {
      if (buildingFrame) return;
      buildingFrame = true;
      try {
        // Downsample to max 100 points for smooth CPR while keeping IPC fast
        const sampled = downsample(cl, 100);
        // The spline centerline is in cornerstone3D world coords [x, y, z]
        // Rust expects [z, y, x]
        const centerlineZyx = sampled.map(
          ([x, y, z]) => [z, y, x] as [number, number, number],
        );

        await invoke('build_cpr_frame', {
          centerlineMm: centerlineZyx,
          pixelsWide: 768,
        });

        frameReady = true;
        // Trigger initial render
        renderCpr();
        renderCrossSections();
      } catch (e) {
        console.error('CprView: build_cpr_frame failed', e);
      } finally {
        buildingFrame = false;
        loading = false;
      }
    }, 200);

    return () => clearTimeout(frameDebounce);
  });

  // ---- Phase 2: Render when rotation or mode changes (uses cached frame) ----

  let renderDebounce: ReturnType<typeof setTimeout> | undefined;
  let renderingCpr = false;

  $effect(() => {
    // Track rotation and mode deps
    void rotationDeg;
    void cprMode;

    if (!frameReady) return;

    clearTimeout(renderDebounce);
    renderDebounce = setTimeout(() => {
      renderCpr();
      renderCrossSections();
    }, 50);

    return () => clearTimeout(renderDebounce);
  });

  async function renderCpr() {
    if (!frameReady || renderingCpr) return;
    renderingCpr = true;
    try {
      let buffer: ArrayBuffer;

      if (cprMode === 'curved') {
        buffer = await invoke<ArrayBuffer>('render_curved_cpr_image', {
          rotationDeg,
          widthMm: CPR_WIDTH_MM,
          pixelsWide: 512,
          pixelsHigh: 512,
          slabMm: 1.0,
        });
      } else {
        buffer = await invoke<ArrayBuffer>('render_cpr_image', {
          rotationDeg,
          widthMm: CPR_WIDTH_MM,
          pixelsHigh: 384,
          slabMm: 1.0,
        });
      }

      const decoded = decodeCprBinary(buffer);
      cprWidth = decoded.width;
      cprHeight = decoded.height;
      arclengths = decoded.arclengths;
      cprImageData = decoded.image;

      // Fetch projection info for seed overlay (lightweight JSON)
      fetchProjectionInfo();
    } catch (e) {
      console.error('CprView: render CPR failed', e);
    } finally {
      renderingCpr = false;
    }
  }

  /** Fetch projection info from Rust for seed overlay mapping. */
  async function fetchProjectionInfo() {
    if (!frameReady) {
      projectionInfo = null;
      return;
    }
    try {
      projectionInfo = await invoke<CprProjectionInfo>('get_cpr_projection_info', {
        rotationDeg,
        widthMm: CPR_WIDTH_MM,
        pixelsWide: 512,
        pixelsHigh: 512,
      });
    } catch (e) {
      console.error('CprView: get_cpr_projection_info failed', e);
      projectionInfo = null;
    }
  }

  // Re-render canvas when image data, W/L, needle positions change
  $effect(() => {
    // Touch reactive deps so Svelte tracks them for this effect
    void cprImageData;
    void windowCenter;
    void windowWidth;
    void needleAFraction;
    void needleBFraction;
    void needleCFraction;
    void arclengths;
    void cprMode;
    // Track seed state for centerline overlay dots
    void seedStore.activeVesselData;
    void seedStore.selectedSeedIndex;
    // Track ostium for overlay update
    void activeOstiumFrac;
    // Track projection info for seed overlay
    void projectionInfo;
    void hoverSeedIndex;

    repaintCanvas();
  });

  // ---- Cross-section computation (uses cached frame, raw binary) ----

  let xsDebounce: ReturnType<typeof setTimeout> | undefined;
  let computingXs = false;

  $effect(() => {
    // Track needle and rotation deps
    void needleAFraction;
    void needleBFraction;
    void needleCFraction;
    void rotationDeg;

    if (!frameReady) {
      batchXsA = null;
      batchXsB = null;
      batchXsC = null;
      return;
    }

    clearTimeout(xsDebounce);
    xsDebounce = setTimeout(() => {
      renderCrossSections();
    }, 100);

    return () => clearTimeout(xsDebounce);
  });

  async function renderCrossSections() {
    if (!frameReady || computingXs) return;
    computingXs = true;
    try {
      const buffer = await invoke<ArrayBuffer>('render_cross_sections', {
        positionFractions: [needleAFraction, needleBFraction, needleCFraction],
        rotationDeg,
        widthMm: 15.0,
        pixels: 128,
      });

      const results = decodeCrossSectionsBinary(buffer);
      if (results.length === 3) {
        batchXsA = results[0];
        batchXsB = results[1];
        batchXsC = results[2];
      }
    } catch (e) {
      console.error('CprView: render_cross_sections failed', e);
    } finally {
      computingXs = false;
    }
  }

  // ---- Seed selection -> needle B navigation ----

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

  // ---- Helpers: navigate MPR to needle B's current position ----

  /** Compute the world position at needle B and navigate MPR views there. */
  function navigateToNeedlePos() {
    const cl = seedStore.activeVesselData?.centerline;
    if (!cl || cl.length < 2) return;
    const idx = Math.round(needleBFraction * (cl.length - 1));
    const pos = cl[Math.min(idx, cl.length - 1)];
    navigateToWorldPos(pos);
  }

  // ---- Needle dragging ----

  let dragging = $state(false);

  /** Find the seed index nearest to the given canvas position, within hitRadius. */
  function findSeedAtCanvasPos(canvasX: number, canvasY: number, hitRadius: number): number | null {
    if (!projectionInfo || !cprCanvas) return null;
    const data = seedStore.activeVesselData;
    const w = cprCanvas.width;
    const h = cprCanvas.height;
    const rect = cprCanvas.getBoundingClientRect();
    const scaleX = rect.width / w;
    const scaleY = rect.height / h;

    let bestIdx: number | null = null;
    let bestDist = hitRadius * hitRadius;

    for (let i = 0; i < data.seeds.length; i++) {
      const seedPos = data.seeds[i].position;
      const seedZyx: [number, number, number] = [seedPos[2], seedPos[1], seedPos[0]];
      const projected = cprMode === 'curved'
        ? worldToCurvedCpr(seedZyx, projectionInfo, w, h)
        : worldToStraightenedCpr(seedZyx, projectionInfo, w, h);
      if (!projected) continue;

      const dx = canvasX - projected[0] * scaleX;
      const dy = canvasY - projected[1] * scaleY;
      const d = dx * dx + dy * dy;
      if (d < bestDist) {
        bestDist = d;
        bestIdx = i;
      }
    }
    return bestIdx;
  }

  function onCanvasMouseDown(event: MouseEvent) {
    if (event.button !== 0 || !cprCanvas) return;

    // Shift+click sets ostium marker
    if (event.shiftKey) {
      const rect = cprCanvas.getBoundingClientRect();
      const x = event.clientX - rect.left;
      const fraction = Math.max(0, Math.min(1, x / rect.width));
      seedStore.setOstiumFraction(fraction);
      event.preventDefault();
      return;
    }

    const rect = cprCanvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;

    // Priority 2: Check if near a seed (8px hit radius)
    const seedIdx = findSeedAtCanvasPos(x, y, 8);
    if (seedIdx !== null) {
      seedStore.selectSeed(seedIdx);
      draggingSeedIndex = seedIdx;
      event.preventDefault();
      return;
    }

    if (cprMode === 'straightened') {
      const fraction = x / rect.width;
      // Check if click is near needle B (within ~8px)
      const bPixel = needleBFraction * rect.width;
      if (Math.abs(x - bPixel) < 8) {
        dragging = true;
        event.preventDefault();
      } else {
        needleBFraction = Math.max(0, Math.min(1, fraction));
        navigateToNeedlePos();
      }
    } else if (projectionInfo && cprCanvas) {
      // Curved mode: find nearest centerline point to click position
      const canvasPixelX = (x / rect.width) * cprCanvas.width;
      const canvasPixelY = (y / rect.height) * cprCanvas.height;
      const nPos = projectionInfo.positions.length;
      const w = cprCanvas.width;
      const h = cprCanvas.height;

      let bestIdx = 0;
      let bestDist = Infinity;
      for (let j = 0; j < nPos; j++) {
        const projected = worldToCurvedCpr(projectionInfo.positions[j], projectionInfo, w, h);
        if (!projected) continue;
        const dx = canvasPixelX - projected[0];
        const dy = canvasPixelY - projected[1];
        const d = dx * dx + dy * dy;
        if (d < bestDist) {
          bestDist = d;
          bestIdx = j;
        }
      }
      needleBFraction = bestIdx / (nPos - 1);
      navigateToNeedlePos();
    }
  }

  function onCanvasMouseMove(event: MouseEvent) {
    if (!cprCanvas) return;
    const rect = cprCanvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;

    // Seed dragging takes priority
    if (draggingSeedIndex !== null && projectionInfo) {
      // Map canvas CSS position to canvas pixel position
      const canvasPixelX = (x / rect.width) * cprCanvas.width;
      const canvasPixelY = (y / rect.height) * cprCanvas.height;

      // Unproject to 3D
      const worldZyx = cprMode === 'curved'
        ? curvedCprToWorld(canvasPixelX, canvasPixelY, projectionInfo, cprCanvas.width, cprCanvas.height)
        : straightenedCprToWorld(canvasPixelX, canvasPixelY, projectionInfo, cprCanvas.width, cprCanvas.height);

      // Convert [z,y,x] back to [x,y,z] for seedStore
      const worldXyz: [number, number, number] = [worldZyx[2], worldZyx[1], worldZyx[0]];
      seedStore.moveSeed(draggingSeedIndex, worldXyz);
      return;
    }

    // Needle B dragging
    if (dragging) {
      if (cprMode === 'straightened') {
        needleBFraction = Math.max(0, Math.min(1, x / rect.width));
      }
      // In curved mode, needle dragging isn't supported (use scroll instead)
      navigateToNeedlePos();
      return;
    }

    // Hover detection for seed highlighting
    const seedIdx = findSeedAtCanvasPos(x, y, 8);
    if (seedIdx !== hoverSeedIndex) {
      hoverSeedIndex = seedIdx;
    }
  }

  function onCanvasMouseUp() {
    if (draggingSeedIndex !== null) {
      draggingSeedIndex = null;
      return;
    }
    if (dragging) {
      navigateToNeedlePos();
    }
    dragging = false;
  }

  /** Scroll/pinch on CPR canvas.
   *  Pinch (ctrlKey): zoom toward cursor.
   *  Scroll: move needle B position. */
  function onCanvasWheel(event: WheelEvent) {
    event.preventDefault();

    if (event.ctrlKey) {
      // Pinch-to-zoom toward cursor using transform-origin
      const zoomFactor = 1 - event.deltaY * 0.01;
      const newZoom = Math.max(1, Math.min(8, cprZoom * zoomFactor));
      if (cprCanvas) {
        const rect = cprCanvas.parentElement!.getBoundingClientRect();
        const cursorX = (event.clientX - rect.left) / rect.width;
        const cursorY = (event.clientY - rect.top) / rect.height;
        // Set transform-origin to cursor position (as offset from center)
        cprPanX = 0.5 - cursorX;
        cprPanY = 0.5 - cursorY;
      }
      cprZoom = newZoom;
    } else {
      // Scroll: move needle B
      const sensitivity = 0.0003;
      const delta = -event.deltaY * sensitivity;
      needleBFraction = Math.max(0, Math.min(1, needleBFraction + delta));
      navigateToNeedlePos();
    }
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
    if (draggingSeedIndex !== null || dragging) onCanvasMouseMove(event);
    else if (wlDragging) onCanvasRightMove(event);
    else onCanvasMouseMove(event); // hover detection
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
    <!-- Left: CPR (70%) -->
    <div class="relative flex min-h-0 flex-[7] flex-col">
      <!-- CPR label -->
      <span
        class="pointer-events-none absolute left-2 top-1.5 z-10 text-[11px] font-semibold tracking-wider text-text-secondary/60"
      >
        CPR &mdash; {seedStore.activeVessel}
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
            Place seeds along the vessel (start in aorta, trace into coronary)
          </p>
        </div>
      {:else}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="min-h-0 flex-1 overflow-hidden relative"
          style:cursor={draggingSeedIndex !== null ? 'grabbing' : hoverSeedIndex !== null ? 'grab' : 'crosshair'}
          onmousedown={handleMouseDown}
          oncontextmenu={onCanvasContextMenu}
          onwheel={onCanvasWheel}
        >
          <canvas
            bind:this={cprCanvas}
            class="absolute inset-0"
            style="image-rendering: pixelated; width: 100%; height: 100%; transform: scale({cprZoom}); transform-origin: {(0.5 - cprPanX) * 100}% {(0.5 - cprPanY) * 100}%;"
          ></canvas>
        </div>
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
            batchImageData={batchXsA?.imageData ?? null}
            batchPixels={batchXsA?.pixels ?? null}
            arcMmProp={batchXsA?.arc_mm ?? null}
            showFaiOverlay={showFaiOverlay}
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
            batchImageData={batchXsB?.imageData ?? null}
            batchPixels={batchXsB?.pixels ?? null}
            arcMmProp={batchXsB?.arc_mm ?? null}
            showFaiOverlay={showFaiOverlay}
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
            batchImageData={batchXsC?.imageData ?? null}
            batchPixels={batchXsC?.pixels ?? null}
            arcMmProp={batchXsC?.arc_mm ?? null}
            showFaiOverlay={showFaiOverlay}
          />
        </div>
      {:else}
        <div class="flex flex-1 items-center justify-center bg-surface-secondary">
          <p class="text-[10px] text-text-secondary/40">No cross-sections</p>
        </div>
      {/if}
    </div>
  </div>

  <!-- Bottom toolbar: rotation slider + mode toggle -->
  <div
    class="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-t border-border bg-surface-secondary px-3 py-1"
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
        title="Rotate the cross-section viewing angle"
      />
      <span class="w-7 text-right tabular-nums">{rotationDeg}&deg;</span>
    </label>

    <span class="text-[10px] text-text-secondary/40">|</span>

    <!-- Curved / Straightened toggle -->
    <button
      class="rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors
        {cprMode === 'straightened'
          ? 'bg-accent/20 text-accent'
          : 'text-text-secondary/60 hover:text-text-secondary'}"
      onclick={() => { cprMode = 'straightened'; }}
    >
      Straightened
    </button>
    <button
      class="rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors
        {cprMode === 'curved'
          ? 'bg-accent/20 text-accent'
          : 'text-text-secondary/60 hover:text-text-secondary'}"
      onclick={() => { cprMode = 'curved'; }}
    >
      Curved
    </button>

    <button
      class="rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors
        {showFaiOverlay
          ? 'bg-error/20 text-error'
          : 'text-text-secondary/60 hover:text-text-secondary'}"
      onclick={() => { showFaiOverlay = !showFaiOverlay; }}
      title="Fat Attenuation Index overlay: green = healthy fat, red = inflamed fat"
    >
      FAI
    </button>

    <span class="text-[10px] text-text-secondary/40">|</span>

    <span class="text-[10px] tabular-nums text-text-secondary/50">
      B: {(needleBFraction * 100).toFixed(0)}%
    </span>

    <label class="flex items-center gap-1 text-[10px] text-text-secondary">
      <span>Spread</span>
      <input
        type="range"
        min="0.01"
        max="0.20"
        step="0.01"
        bind:value={needleOffset}
        class="h-1 w-16 cursor-pointer accent-accent"
        title="Spacing between cross-section positions A and C"
      />
      <span class="w-6 text-right tabular-nums">{(needleOffset * 100).toFixed(0)}%</span>
    </label>

    <span class="text-[10px] text-text-secondary/40">|</span>

    <button
      class="rounded px-2 py-0.5 text-[10px] font-medium transition-colors"
      style={activeOstiumFrac !== null
        ? 'background-color: rgba(255,0,255,0.15); color: #ff00ff;'
        : 'color: #ff00ff;'}
      onclick={() => seedStore.setOstiumFraction(needleBFraction)}
      title="Mark where the coronary artery exits the aorta (proximal reference point for analysis)"
    >
      {activeOstiumFrac !== null ? 'Ostium Set' : 'Set Ostium'}
    </button>

    {#if activeOstiumFrac !== null}
      <span class="text-[10px] tabular-nums" style="color: #ff00ff;">
        {(activeOstiumFrac * 100).toFixed(0)}%
      </span>
      <button
        class="text-[10px] text-text-secondary/40 hover:text-error"
        onclick={() => seedStore.setOstiumFraction(null)}
        title="Clear ostium"
      >
        &times;
      </button>
    {/if}

    <span class="text-[10px] text-text-secondary/30">
      Scroll: needle &middot; Shift+click: ostium &middot; Drag: refine &middot; Right-drag: W/L
    </span>
  </div>
</div>
