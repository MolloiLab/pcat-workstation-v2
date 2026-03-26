<script lang="ts">
  /**
   * SVG overlay for seed markers and centerline polylines.
   *
   * Positioned absolutely over a SliceViewport. Projects 3D seed positions
   * and centerline points into 2D canvas coordinates via cornerstone3D's
   * `worldToCanvas()`. Uses `pointer-events: none` to avoid blocking
   * viewport mouse interactions.
   *
   * Visual feedback:
   *  - Selected seed: white glow ring behind the normal marker
   *  - Ghost insertion dot: shown at hoverInsertPos on centerline hover
   */
  import { seedStore, VESSEL_COLORS, type Vessel } from '$lib/stores/seedStore.svelte';
  import { getRenderingEngine } from '$lib/cornerstone/init';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import type { Types } from '@cornerstonejs/core';

  type Props = {
    viewportId: string;
    containerEl: HTMLDivElement | null;
    /** World-space position for ghost insertion dot (from centerline hover). */
    hoverInsertPos?: [number, number, number] | null;
  };

  let { viewportId, containerEl, hoverInsertPos = null }: Props = $props();

  const vesselNames: Vessel[] = ['LAD', 'LCx', 'RCA'];

  type ProjectedSeed = {
    cx: number;
    cy: number;
    vessel: Vessel;
    seedIndex: number;
    type: 'ostium' | 'waypoint';
    inBounds: boolean;
    isSelected: boolean;
  };

  type ProjectedCenterline = {
    points: string; // SVG polyline points string
    vessel: Vessel;
  };

  // Projected data updated by $effect
  let projectedSeeds = $state<ProjectedSeed[]>([]);
  let projectedCenterlines = $state<ProjectedCenterline[]>([]);
  let projectedGhostDot = $state<{ cx: number; cy: number; color: string } | null>(null);
  /** Crosshair position for the selected seed (projected onto this viewport). */
  let crosshairPos = $state<{ cx: number; cy: number; width: number; height: number } | null>(null);

  /**
   * Trigger version: bumped on each cornerstone render event so the
   * $effect re-projects all points. We listen for the IMAGE_RENDERED
   * event from cornerstone to know when the viewport camera changed.
   */
  let renderVersion = $state(0);

  // Subscribe to cornerstone render events so overlay updates on scroll/pan/zoom
  $effect(() => {
    if (!containerEl) return;

    const handler = () => {
      renderVersion++;
    };

    // cornerstone3D dispatches 'CORNERSTONE_IMAGE_RENDERED' on the element
    containerEl.addEventListener('CORNERSTONE_IMAGE_RENDERED', handler);
    // Also listen for general cornerstoneTools camera modifications
    containerEl.addEventListener('CORNERSTONE_CAMERA_MODIFIED', handler);

    return () => {
      containerEl!.removeEventListener('CORNERSTONE_IMAGE_RENDERED', handler);
      containerEl!.removeEventListener('CORNERSTONE_CAMERA_MODIFIED', handler);
    };
  });

  /**
   * Determine which axis index corresponds to the viewport's slice normal.
   * Returns the world-coordinate component (0=x, 1=y, 2=z) that is
   * perpendicular to the viewed plane, plus the slice spacing for that axis.
   *
   * Axial: normal is Z (index 2), spacing[0] (sz)
   * Coronal: normal is Y (index 1), spacing[1] (sy)
   * Sagittal: normal is X (index 0), spacing[2] (sx)
   */
  function getSliceAxisInfo(vp: any): { axisIndex: number; spacing: number } | null {
    const meta = volumeStore.current;
    if (!meta) return null;

    const camera = vp.getCamera();
    if (!camera?.viewPlaneNormal) return null;

    const n = camera.viewPlaneNormal as [number, number, number];
    const absN = n.map(Math.abs);
    const maxIdx = absN[0] > absN[1] && absN[0] > absN[2] ? 0 : absN[1] > absN[2] ? 1 : 2;

    // spacing is [sz, sy, sx] in volumeStore
    const spacingMap = [meta.spacing[2], meta.spacing[1], meta.spacing[0]]; // [sx, sy, sz]
    return { axisIndex: maxIdx, spacing: spacingMap[maxIdx] };
  }

  /** Check if a world point is near the current slice plane (within +/-sliceTolerance). */
  function isNearSlice(
    worldPoint: [number, number, number],
    focalPoint: Types.Point3,
    axisIndex: number,
    tolerance: number,
  ): boolean {
    return Math.abs(worldPoint[axisIndex] - focalPoint[axisIndex]) <= tolerance;
  }

  // Re-project all seeds and centerlines whenever seeds change or viewport renders
  $effect(() => {
    // Track reactive dependencies — reading these values ensures re-runs
    const vesselData = seedStore.vessels;
    const currentActiveVessel = seedStore.activeVessel;
    const currentSelectedIndex = seedStore.selectedSeedIndex;
    const currentHoverPos = hoverInsertPos;
    void renderVersion;
    const csVol = volumeStore.cornerstoneVolumeId;

    if (!containerEl || !csVol) {
      projectedSeeds = [];
      projectedCenterlines = [];
      projectedGhostDot = null;
      return;
    }

    let engine;
    try {
      engine = getRenderingEngine();
    } catch {
      projectedSeeds = [];
      projectedCenterlines = [];
      projectedGhostDot = null;
      return;
    }

    const vp = engine.getViewport(viewportId);
    if (!vp) {
      projectedSeeds = [];
      projectedCenterlines = [];
      projectedGhostDot = null;
      return;
    }

    const canvas = containerEl;
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;

    // Get slice-plane info for depth filtering
    const sliceInfo = getSliceAxisInfo(vp);
    const camera = vp.getCamera();
    const focalPoint = camera?.focalPoint as Types.Point3 | undefined;

    // --- Project seeds (only those near the current slice) ---
    const seeds: ProjectedSeed[] = [];
    for (const vessel of vesselNames) {
      const data = vesselData[vessel];
      for (let i = 0; i < data.seeds.length; i++) {
        const seed = data.seeds[i];
        // Depth filter: hide seeds not near the current slice (+/-3 slices)
        if (sliceInfo && focalPoint) {
          const tolerance = sliceInfo.spacing * 3;
          if (!isNearSlice(seed.position as [number, number, number], focalPoint, sliceInfo.axisIndex, tolerance)) {
            continue; // skip — not on this slice
          }
        }

        const canvasPos = vp.worldToCanvas(seed.position as [number, number, number]);
        if (!canvasPos || !isFinite(canvasPos[0]) || !isFinite(canvasPos[1])) continue;
        const [cx, cy] = canvasPos;
        const inBounds = cx >= -20 && cx <= width + 20 && cy >= -20 && cy <= height + 20;
        const isSelected =
          vessel === currentActiveVessel &&
          currentSelectedIndex !== null &&
          i === currentSelectedIndex;
        seeds.push({
          cx,
          cy,
          vessel,
          seedIndex: i,
          type: seed.type,
          inBounds,
          isSelected,
        });
      }
    }
    projectedSeeds = seeds;

    // --- Project centerlines (only segments near the current slice) ---
    const centerlines: ProjectedCenterline[] = [];
    for (const vessel of vesselNames) {
      const data = vesselData[vessel];
      if (!data.centerline || data.centerline.length < 2) continue;

      const pts: string[] = [];
      for (const pt of data.centerline) {
        // Depth filter: only draw centerline points within +/-5 slices
        if (sliceInfo && focalPoint) {
          const tolerance = sliceInfo.spacing * 5;
          if (!isNearSlice(pt, focalPoint, sliceInfo.axisIndex, tolerance)) {
            // Break the polyline — push what we have so far
            if (pts.length >= 2) {
              centerlines.push({ points: pts.join(' '), vessel });
            }
            pts.length = 0;
            continue;
          }
        }

        const canvasPos = vp.worldToCanvas(pt);
        if (!canvasPos || !isFinite(canvasPos[0]) || !isFinite(canvasPos[1])) continue;
        pts.push(`${canvasPos[0]},${canvasPos[1]}`);
      }
      if (pts.length >= 2) {
        centerlines.push({ points: pts.join(' '), vessel });
      }
    }
    projectedCenterlines = centerlines;

    // --- Project ghost insertion dot ---
    if (currentHoverPos) {
      const canvasPos = vp.worldToCanvas(currentHoverPos);
      if (canvasPos && isFinite(canvasPos[0]) && isFinite(canvasPos[1])) {
        projectedGhostDot = {
          cx: canvasPos[0],
          cy: canvasPos[1],
          color: VESSEL_COLORS[currentActiveVessel],
        };
      } else {
        projectedGhostDot = null;
      }
    } else {
      projectedGhostDot = null;
    }

    // --- Crosshair lines at selected seed position ---
    if (currentSelectedIndex !== null) {
      const activeData = vesselData[currentActiveVessel];
      if (activeData && currentSelectedIndex < activeData.seeds.length) {
        const seedPos = activeData.seeds[currentSelectedIndex].position as [number, number, number];
        const canvasPos = vp.worldToCanvas(seedPos);
        if (canvasPos && isFinite(canvasPos[0]) && isFinite(canvasPos[1])) {
          crosshairPos = { cx: canvasPos[0], cy: canvasPos[1], width, height };
        } else {
          crosshairPos = null;
        }
      } else {
        crosshairPos = null;
      }
    } else {
      crosshairPos = null;
    }
  });
