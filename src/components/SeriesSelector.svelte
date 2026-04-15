<script lang="ts">
  /**
   * Modal dialog for selecting dual-energy DICOM series.
   *
   * Displays detected series with keV labels and slice counts,
   * lets the user assign Low/High energy roles, then loads
   * both volumes via the Rust backend.
   */
  import { loadDualEnergy, type SeriesInfo, type DualEnergyInfo } from '$lib/api';

  interface Props {
    seriesList: SeriesInfo[];
    dicomPath: string;
    onClose: () => void;
    onLoaded: (info: DualEnergyInfo) => void;
  }

  let { seriesList, dicomPath, onClose, onLoaded }: Props = $props();

  let lowUid = $state<string | null>(null);
  let highUid = $state<string | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Auto-assign: lowest keV -> low, highest keV -> high
  $effect(() => {
    const withKev = seriesList.filter((s) => s.kev_label !== null);
    if (withKev.length >= 2) {
      const sorted = [...withKev].sort((a, b) => a.kev_label! - b.kev_label!);
      lowUid = sorted[0].series_uid;
      highUid = sorted[sorted.length - 1].series_uid;
    }
  });

  let canLoad = $derived(
    lowUid !== null && highUid !== null && lowUid !== highUid && !loading,
  );

  function kevFor(uid: string | null): number {
    if (!uid) return 0;
    const s = seriesList.find((s) => s.series_uid === uid);
    return s?.kev_label ?? 0;
  }

  async function handleLoad() {
    if (!lowUid || !highUid) return;
    loading = true;
    error = null;
    try {
      const info = await loadDualEnergy(
        dicomPath,
        lowUid,
        highUid,
        kevFor(lowUid),
        kevFor(highUid),
      );
      onLoaded(info);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function labelFor(uid: string): string {
    if (uid === lowUid && uid === highUid) return ''; // shouldn't happen
    if (uid === lowUid) return 'LOW';
    if (uid === highUid) return 'HIGH';
    return '';
  }
</script>

<!-- Backdrop -->
<div
  class="absolute inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
  role="dialog"
  aria-modal="true"
  aria-label="Select dual-energy series"
>
  <!-- Modal card -->
  <div
    class="flex w-[480px] max-h-[80vh] flex-col rounded-lg border border-border bg-surface-secondary shadow-2xl"
  >
    <!-- Header -->
    <div class="flex items-center justify-between border-b border-border px-5 py-3.5">
      <h3 class="text-sm font-semibold text-text-primary">
        Select Dual-Energy Series
      </h3>
      <button
        class="rounded p-1 text-text-secondary hover:bg-surface-tertiary hover:text-text-primary"
        onclick={onClose}
        title="Close"
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="18" y1="6" x2="6" y2="18"></line>
          <line x1="6" y1="6" x2="18" y2="18"></line>
        </svg>
      </button>
    </div>

    <!-- Series table -->
    <div class="flex-1 overflow-y-auto px-5 py-4">
      <table class="w-full text-xs">
        <thead>
          <tr class="text-left text-text-secondary">
            <th class="pb-2 pr-3 font-medium">Series</th>
            <th class="pb-2 pr-3 font-medium text-right w-16">Slices</th>
            <th class="pb-2 pr-3 font-medium text-right w-16">keV</th>
            <th class="pb-2 font-medium text-center w-14">Role</th>
          </tr>
        </thead>
        <tbody>
          {#each seriesList as series}
            {@const isLow = series.series_uid === lowUid}
            {@const isHigh = series.series_uid === highUid}
            {@const label = labelFor(series.series_uid)}
            <tr
              class="border-t border-border/50 transition-colors"
              class:bg-accent/10={isLow || isHigh}
            >
              <td class="py-2 pr-3 text-text-primary truncate max-w-[240px]" title={series.description}>
                {series.description || series.series_uid}
              </td>
              <td class="py-2 pr-3 text-right tabular-nums text-text-secondary">
                {series.num_slices}
              </td>
              <td class="py-2 pr-3 text-right tabular-nums text-text-secondary">
                {series.kev_label !== null ? series.kev_label.toFixed(0) : '--'}
              </td>
              <td class="py-2 text-center">
                {#if label === 'LOW'}
                  <span class="rounded px-1.5 py-0.5 text-[10px] font-semibold bg-accent/20 text-accent">
                    LOW
                  </span>
                {:else if label === 'HIGH'}
                  <span class="rounded px-1.5 py-0.5 text-[10px] font-semibold bg-warning/20 text-warning">
                    HIGH
                  </span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <!-- Assignment controls -->
    <div class="flex flex-col gap-3 border-t border-border px-5 py-4">
      <div class="flex items-center gap-3">
        <label class="text-xs text-text-secondary w-20 shrink-0">Low Energy</label>
        <select
          class="flex-1 rounded border border-border bg-surface-tertiary px-2.5 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
          bind:value={lowUid}
          disabled={loading}
        >
          <option value={null}>-- select --</option>
          {#each seriesList as series}
            <option value={series.series_uid}>
              {series.description || series.series_uid}
              {series.kev_label !== null ? ` (${series.kev_label} keV)` : ''}
            </option>
          {/each}
        </select>
      </div>

      <div class="flex items-center gap-3">
        <label class="text-xs text-text-secondary w-20 shrink-0">High Energy</label>
        <select
          class="flex-1 rounded border border-border bg-surface-tertiary px-2.5 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
          bind:value={highUid}
          disabled={loading}
        >
          <option value={null}>-- select --</option>
          {#each seriesList as series}
            <option value={series.series_uid}>
              {series.description || series.series_uid}
              {series.kev_label !== null ? ` (${series.kev_label} keV)` : ''}
            </option>
          {/each}
        </select>
      </div>

      {#if lowUid && highUid && lowUid === highUid}
        <p class="text-[11px] text-warning">Low and High must be different series.</p>
      {/if}
    </div>

    <!-- Error -->
    {#if error}
      <div class="mx-5 mb-3 rounded bg-error/10 px-3 py-2">
        <p class="text-xs text-error">{error}</p>
      </div>
    {/if}

    <!-- Footer buttons -->
    <div class="flex items-center justify-end gap-2 border-t border-border px-5 py-3">
      <button
        class="rounded px-3 py-1.5 text-xs text-text-secondary hover:bg-surface-tertiary hover:text-text-primary transition-colors"
        onclick={onClose}
        disabled={loading}
      >
        Cancel
      </button>
      <button
        class="rounded px-3 py-1.5 text-xs font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
        class:bg-accent={canLoad}
        class:text-white={canLoad}
        class:hover:bg-accent-hover={canLoad}
        class:bg-surface-tertiary={!canLoad}
        class:text-text-secondary={!canLoad}
        onclick={handleLoad}
        disabled={!canLoad}
      >
        {#if loading}
          <span class="inline-flex items-center gap-1.5">
            <svg class="h-3 w-3 animate-spin" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
              <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
              <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
            </svg>
            Loading...
          </span>
        {:else}
          Load Dual Energy
        {/if}
      </button>
    </div>
  </div>
</div>
