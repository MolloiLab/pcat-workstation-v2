<script lang="ts">
  /**
   * 田字格 (2x2 grid) MPR layout.
   *
   * Orchestrates cornerstone3D: initialises the rendering engine, creates
   * three orthographic viewports (axial, coronal, sagittal), wires up tools,
   * and reacts to volume changes in the store.
   *
   * Grid positions:
   *   [0,0] Axial    [0,1] Coronal
   *   [1,0] Sagittal [1,1] ContextPanel
   */
  import { onMount, untrack } from 'svelte';
  import { tick } from 'svelte';
  import { Enums, setVolumesForViewports } from '@cornerstonejs/core';

  import { initCornerstone, getRenderingEngine } from '$lib/cornerstone/init';
  import { setupToolGroup } from '$lib/cornerstone/tools';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';

  import { seedStore } from '$lib/stores/seedStore.svelte';
  import { pipelineStore } from '$lib/stores/pipelineStore.svelte';

  import SliceViewport from './SliceViewport.svelte';
  import ContextPanel from './ContextPanel.svelte';

  // Viewport IDs
  const VP_AXIAL = 'vp-axial';
  const VP_CORONAL = 'vp-coronal';
  const VP_SAGITTAL = 'vp-sagittal';
  const VIEWPORT_IDS = [VP_AXIAL, VP_CORONAL, VP_SAGITTAL];

  // Bindable container elements from SliceViewport children
  let axialEl = $state<HTMLDivElement | null>(null);
  let coronalEl = $state<HTMLDivElement | null>(null);
  let sagittalEl = $state<HTMLDivElement | null>(null);

  let engineReady = $state(false);

  // Check if the active vessel has a computed centerline (requires 2+ seeds)
  let hasCenterline = $derived(
    seedStore.activeVesselData?.centerline !== null &&
    seedStore.activeVesselData?.centerline !== undefined &&
    (seedStore.activeVesselData?.centerline?.length ?? 0) >= 2,
  );

  // Derive the current workflow phase for the context panel
  // Priority: analysis > seeds (centerline) > dicom > empty
  let phase = $derived<'empty' | 'dicom' | 'seeds' | 'analysis'>(
    pipelineStore.status === 'complete'
      ? 'analysis'
      : hasCenterline
        ? 'seeds'
        : volumeStore.current
          ? 'dicom'
          : 'empty',
  );

  // ---------- Initialise cornerstone3D on mount ----------
  onMount(async () => {
    await initCornerstone();

    // Ensure DOM elements are rendered and have layout dimensions
    await tick();

    const engine = getRenderingEngine();

    // Small delay to guarantee browser has painted and elements have size
    await new Promise((r) => setTimeout(r, 50));

    if (!axialEl || !coronalEl || !sagittalEl) {
      console.error('MprPanel: viewport container elements not available');
      return;
    }

    engine.setViewports([
      {
        viewportId: VP_AXIAL,
        type: Enums.ViewportType.ORTHOGRAPHIC,
        element: axialEl,
        defaultOptions: { orientation: Enums.OrientationAxis.AXIAL },
      },
      {
        viewportId: VP_CORONAL,
        type: Enums.ViewportType.ORTHOGRAPHIC,
        element: coronalEl,
        defaultOptions: { orientation: Enums.OrientationAxis.CORONAL },
      },
      {
        viewportId: VP_SAGITTAL,
        type: Enums.ViewportType.ORTHOGRAPHIC,
        element: sagittalEl,
        defaultOptions: { orientation: Enums.OrientationAxis.SAGITTAL },
      },
    ]);

    setupToolGroup(VIEWPORT_IDS);
    engineReady = true;
  });

  // ---------- React to volume changes ----------
  $effect(() => {
    const csVolumeId = volumeStore.cornerstoneVolumeId;
    if (!csVolumeId || !engineReady) return;

    const engine = getRenderingEngine();

    setVolumesForViewports(
      engine,
      [{ volumeId: csVolumeId }],
      VIEWPORT_IDS,
    ).then(() => {
      engine.renderViewports(VIEWPORT_IDS);
    });
  });

  // ---------- Navigate viewports when a seed is selected ----------
  // ONLY re-run when selectedSeedIndex changes — use untrack() for seed data
  // to prevent re-navigation on every seed add/move.
  let lastNavigatedIdx: number | null = null;

  $effect(() => {
    const selectedIdx = seedStore.selectedSeedIndex;
    if (selectedIdx === lastNavigatedIdx) return;
    lastNavigatedIdx = selectedIdx;

    if (selectedIdx === null || !engineReady) return;

    // Read seed position without creating a reactive dependency on seed data
    const pos = untrack(() => {
      const data = seedStore.activeVesselData;
      if (!data || selectedIdx >= data.seeds.length) return null;
      return data.seeds[selectedIdx].position;
    });

    if (!pos || !isFinite(pos[0]) || !isFinite(pos[1]) || !isFinite(pos[2])) return;

    const engine = getRenderingEngine();
    for (const vpId of VIEWPORT_IDS) {
      const vp = engine.getViewport(vpId);
      if (!vp) continue;
      const camera = vp.getCamera();
      const oldFocal = camera.focalPoint as [number, number, number];
      const oldPos = camera.position as [number, number, number];

      const delta: [number, number, number] = [
        pos[0] - oldFocal[0],
        pos[1] - oldFocal[1],
        pos[2] - oldFocal[2],
      ];

      vp.setCamera({
        ...camera,
        focalPoint: [pos[0], pos[1], pos[2]] as [number, number, number],
        position: [
          oldPos[0] + delta[0],
          oldPos[1] + delta[1],
          oldPos[2] + delta[2],
        ] as [number, number, number],
      });
      vp.render();
    }
  });
</script>

<!--
  2x2 grid with 1px gap.
  The parent grid has bg-border so the gap colour is the border colour,
  while each cell has its own background — producing crisp 1px grid lines.
-->
<div class="grid h-full w-full grid-cols-2 grid-rows-2 gap-px bg-border">
  <!-- [0,0] Axial -->
  <SliceViewport
    orientation="axial"
    viewportId={VP_AXIAL}
    bind:containerEl={axialEl}
  />

  <!-- [0,1] Coronal -->
  <SliceViewport
    orientation="coronal"
    viewportId={VP_CORONAL}
    bind:containerEl={coronalEl}
  />

  <!-- [1,0] Sagittal -->
  <SliceViewport
    orientation="sagittal"
    viewportId={VP_SAGITTAL}
    bind:containerEl={sagittalEl}
  />

  <!-- [1,1] Context panel -->
  <ContextPanel {phase} />
</div>
