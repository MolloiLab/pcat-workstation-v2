<script lang="ts">
  /**
   * Modal overlay shown during pipeline execution.
   *
   * Displays per-vessel progress bars (LAD, LCx, RCA) with current
   * stage names and overall status message. Semi-transparent dark
   * overlay centered in the viewport area.
   */
  import { pipelineStore } from '$lib/stores/pipelineStore.svelte';
  import { VESSEL_COLORS, type Vessel } from '$lib/stores/seedStore.svelte';

  const vesselOrder: Vessel[] = ['RCA', 'LAD', 'LCx'];

  let progressEntries = $derived(
    vesselOrder
      .filter((v) => v in pipelineStore.progress)
      .map((v) => ({
        vessel: v,
        color: VESSEL_COLORS[v],
        stage: pipelineStore.progress[v]?.stage ?? 'Queued',
        progress: pipelineStore.progress[v]?.progress ?? 0,
      })),
  );

  let overallProgress = $derived(
    progressEntries.length > 0
      ? progressEntries.reduce((sum, e) => sum + e.progress, 0) /
        progressEntries.length
      : 0,
  );
</script>

<div
  class="absolute inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
>
  <div
    class="flex w-80 flex-col gap-4 rounded-lg border border-border bg-surface-secondary p-6 shadow-2xl"
  >
    <!-- Header -->
    <div class="flex flex-col gap-1">
      <h3 class="text-sm font-semibold text-text-primary">
        Running FAI Pipeline
      </h3>
      <p class="text-xs text-text-secondary">
        Analyzing pericoronary adipose tissue...
      </p>
    </div>

    <!-- Per-vessel progress bars -->
    <div class="flex flex-col gap-3">
      {#each progressEntries as entry}
        <div class="flex flex-col gap-1">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <span
                class="h-2 w-2 rounded-full"
                style="background-color: {entry.color}"
              ></span>
              <span class="text-xs font-medium text-text-primary">
                {entry.vessel}
              </span>
            </div>
            <span class="text-[10px] text-text-secondary">
              {entry.stage}
            </span>
          </div>

          <!-- Progress bar -->
          <div class="h-1.5 w-full overflow-hidden rounded-full bg-surface-tertiary">
            <div
              class="h-full rounded-full transition-all duration-300 ease-out"
              style="width: {entry.progress * 100}%; background-color: {entry.color}"
            ></div>
          </div>

          <div class="text-right text-[10px] tabular-nums text-text-secondary/60">
            {(entry.progress * 100).toFixed(0)}%
          </div>
        </div>
      {/each}
    </div>

    <!-- Overall progress -->
    <div class="border-t border-border pt-3">
      <div class="flex items-center justify-between">
        <span class="text-xs text-text-secondary">Overall</span>
        <span class="text-xs tabular-nums text-text-primary">
          {(overallProgress * 100).toFixed(0)}%
        </span>
      </div>
      <div class="mt-1 h-1 w-full overflow-hidden rounded-full bg-surface-tertiary">
        <div
          class="h-full rounded-full bg-accent transition-all duration-300 ease-out"
          style="width: {overallProgress * 100}%"
        ></div>
      </div>
    </div>

    <!-- Error message (if any) -->
    {#if pipelineStore.error}
      <div class="rounded bg-error/10 px-3 py-2">
        <p class="text-xs text-error">{pipelineStore.error}</p>
      </div>
    {/if}
  </div>
</div>
