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
  import { openDicomDialog, loadDicom } from '$lib/api';
  import { loadVolume } from '$lib/cornerstone/volumeLoader';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import type { VolumeMetadata } from '$lib/stores/volumeStore.svelte';
  import { pipelineStore } from '$lib/stores/pipelineStore.svelte';
  import { seedStore, type Vessel } from '$lib/stores/seedStore.svelte';

  let errorMessage = $state('');

  // ---- Keyboard shortcuts ----
  function handleKeydown(event: KeyboardEvent) {
    // Ignore if user is typing in an input/textarea
    const tag = (event.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;

    // Ctrl+Z / Cmd+Z: undo last seed
    if ((event.ctrlKey || event.metaKey) && event.key === 'z') {
      event.preventDefault();
      const data = seedStore.activeVesselData;
      if (data.seeds.length > 0) {
        seedStore.removeSeed(data.seeds.length - 1);
      }
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

    // 1/2/3: switch active vessel
    const vesselMap: Record<string, Vessel> = { '1': 'RCA', '2': 'LAD', '3': 'LCx' };
    if (vesselMap[event.key]) {
      seedStore.setActiveVessel(vesselMap[event.key]);
      return;
    }
  }

  /** Open DICOM folder, load into Rust backend, then into cornerstone3D. */
  async function handleOpenDicom() {
    errorMessage = '';

    try {
      // 1. Native folder picker
      const path = await openDicomDialog();
      if (!path) return; // user cancelled

      // 2. Begin loading
      volumeStore.setLoading(true);
      volumeStore.setLoadProgress(0);

      // 3. Rust loads DICOM directory -> returns snake_case metadata
      const info = await loadDicom(path);

      // 4. Map Rust snake_case -> TS camelCase and store metadata
      const meta: VolumeMetadata = {
        volumeId: 'vol-1',
        shape: info.shape,
        spacing: info.spacing,
        origin: info.origin,
        direction: info.direction,
        windowCenter: info.window_center,
        windowWidth: info.window_width,
        patientName: info.patient_name,
        studyDescription: info.study_description,
      };
      volumeStore.set(meta);

      // 5. Fetch all slices into cornerstone3D volume (with progress)
      const csId = await loadVolume(meta, (p) => volumeStore.setLoadProgress(p));

      // 6. Set cornerstone volume ID -> triggers MprPanel $effect
      volumeStore.setCornerstoneVolumeId(csId);

      // 7. Done
      volumeStore.setLoading(false);
    } catch (e) {
      volumeStore.setLoading(false);
      errorMessage = e instanceof Error ? e.message : String(e);
      console.error('Failed to load DICOM:', e);
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

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
          class="rounded bg-success/10 px-3 py-1 text-xs font-medium text-success hover:bg-success/20"
          onclick={() => pipelineStore.reset()}
        >
          View Results
        </button>
      {:else if pipelineStore.canRun}
        <button
          class="rounded px-3 py-1 text-xs font-medium text-accent hover:bg-accent/10 active:bg-accent/20 disabled:opacity-40"
          onclick={() => pipelineStore.run()}
          disabled={pipelineStore.status === 'running'}
        >
          {pipelineStore.status === 'running' ? 'Running...' : 'Run Pipeline'}
        </button>
      {/if}

      <button
        class="rounded px-3 py-1 text-xs font-medium text-accent hover:bg-accent/10 active:bg-accent/20 disabled:opacity-40"
        onclick={handleOpenDicom}
        disabled={volumeStore.loading}
      >
        Open DICOM
      </button>
      <button
        class="rounded px-3 py-1 text-xs text-text-secondary hover:bg-surface-tertiary hover:text-text-primary"
      >
        Settings
      </button>
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
