<script lang="ts">
  /**
   * MMD Analysis View — full-screen analysis tab for multi-material
   * decomposition results.
   *
   * Layout:
   *   ┌────────────────────┬──────────────────────────┐
   *   │                    │  Surface Plot (Plotly 3D) │
   *   │  Cross-Section     │  [arc slider]             │
   *   │  (SnakeEditor)     ├──────────────────────────┤
   *   │                    │  (radial profile TBD)    │
   *   ├────────────────────┴──────────────────────────┤
   *   │ [Water|Lipid|Iodine|Ca|rho] [Vol%|Mass]       │
   *   ├───────────────────────────────────────────────┤
   *   │ CrossSectionStrip: [0mm] [2mm] [4mm*] ...     │
   *   ├───────────────────────────────────────────────┤
   *   │ Contour: [Init][Evolve][Reset][Accept] | MMD  │
   *   └───────────────────────────────────────────────┘
   */
  import {
    type AnnotationTarget,
    type CrossSectionSurface,
    type MmdSummary,
    generateAnnotationTargets,
    sampleSurfaces,
    runMmdOnRoi,
  } from '$lib/api';

  import SnakeEditor from './SnakeEditor.svelte';
  import CrossSectionStrip from './CrossSectionStrip.svelte';
  import OverlaySelector from './OverlaySelector.svelte';
  import SurfacePlotPanel from './SurfacePlotPanel.svelte';

  type Props = {
    /** World-space centerline points for the vessel segment. */
    centerlineMm: [number, number, number][];
  };

  let { centerlineMm }: Props = $props();

  /* ── State ───────────────────────────────────────────── */

  let targets = $state<AnnotationTarget[]>([]);
  let selectedIndex = $state(0);
  let statusMap = $state<Record<number, 'pending' | 'in-progress' | 'done'>>({});
  let snakePoints = $state<Record<number, [number, number][]>>({});

  let material = $state('lipid');
  let unit = $state('fraction');

  let surfaces = $state<CrossSectionSurface[]>([]);
  let surfaceIndex = $state(0);

  let mmdSummary = $state<MmdSummary | null>(null);
  let mmdBusy = $state(false);
  let mmdError = $state('');

  let loadingTargets = $state(false);

  /* ── Derived ─────────────────────────────────────────── */

  let currentTarget = $derived(targets[selectedIndex] ?? null);
  let currentSnake = $derived(snakePoints[selectedIndex] ?? null);
  let currentStatus = $derived(statusMap[selectedIndex] ?? 'pending');
  let finalizedCount = $derived(
    Object.values(statusMap).filter((s) => s === 'done').length,
  );
  let totalCount = $derived(targets.length);

  /* ── Load targets on mount / centerline change ────── */

  $effect(() => {
    if (centerlineMm.length >= 2) {
      loadTargets();
    }
  });

  async function loadTargets() {
    loadingTargets = true;
    try {
      targets = await generateAnnotationTargets(centerlineMm);
      selectedIndex = 0;
      statusMap = {};
      snakePoints = {};
      surfaces = [];
      mmdSummary = null;
      mmdError = '';
    } catch (err) {
      console.error('Failed to generate annotation targets:', err);
    } finally {
      loadingTargets = false;
    }
  }

  /* ── Handlers ─────────────────────────────────────────── */

  function handleSelect(index: number) {
    selectedIndex = index;
  }

  function handleSnakeUpdate(points: [number, number][]) {
    snakePoints = { ...snakePoints, [selectedIndex]: points };
    if (statusMap[selectedIndex] !== 'done') {
      statusMap = { ...statusMap, [selectedIndex]: 'in-progress' };
    }
  }

  function handleFinalize() {
    statusMap = { ...statusMap, [selectedIndex]: 'done' };
  }

  function handleMaterialChange(m: string) {
    material = m;
    // Refresh surfaces if MMD has been run.
    if (mmdSummary) {
      refreshSurfaces();
    }
  }

  function handleUnitChange(u: string) {
    unit = u;
    if (mmdSummary) {
      refreshSurfaces();
    }
  }

  function handleSurfaceSlider(index: number) {
    surfaceIndex = index;
  }

  async function handleRunMmd() {
    if (mmdBusy) return;
    mmdBusy = true;
    mmdError = '';
    try {
      mmdSummary = await runMmdOnRoi('pwsqs');
      await refreshSurfaces();
    } catch (err) {
      mmdError = err instanceof Error ? err.message : String(err);
      console.error('MMD failed:', err);
    } finally {
      mmdBusy = false;
    }
  }

  async function refreshSurfaces() {
    try {
      surfaces = await sampleSurfaces(material, unit);
      surfaceIndex = Math.min(surfaceIndex, Math.max(0, surfaces.length - 1));
    } catch (err) {
      console.error('Failed to sample surfaces:', err);
    }
  }
