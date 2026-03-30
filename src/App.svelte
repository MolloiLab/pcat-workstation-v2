<script lang="ts">
  /**
   * Root application shell.
   *
   * Layout:
   *   - Header toolbar (title + actions)
   *   - Main area    (MprPanel fills remaining space)
   *   - Footer status bar (loading progress / ready state)
   */
  import MprPanel from './components/MprPanel.svelte';
  import SeedToolbar from './components/SeedToolbar.svelte';
  import HintLine from './components/HintLine.svelte';
  import ProgressOverlay from './components/ProgressOverlay.svelte';
  import { openDicomDialog, loadDicom, getRecentDicoms, loadSeeds } from '$lib/api';
  import { loadVolume } from '$lib/cornerstone/volumeLoader';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import type { VolumeMetadata } from '$lib/stores/volumeStore.svelte';
  import { pipelineStore } from '$lib/stores/pipelineStore.svelte';
  import { seedStore, type Vessel } from '$lib/stores/seedStore.svelte';
  import { navigateToWorldPos } from '$lib/navigation';

  let errorMessage = $state('');
  let recentPaths = $state<string[]>([]);
  let showRecent = $state(false);

  // Load recent paths on mount
  $effect(() => {
    getRecentDicoms().then((paths) => { recentPaths = paths; }).catch(() => {});
  });

  // ---- Keyboard shortcuts ----
  function handleKeydown(event: KeyboardEvent) {
    // Ignore if user is typing in an input/textarea
    const tag = (event.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;

    // Cmd+Shift+Z / Ctrl+Shift+Z: redo
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key === 'z') {
      event.preventDefault();
      seedStore.redo();
      return;
    }

    // Cmd+Z / Ctrl+Z: undo
    if ((event.ctrlKey || event.metaKey) && event.key === 'z') {
      event.preventDefault();
      seedStore.undo();
      return;
    }

    // Backspace / Delete: if seed selected, delete selected; else delete last
    if (event.key === 'Backspace' || event.key === 'Delete') {
      event.preventDefault();
      const selected = seedStore.selectedSeedIndex;
      if (selected !== null) {
        seedStore.removeSeed(selected);
      } else {
        const data = seedStore.activeVesselData;
        if (data.seeds.length > 0) {
          seedStore.removeSeed(data.seeds.length - 1);
        }
      }
      return;
    }

    // Escape: if seed selected, deselect; else clear vessel
    if (event.key === 'Escape') {
      if (seedStore.selectedSeedIndex !== null) {
        seedStore.deselectSeed();
      } else {
        seedStore.clearVessel(seedStore.activeVessel);
      }
      return;
    }

    // Arrow Left/Right: cycle through seeds
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      const data = seedStore.activeVesselData;
      if (data.seeds.length === 0) return;
      const current = seedStore.selectedSeedIndex;
      let next: number;
      if (current === null) {
        next = event.key === 'ArrowRight' ? 0 : data.seeds.length - 1;
      } else {
        next = event.key === 'ArrowRight'
          ? (current + 1) % data.seeds.length
          : (current - 1 + data.seeds.length) % data.seeds.length;
      }
      seedStore.selectSeed(next);
      navigateToWorldPos(data.seeds[next].position);
      return;
    }

    // 1/2/3: switch active vessel
    const vesselMap: Record<string, Vessel> = { '1': 'RCA', '2': 'LAD', '3': 'LCx' };
    if (vesselMap[event.key]) {
      seedStore.setActiveVessel(vesselMap[event.key]);
      return;
    }
  }

  /** Counter for unique volume IDs across loads. */
  let volumeCounter = 0;

  /** Load DICOM from a specific folder path. */
  async function loadFromPath(path: string) {
    errorMessage = '';
    showRecent = false;

    try {
      // Clear previous state
      seedStore.clearAll();
      volumeStore.clear();

      volumeStore.setLoading(true);
      volumeStore.setLoadProgress(0);

      const info = await loadDicom(path);

      volumeCounter++;
      const meta: VolumeMetadata = {
        volumeId: `vol-${volumeCounter}`,
        shape: info.shape,
        spacing: info.spacing,
        origin: info.origin,
        direction: info.direction,
        windowCenter: info.window_center,
        windowWidth: info.window_width,
        patientName: info.patient_name,
        studyDescription: info.study_description,
        dicomPath: path,
      };
      volumeStore.set(meta);

      const csId = await loadVolume(meta, (p) => volumeStore.setLoadProgress(p));
      volumeStore.setCornerstoneVolumeId(csId);
      volumeStore.setLoading(false);

      // Auto-load seeds for this patient
      try {
        const seedsJson = await loadSeeds(path);
        if (seedsJson) {
          seedStore.importJson(seedsJson);
        }
      } catch { /* no saved seeds for this patient */ }

      // Refresh recent list
      getRecentDicoms().then((paths) => { recentPaths = paths; }).catch(() => {});
    } catch (e) {
      volumeStore.setLoading(false);
      errorMessage = e instanceof Error ? e.message : String(e);
      console.error('Failed to load DICOM:', e);
    }
  }

  /** Open DICOM folder picker, then load. */
  async function handleOpenDicom() {
    const path = await openDicomDialog();
    if (!path) return;
    loadFromPath(path);
  }
