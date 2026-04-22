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
    saveAnnotations,
    loadAnnotations,
    exportMmdCsv,
    useVesselWallAsContour,
  } from '$lib/api';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import { seedStore } from '$lib/stores/seedStore.svelte';

  import SnakeEditor from './SnakeEditor.svelte';
  import CrossSectionStrip from './CrossSectionStrip.svelte';
  import OverlaySelector from './OverlaySelector.svelte';
  import SurfacePlotPanel from './SurfacePlotPanel.svelte';

  type Props = {
    /** World-space centerline points for the vessel segment. */
    centerlineMm: [number, number, number][];
  };

  let { centerlineMm }: Props = $props();

  /* ── Derived from volume store ──────────────────────── */

  let dicomPath = $derived(volumeStore.current?.dicomPath ?? '');
  let patientName = $derived(volumeStore.current?.patientName ?? 'unknown');

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
  let saveBusy = $state(false);
  let saveMsg = $state('');
  let exportBusy = $state(false);

  /* ── Derived ─────────────────────────────────────────── */

  let currentTarget = $derived(targets[selectedIndex] ?? null);
  let currentSnake = $derived(snakePoints[selectedIndex] ?? null);
  let currentStatus = $derived(statusMap[selectedIndex] ?? 'pending');
  let finalizedCount = $derived(
    Object.values(statusMap).filter((s) => s === 'done').length,
  );
  let totalCount = $derived(targets.length);

  // Arc-length offset: the first target sits at `start_arc_mm` (the ostium's
  // arc-length along the centerline). Subtracting this in the UI displays
  // distances as offsets from the ostium rather than from centerline point 0.
  let arcOffsetMm = $derived(targets[0]?.arc_mm ?? 0);

  /* ── Load targets on mount / centerline change ────── */

  $effect(() => {
    if (centerlineMm.length >= 2) {
      loadTargets();
    }
  });

  async function loadTargets() {
    loadingTargets = true;
    try {
      // Prefer the user-placed ostium marker over the first centerline waypoint
      // so MMD cross-sections start at the true coronary ostium (matches FAI).
      // seedStore returns [x, y, z] (cornerstone world); Rust pipeline expects
      // [z, y, x] — flip to match (same convention as pipelineStore.toZyx).
      const ostiumWorld = seedStore.getOstiumWorldPosForVessel(seedStore.activeVessel);
      const ostiumZyx: [number, number, number] | null = ostiumWorld
        ? [ostiumWorld[2], ostiumWorld[1], ostiumWorld[0]]
        : null;
      targets = await generateAnnotationTargets(centerlineMm, ostiumZyx);
      selectedIndex = 0;
      statusMap = {};
      snakePoints = {};
      surfaces = [];
      mmdSummary = null;
      mmdError = '';

      // Try to restore saved annotations for this patient.
      if (dicomPath) {
        try {
          const saved = await loadAnnotations(dicomPath);
          if (saved) {
            // Restore snake points and status map.
            const restoredSnake: Record<number, [number, number][]> = {};
            const restoredStatus: Record<number, 'pending' | 'in-progress' | 'done'> = {};
            for (const [key, pts] of Object.entries(saved.snake_contours)) {
              const idx = Number(key);
              restoredSnake[idx] = pts;
              restoredStatus[idx] = saved.finalized[idx] ? 'done' : 'in-progress';
            }
            snakePoints = restoredSnake;
            statusMap = restoredStatus;
            console.log('Restored saved annotations');
          }
        } catch (err) {
          console.warn('Could not load saved annotations:', err);
        }
      }
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
    // Auto-save after finalization.
    autoSave();
  }

  async function autoSave() {
    if (!dicomPath) return;
    try {
      await saveAnnotations(dicomPath, centerlineMm);
    } catch (err) {
      console.warn('Auto-save failed:', err);
    }
  }

  async function handleSave() {
    if (saveBusy || !dicomPath) return;
    saveBusy = true;
    saveMsg = '';
    try {
      const path = await saveAnnotations(dicomPath, centerlineMm);
      saveMsg = 'Saved';
      setTimeout(() => { saveMsg = ''; }, 2000);
    } catch (err) {
      saveMsg = 'Save failed';
      console.error('Save failed:', err);
    } finally {
      saveBusy = false;
    }
  }

  async function handleExportCsv() {
    if (exportBusy || !mmdSummary) return;
    exportBusy = true;
    try {
      const csv = await exportMmdCsv(patientName);
      const blob = new Blob([csv], { type: 'text/csv' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${patientName}_mmd_surfaces.csv`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (err) {
      console.error('CSV export failed:', err);
    } finally {
      exportBusy = false;
    }
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

  async function handleUseWallAll() {
    if (mmdBusy) return;
    mmdBusy = true;
    mmdError = '';
    try {
      await useVesselWallAsContour({ all: true });
      // Reflect the bulk finalization in the local UI state.
      const newSnake: Record<number, [number, number][]> = { ...snakePoints };
      const newStatus: Record<number, 'pending' | 'in-progress' | 'done'> = { ...statusMap };
      for (let i = 0; i < targets.length; i++) {
        if (targets[i].vessel_wall.length > 0) {
          newSnake[i] = targets[i].vessel_wall.map(
            (p) => [p[0], p[1]] as [number, number],
          );
          newStatus[i] = 'done';
        }
      }
      snakePoints = newSnake;
      statusMap = newStatus;
      await autoSave();
    } catch (err) {
      mmdError = err instanceof Error ? err.message : String(err);
      console.error('Use wall (all) failed:', err);
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

<div class="flex h-full w-full flex-col overflow-hidden bg-surface">
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
    <div class="flex min-h-0 flex-1 overflow-hidden">
      <!-- Left: SnakeEditor for the selected cross-section -->
      <div class="flex w-1/2 shrink-0 flex-col overflow-hidden border-r border-border">
        {#if currentTarget}
          <SnakeEditor
            target={currentTarget}
            targetIndex={selectedIndex}
            snakePoints={currentSnake}
            onSnakeUpdate={handleSnakeUpdate}
            onFinalize={handleFinalize}
            status={currentStatus}
            {arcOffsetMm}
          />
        {/if}
      </div>

      <!-- Right: surface plot + radial profile placeholder -->
      <div class="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <SurfacePlotPanel
          {surfaces}
          selectedIndex={surfaceIndex}
          {material}
          {unit}
          onSliderChange={handleSurfaceSlider}
          {arcOffsetMm}
        />

        <!-- Radial profile placeholder -->
        <div class="shrink-0 border-t border-border px-3 py-2">
          <span class="text-[10px] text-text-secondary/60">Radial profile (coming soon)</span>
        </div>
      </div>
    </div>

    <!-- Cross-section strip (full width) -->
    <div class="shrink-0 border-t border-border bg-surface-secondary">
      <CrossSectionStrip
        {targets}
        {selectedIndex}
        {statusMap}
        onSelect={handleSelect}
        {arcOffsetMm}
      />
    </div>

    <!-- Bottom toolbar: overlay chips (left) + MMD actions (right) -->
    <div class="flex shrink-0 flex-wrap items-center gap-3 border-t border-border bg-surface-secondary px-3 py-1.5">
      <OverlaySelector
        {material}
        {unit}
        onMaterialChange={handleMaterialChange}
        onUnitChange={handleUnitChange}
      />

      <div class="ml-auto flex items-center gap-2">
        <button
          class="rounded bg-surface-tertiary px-3 py-1 text-xs font-medium text-text-primary hover:bg-surface-tertiary/80 active:bg-surface-tertiary/60 disabled:bg-surface-tertiary/40 disabled:text-text-secondary/70"
          onclick={handleUseWallAll}
          disabled={mmdBusy || targets.length === 0}
          title="Use the auto-detected vessel wall as the contour for every cross-section"
        >
          Use Wall (All)
        </button>
        <button
          class="rounded bg-accent/15 px-3 py-1 text-xs font-medium text-accent hover:bg-accent/25 active:bg-accent/35 disabled:bg-surface-tertiary/40 disabled:text-text-secondary/70"
          onclick={handleRunMmd}
          disabled={mmdBusy || finalizedCount === 0}
          title="Run PWSQS multi-material decomposition on finalized contours"
        >
          {mmdBusy ? 'Running MMD...' : 'Run MMD'}
        </button>

        <button
          class="rounded bg-surface-tertiary px-3 py-1 text-xs font-medium text-text-primary hover:bg-surface-tertiary/80 active:bg-surface-tertiary/60 disabled:bg-surface-tertiary/40 disabled:text-text-secondary/70"
          onclick={handleSave}
          disabled={saveBusy || targets.length === 0}
          title="Save annotation state for this patient"
        >
          {saveBusy ? 'Saving...' : 'Save'}
        </button>

        <button
          class="rounded bg-surface-tertiary px-3 py-1 text-xs font-medium text-text-primary hover:bg-surface-tertiary/80 active:bg-surface-tertiary/60 disabled:bg-surface-tertiary/40 disabled:text-text-secondary/70"
          onclick={handleExportCsv}
          disabled={exportBusy || !mmdSummary}
          title="Export MMD surface data as CSV"
        >
          {exportBusy ? 'Exporting...' : 'Export CSV'}
        </button>

        <span class="text-[11px] tabular-nums text-text-secondary">
          {finalizedCount}/{totalCount} done
        </span>

        {#if saveMsg}
          <span class="text-[10px] text-success">{saveMsg}</span>
        {/if}

        {#if mmdSummary}
          <span class="text-[10px] text-success">
            MMD {mmdSummary.converged ? 'converged' : 'done'} ({mmdSummary.n_voxels.toLocaleString()} voxels)
          </span>
        {/if}

        {#if mmdError}
          <span class="max-w-[24ch] truncate text-[10px] text-error">{mmdError}</span>
        {/if}
      </div>
    </div>
  {/if}
</div>