</script>

<div class="flex h-full w-full flex-col bg-surface">
  {#if loadingTargets}
    <div class="flex flex-1 items-center justify-center">
      <span class="text-xs text-text-secondary">Generating cross-sections...</span>
    </div>
  {:else if targets.length === 0}
    <div class="flex flex-1 items-center justify-center">
      <span class="text-xs text-text-secondary">No annotation targets. Ensure a centerline with 2+ points is selected.</span>
    </div>
  {:else}
    <!-- Main content: editor + surface plot side-by-side -->
    <div class="flex min-h-0 flex-1">
      <!-- Left: SnakeEditor for the selected cross-section -->
      <div class="flex w-1/2 shrink-0 flex-col border-r border-border">
        {#if currentTarget}
          <SnakeEditor
            target={currentTarget}
            targetIndex={selectedIndex}
            snakePoints={currentSnake}
            onSnakeUpdate={handleSnakeUpdate}
            onFinalize={handleFinalize}
            status={currentStatus}
          />
        {/if}
      </div>

      <!-- Right: surface plot + radial profile placeholder -->
      <div class="flex min-w-0 flex-1 flex-col">
        <div class="flex-1 overflow-y-auto p-2">
          <SurfacePlotPanel
            {surfaces}
            selectedIndex={surfaceIndex}
            {material}
            {unit}
            onSliderChange={handleSurfaceSlider}
          />
        </div>

        <!-- Radial profile placeholder -->
        <div class="border-t border-border px-3 py-2">
          <span class="text-[10px] text-text-secondary/60">Radial profile (coming soon)</span>
        </div>
      </div>
    </div>

    <!-- Overlay selector bar -->
    <div class="shrink-0 border-t border-border bg-surface-secondary">
      <OverlaySelector
        {material}
        {unit}
        onMaterialChange={handleMaterialChange}
        onUnitChange={handleUnitChange}
      />
    </div>

    <!-- Cross-section strip -->
    <div class="shrink-0 border-t border-border bg-surface-secondary">
      <CrossSectionStrip
        {targets}
        {selectedIndex}
        {statusMap}
        onSelect={handleSelect}
      />
    </div>

    <!-- Bottom toolbar: MMD controls + progress -->
    <div class="flex shrink-0 items-center gap-2 border-t border-border bg-surface-secondary px-3 py-1.5">
      <button
        class="rounded bg-accent/10 px-3 py-1 text-xs font-medium text-accent hover:bg-accent/20 active:bg-accent/30 disabled:opacity-40"
        onclick={handleRunMmd}
        disabled={mmdBusy || finalizedCount === 0}
        title="Run PWSQS multi-material decomposition on finalized contours"
      >
        {mmdBusy ? 'Running MMD...' : 'Run MMD'}
      </button>

      <!-- Progress indicator -->
      <span class="text-[11px] tabular-nums text-text-secondary">
        {finalizedCount}/{totalCount} done
      </span>

      {#if mmdSummary}
        <span class="ml-1 text-[10px] text-success">
          MMD {mmdSummary.converged ? 'converged' : 'done'} ({mmdSummary.n_voxels.toLocaleString()} voxels)
        </span>
      {/if}

      {#if mmdError}
        <span class="ml-1 truncate text-[10px] text-error">{mmdError}</span>
      {/if}
    </div>
  {/if}
</div>
