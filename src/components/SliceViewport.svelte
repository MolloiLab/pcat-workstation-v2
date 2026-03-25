<script lang="ts">
  /**
   * Single cornerstone3D viewport container.
   *
   * This component only provides the DOM element; viewport creation is
   * handled by the parent MprPanel which calls renderingEngine.setViewports()
   * with all three containers together.
   *
   * Also handles:
   *  - Three-zone click detection (seed / centerline / empty space)
   *  - Drag-to-move selected seeds
   *  - Hover detection near centerline for ghost insertion dot
   *  - SVG overlay for seed markers and centerline polylines
   */
  import { getRenderingEngine } from '$lib/cornerstone/init';
  import { seedStore, type Vessel } from '$lib/stores/seedStore.svelte';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import SeedOverlay from './SeedOverlay.svelte';

  type Props = {
    orientation: 'axial' | 'coronal' | 'sagittal';
    viewportId: string;
    /** Bind-back: parent reads this to get the mounted DOM element. */
    containerEl?: HTMLDivElement | null;
  };

  let { orientation, viewportId, containerEl = $bindable(null) }: Props =
    $props();

  const labels: Record<string, string> = {
    axial: 'Axial',
    coronal: 'Coronal',
    sagittal: 'Sagittal',
  };

  const SEED_HIT_RADIUS = 8; // px — proximity threshold for seed selection
  const CENTERLINE_HIT_RADIUS = 6; // px — proximity threshold for centerline insertion

  const vesselNames: Vessel[] = ['RCA', 'LAD', 'LCx'];

  // --- Drag state ---
  let isDragging = $state(false);
  let dragSeedIndex = $state<number | null>(null);

  // --- Hover state for centerline ghost dot ---
  let hoveringCenterline = $state(false);
  let hoverInsertPos = $state<[number, number, number] | null>(null);

  /**
   * Get the cornerstone viewport object, or null if unavailable.
   */
  function getViewport() {
    if (!containerEl || !volumeStore.cornerstoneVolumeId) return null;
    let engine;
    try {
      engine = getRenderingEngine();
    } catch {
      return null;
    }
    return engine.getViewport(viewportId) ?? null;
  }

  /**
   * Convert client mouse coordinates to canvas-local coordinates.
   */
  function clientToCanvas(event: MouseEvent): [number, number] | null {
    if (!containerEl) return null;
    const rect = containerEl.getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  }

  /**
   * Convert canvas coordinates to world coordinates via cornerstone.
   */
  function canvasToWorld(canvasPos: [number, number]): [number, number, number] | null {
    const vp = getViewport();
    if (!vp) return null;
    const worldPos = vp.canvasToWorld(canvasPos) as [number, number, number];
    if (!worldPos || !isFinite(worldPos[0]) || !isFinite(worldPos[1]) || !isFinite(worldPos[2])) {
      return null;
    }
    return worldPos;
  }

  /**
   * Find the nearest seed across ALL vessels within hitRadius pixels.
   * Returns { vessel, seedIndex, distance } or null.
   */
  function findNearestSeed(canvasX: number, canvasY: number): {
    vessel: Vessel;
    seedIndex: number;
    distance: number;
  } | null {
    const vp = getViewport();
    if (!vp) return null;

    let best: { vessel: Vessel; seedIndex: number; distance: number } | null = null;

    for (const vessel of vesselNames) {
      const data = seedStore.vessels[vessel];
      for (let i = 0; i < data.seeds.length; i++) {
        const seed = data.seeds[i];
        const canvasPos = vp.worldToCanvas(seed.position as [number, number, number]);
        if (!canvasPos || !isFinite(canvasPos[0]) || !isFinite(canvasPos[1])) continue;

        const dist = Math.hypot(canvasPos[0] - canvasX, canvasPos[1] - canvasY);
        if (dist <= SEED_HIT_RADIUS && (!best || dist < best.distance)) {
          best = { vessel, seedIndex: i, distance: dist };
        }
      }
    }
    return best;
  }

  /**
   * Find the nearest point on the active vessel's centerline within hitRadius.
   * Returns the insertion index (between which two seeds) and the world position,
   * or null if not close enough.
   */
  function findNearestCenterlinePoint(canvasX: number, canvasY: number): {
    insertIndex: number;
    worldPos: [number, number, number];
    distance: number;
  } | null {
    const vp = getViewport();
    if (!vp) return null;

    const data = seedStore.activeVesselData;
    if (!data.centerline || data.centerline.length < 2 || data.seeds.length < 2) return null;

    // Project all centerline points to canvas
    const projected: { cx: number; cy: number; world: [number, number, number] }[] = [];
    for (const pt of data.centerline) {
      const canvasPos = vp.worldToCanvas(pt);
      if (!canvasPos || !isFinite(canvasPos[0]) || !isFinite(canvasPos[1])) continue;
      projected.push({ cx: canvasPos[0], cy: canvasPos[1], world: pt });
    }

    if (projected.length < 2) return null;

    // Find closest centerline point
    let bestDist = Infinity;
    let bestIdx = -1;
    for (let i = 0; i < projected.length; i++) {
      const dist = Math.hypot(projected[i].cx - canvasX, projected[i].cy - canvasY);
      if (dist < bestDist) {
        bestDist = dist;
        bestIdx = i;
      }
    }

    if (bestDist > CENTERLINE_HIT_RADIUS) return null;

    // Determine insertion index: figure out which segment of the seed list this
    // centerline point falls between. The centerline is a spline interpolation
    // of the seeds, so we find which two seeds the nearest point is between
    // by projecting seeds to canvas and finding the pair that brackets this point.
    const seedCanvasPositions: { cx: number; cy: number }[] = [];
    for (const seed of data.seeds) {
      const sp = vp.worldToCanvas(seed.position as [number, number, number]);
      if (!sp || !isFinite(sp[0]) || !isFinite(sp[1])) {
        seedCanvasPositions.push({ cx: -9999, cy: -9999 });
      } else {
        seedCanvasPositions.push({ cx: sp[0], cy: sp[1] });
      }
    }

    // Find which seed segment the closest centerline point is nearest to
    // by checking which pair of consecutive seeds it falls between
    let insertIndex = data.seeds.length; // default: append at end
    let bestSegDist = Infinity;

    for (let i = 0; i < seedCanvasPositions.length - 1; i++) {
      const a = seedCanvasPositions[i];
      const b = seedCanvasPositions[i + 1];
      // Distance from the centerline point to the midpoint of this seed segment
      const midX = (a.cx + b.cx) / 2;
      const midY = (a.cy + b.cy) / 2;
      const d = Math.hypot(projected[bestIdx].cx - midX, projected[bestIdx].cy - midY);
      if (d < bestSegDist) {
        bestSegDist = d;
        insertIndex = i + 1; // insert between seed[i] and seed[i+1]
      }
    }

    return {
      insertIndex,
      worldPos: projected[bestIdx].world,
      distance: bestDist,
    };
  }

  /**
   * Handle mousedown on the viewport — three-zone click detection + drag initiation.
   */
  function handleMouseDown(event: MouseEvent) {
    // Only handle left clicks without modifier keys
    if (event.button !== 0) return;
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    if (!volumeStore.cornerstoneVolumeId) return;

    const canvasPos = clientToCanvas(event);
    if (!canvasPos) return;
    const [canvasX, canvasY] = canvasPos;

    // --- Zone 1: Check proximity to existing seeds ---
    const nearestSeed = findNearestSeed(canvasX, canvasY);
    if (nearestSeed) {
      // Switch to the seed's vessel if needed, then select it
      if (nearestSeed.vessel !== seedStore.activeVessel) {
        seedStore.setActiveVessel(nearestSeed.vessel);
      }
      seedStore.selectSeed(nearestSeed.seedIndex);

      // Start drag
      isDragging = true;
      dragSeedIndex = nearestSeed.seedIndex;

      // Attach window-level listeners for drag
      window.addEventListener('mousemove', handleDragMove);
      window.addEventListener('mouseup', handleDragEnd);

      // Prevent cornerstone from also handling this as a tool interaction
      event.stopPropagation();
      return;
    }

    // --- Zone 2: Check proximity to centerline ---
    const nearestCL = findNearestCenterlinePoint(canvasX, canvasY);
    if (nearestCL) {
      seedStore.insertSeedAt(nearestCL.insertIndex, nearestCL.worldPos);
      // Clear hover state
      hoveringCenterline = false;
      hoverInsertPos = null;
      event.stopPropagation();
      return;
    }

    // --- Zone 3: Empty space — append new seed ---
    const worldPos = canvasToWorld([canvasX, canvasY]);
    if (!worldPos) return;

    // If a seed was selected, deselect first
    if (seedStore.selectedSeedIndex !== null) {
      seedStore.deselectSeed();
    }

    seedStore.addSeed(worldPos);
  }

  /**
   * Handle mouse movement during drag.
   */
  function handleDragMove(event: MouseEvent) {
    if (!isDragging || dragSeedIndex === null) return;

    const canvasPos = clientToCanvas(event);
    if (!canvasPos) return;

    const worldPos = canvasToWorld(canvasPos);
    if (!worldPos) return;

    seedStore.moveSeed(dragSeedIndex, worldPos);
  }

  /**
   * End drag operation.
   */
  function handleDragEnd() {
    isDragging = false;
    dragSeedIndex = null;
    window.removeEventListener('mousemove', handleDragMove);
    window.removeEventListener('mouseup', handleDragEnd);
  }

  /**
   * Handle mouse movement over the viewport for centerline hover detection.
   */
  function handleMouseMove(event: MouseEvent) {
    // Don't check hover during drag
    if (isDragging) return;

    const canvasPos = clientToCanvas(event);
    if (!canvasPos) return;

    const nearestCL = findNearestCenterlinePoint(canvasPos[0], canvasPos[1]);
    if (nearestCL) {
      hoveringCenterline = true;
      hoverInsertPos = nearestCL.worldPos;
    } else {
      hoveringCenterline = false;
      hoverInsertPos = null;
    }
  }

  /**
   * Clear hover state when mouse leaves the viewport.
   */
  function handleMouseLeave() {
    if (!isDragging) {
      hoveringCenterline = false;
      hoverInsertPos = null;
    }
  }
</script>

<div class="relative h-full w-full overflow-hidden bg-black">
  <!-- Orientation label overlay -->
  <span
    class="pointer-events-none absolute left-2 top-1.5 z-10 text-[11px] font-medium text-text-secondary/80"
  >
    {labels[orientation]}
  </span>

  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    bind:this={containerEl}
    id={viewportId}
    role="application"
    class="h-full w-full"
    class:cursor-crosshair={hoveringCenterline}
    onmousedown={handleMouseDown}
    onmousemove={handleMouseMove}
    onmouseleave={handleMouseLeave}
    oncontextmenu={(e) => e.preventDefault()}
  ></div>

  <!-- Seed marker + centerline SVG overlay -->
  <SeedOverlay {viewportId} {containerEl} {hoverInsertPos} />
</div>
