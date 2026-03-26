<script lang="ts">
  /**
   * Bottom-right context panel in the 2x2 MPR grid.
   *
   * Displays content based on the current workflow phase:
   *   - 'empty'    -> prompt to load DICOM
   *   - 'dicom'    -> patient & volume metadata
   *   - 'seeds'    -> CPR compound view (straightened CPR + cross-sections)
   *   - 'analysis' -> FAI analysis dashboard with results
   */
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import CprView from './CprView.svelte';
  import AnalysisDashboard from './AnalysisDashboard.svelte';

  type Props = {
    phase: 'empty' | 'dicom' | 'seeds' | 'analysis';
    onNeedleMove?: (pos: [number, number, number]) => void;
  };

  let { phase, onNeedleMove }: Props = $props();

  let meta = $derived(volumeStore.current);
</script>

<div
  class="flex h-full w-full flex-col bg-surface-secondary text-text-primary"
  class:p-4={phase !== 'seeds' && phase !== 'analysis'}
>
  {#if phase === 'empty'}
    <div class="flex flex-1 flex-col items-center justify-center gap-2">
      <svg
        class="h-10 w-10 text-text-secondary/40"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        viewBox="0 0 24 24"
      >
        <path
          stroke-linecap="round"
          stroke-linejoin="round"
          d="M3.75 9.776c.112-.017.227-.026.344-.026h15.812c.117 0 .232.009.344.026m-16.5 0a2.25 2.25 0 0 0-1.883 2.542l.857 6a2.25 2.25 0 0 0 2.227 1.932H19.05a2.25 2.25 0 0 0 2.227-1.932l.857-6a2.25 2.25 0 0 0-1.883-2.542m-16.5 0V6A2.25 2.25 0 0 1 6 3.75h3.879a1.5 1.5 0 0 1 1.06.44l2.122 2.12a1.5 1.5 0 0 0 1.06.44H18A2.25 2.25 0 0 1 20.25 9v.776"
        />
      </svg>
      <p class="text-sm text-text-secondary">No DICOM loaded</p>
      <p class="text-xs text-text-secondary/60">
        Open a DICOM directory to begin
      </p>
    </div>
  {:else if phase === 'dicom' && meta}
    <div class="flex flex-col gap-3">
      <h3
        class="border-b border-border pb-2 text-xs font-semibold tracking-wider text-text-secondary"
      >
        PATIENT INFO
      </h3>

      <div class="flex flex-col gap-2 text-sm">
        <div>
          <span class="text-xs text-text-secondary">Patient Name</span>
          <p class="truncate text-text-primary">{meta.patientName || 'N/A'}</p>
        </div>

        <div>
          <span class="text-xs text-text-secondary">Study Description</span>
          <p class="truncate text-text-primary">
            {meta.studyDescription || 'N/A'}
          </p>
        </div>
      </div>

      <h3
        class="mt-2 border-b border-border pb-2 text-xs font-semibold tracking-wider text-text-secondary"
      >
        VOLUME
      </h3>

      <div class="flex flex-col gap-2 text-sm">
        <div>
          <span class="text-xs text-text-secondary">Dimensions (Z, Y, X)</span>
          <p class="font-mono text-xs text-text-primary">
            {meta.shape[0]} x {meta.shape[1]} x {meta.shape[2]}
          </p>
        </div>

        <div>
          <span class="text-xs text-text-secondary">Spacing (mm)</span>
          <p class="font-mono text-xs text-text-primary">
            {meta.spacing[0].toFixed(2)} x {meta.spacing[1].toFixed(2)} x {meta.spacing[2].toFixed(2)}
          </p>
        </div>

        <div>
          <span class="text-xs text-text-secondary">Window</span>
          <p class="font-mono text-xs text-text-primary">
            C: {meta.windowCenter} / W: {meta.windowWidth}
          </p>
        </div>
      </div>
    </div>
  {:else if phase === 'seeds'}
    <CprView {onNeedleMove} />
  {:else if phase === 'analysis'}
    <AnalysisDashboard />
  {:else}
    <div class="flex flex-1 items-center justify-center">
      <p class="text-sm text-text-secondary">Phase: {phase}</p>
    </div>
  {/if}
</div>