</script>

<svg
  class="pointer-events-none absolute inset-0 z-20 h-full w-full"
  xmlns="http://www.w3.org/2000/svg"
>
  <!-- Crosshair lines at selected seed position -->
  {#if crosshairPos}
    <!-- Horizontal crosshair line -->
    <line
      x1="0"
      y1={crosshairPos.cy}
      x2={crosshairPos.width}
      y2={crosshairPos.cy}
      stroke="rgba(255,255,255,0.4)"
      stroke-width="0.75"
      stroke-dasharray="4,3"
    />
    <!-- Vertical crosshair line -->
    <line
      x1={crosshairPos.cx}
      y1="0"
      x2={crosshairPos.cx}
      y2={crosshairPos.height}
      stroke="rgba(255,255,255,0.4)"
      stroke-width="0.75"
      stroke-dasharray="4,3"
    />
  {/if}

  <!-- Centerline polylines (drawn first, behind markers) -->
  {#each projectedCenterlines as cl}
    <polyline
      points={cl.points}
      fill="none"
      stroke={VESSEL_COLORS[cl.vessel]}
      stroke-width="1.5"
      stroke-opacity="0.6"
      stroke-linejoin="round"
    />
  {/each}

  <!-- Ghost insertion dot (shown on centerline hover) -->
  {#if projectedGhostDot}
    <circle
      cx={projectedGhostDot.cx}
      cy={projectedGhostDot.cy}
      r="5"
      fill={projectedGhostDot.color}
      fill-opacity="0.5"
      stroke={projectedGhostDot.color}
      stroke-width="1.5"
      stroke-opacity="0.5"
    />
  {/if}

  <!-- Seed markers -->
  {#each projectedSeeds as seed}
    {@const color = VESSEL_COLORS[seed.vessel]}
    {@const opacity = seed.inBounds ? 1 : 0.3}

    <!-- Selected seed glow ring (drawn behind the marker) -->
    {#if seed.isSelected}
      {#if seed.type === 'ostium'}
        <rect
          x={seed.cx - 7}
          y={seed.cy - 7}
          width="14"
          height="14"
          fill="none"
          stroke="white"
          stroke-width="3"
          stroke-opacity="0.5"
          rx="1"
        />
      {:else}
        <circle
          cx={seed.cx}
          cy={seed.cy}
          r="7"
          fill="none"
          stroke="white"
          stroke-width="3"
          stroke-opacity="0.5"
        />
      {/if}
    {/if}

    {#if seed.type === 'ostium'}
      <!-- Ostium: filled square -->
      <rect
        x={seed.cx - 5}
        y={seed.cy - 5}
        width="10"
        height="10"
        fill={color}
        fill-opacity={opacity * 0.8}
        stroke={color}
        stroke-width="1.5"
        stroke-opacity={opacity}
      />
    {:else}
      <!-- Waypoint: filled circle -->
      <circle
        cx={seed.cx}
        cy={seed.cy}
        r="4.5"
        fill={color}
        fill-opacity={opacity * 0.8}
        stroke={color}
        stroke-width="1.5"
        stroke-opacity={opacity}
      />
    {/if}
  {/each}
</svg>