</script>

<svelte:window onkeydown={handleKeydown} onclick={() => { showRecent = false; }} />

<div class="flex h-screen flex-col">
  <!-- ===== Header toolbar ===== -->
  <header
    class="flex h-11 shrink-0 items-center justify-between border-b border-border bg-surface-secondary px-4"
  >
    <div class="flex items-center gap-2">
      <h1 class="text-sm font-semibold tracking-wide text-text-primary">
        PCAT Workstation
      </h1>
      <span class="text-[11px] text-text-secondary">v2.0-dev</span>
    </div>

    <!-- Seed vessel selector (visible after volume load) -->
    {#if volumeStore.current}
      <div class="flex items-center gap-3">
        <SeedToolbar />
      </div>
    {/if}

    <div class="flex items-center gap-1.5">
      <!-- Pipeline action button -->
      {#if pipelineStore.status === 'complete'}
        <button
          class="rounded bg-accent/10 px-3 py-1 text-xs font-medium text-accent hover:bg-accent/20"
          onclick={() => { pipelineStore.run(); }}
          title="Re-run: centerline → contour extraction → CRISP-CT VOI (1mm gap + 3mm ring) → FAI stats"
        >
          Re-analyze
        </button>
      {:else if pipelineStore.canRun}
        <button
          class="rounded px-3 py-1 text-xs font-medium text-accent hover:bg-accent/10 active:bg-accent/20 disabled:opacity-40"
          onclick={() => pipelineStore.run()}
          disabled={pipelineStore.status === 'running'}
          title="Run FAI pipeline: centerline → contour extraction → CRISP-CT VOI (1mm gap + 3mm ring) → FAI stats"
        >
          {pipelineStore.status === 'running' ? 'Analyzing...' : 'Analyze'}
        </button>
      {/if}

      <div class="relative flex items-center">
        <button
          class="rounded-l px-3 py-1 text-xs font-medium text-accent hover:bg-accent/10 active:bg-accent/20 disabled:opacity-40"
          onclick={handleOpenDicom}
          disabled={volumeStore.loading}
        >
          Open DICOM
        </button>
        {#if recentPaths.length > 0}
          <button
            class="rounded-r border-l border-border px-1.5 py-1 text-xs text-accent hover:bg-accent/10 disabled:opacity-40"
            onclick={(e: MouseEvent) => { e.stopPropagation(); showRecent = !showRecent; }}
            disabled={volumeStore.loading}
            title="Recent files"
          >
            &#9662;
          </button>
        {/if}

        <!-- Recent files dropdown -->
        {#if showRecent && recentPaths.length > 0}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="absolute right-0 top-full z-50 mt-1 max-h-64 w-80 overflow-y-auto rounded border border-border bg-surface-secondary shadow-lg"
          >
            <div class="px-3 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-secondary/60">
              Recent
            </div>
            {#each recentPaths as rp}
              <button
                class="w-full px-3 py-1.5 text-left text-[11px] text-text-primary hover:bg-accent/10 truncate"
                onclick={() => loadFromPath(rp)}
                title={rp}
              >
                {rp.split('/').slice(-2).join('/')}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </header>

  <!-- ===== Main viewport area ===== -->
  <main class="relative min-h-0 flex-1">
    <MprPanel />

    <!-- Contextual hint line -->
    <HintLine />

    <!-- Pipeline progress overlay -->
    {#if pipelineStore.status === 'running'}
      <ProgressOverlay />
    {/if}
  </main>

  <!-- ===== Footer status bar ===== -->
  <footer
    class="flex h-6 shrink-0 items-center justify-between border-t border-border bg-surface-secondary px-4"
  >
    <div class="flex items-center gap-2">
      {#if volumeStore.loading}
        <!-- Loading progress bar -->
        <div class="flex items-center gap-2">
          <div class="h-1.5 w-28 overflow-hidden rounded-full bg-surface-tertiary">
            <div
              class="h-full rounded-full bg-accent transition-all duration-150 ease-out"
              style="width: {volumeStore.loadProgress}%"
            ></div>
          </div>
          <span class="text-[11px] tabular-nums text-text-secondary">
            Loading volume... {volumeStore.loadProgress}%
          </span>
        </div>
      {:else if pipelineStore.status === 'error'}
        <span class="h-1.5 w-1.5 rounded-full bg-error"></span>
        <span class="truncate text-[11px] text-error">Analysis: {pipelineStore.error}</span>
      {:else if errorMessage}
        <span class="h-1.5 w-1.5 rounded-full bg-error"></span>
        <span class="truncate text-[11px] text-error">{errorMessage}</span>
      {:else if volumeStore.current}
        <span class="h-1.5 w-1.5 rounded-full bg-success"></span>
        <span class="text-[11px] text-text-secondary">Volume loaded</span>
      {:else}
        <span class="h-1.5 w-1.5 rounded-full bg-text-secondary/40"></span>
        <span class="text-[11px] text-text-secondary">Ready</span>
      {/if}
    </div>
    <span class="text-[11px] text-text-secondary/60">Rust backend</span>
  </footer>
</div>
