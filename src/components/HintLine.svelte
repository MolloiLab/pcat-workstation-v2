<script lang="ts">
  /**
   * Contextual hint line at the bottom of the main viewport area.
   *
   * Shows context-sensitive guidance text that fades in when the state
   * changes and fades out after 3.5 seconds. Learns to stop showing
   * placement hints once the user has placed 3+ seeds for the current vessel.
   */
  import { seedStore } from '$lib/stores/seedStore.svelte';
  import { volumeStore } from '$lib/stores/volumeStore.svelte';
  import { pipelineStore } from '$lib/stores/pipelineStore.svelte';

  const VESSEL_LABELS: Record<string, string> = {
    RCA: 'RCA',
    LAD: 'LAD',
    LCx: 'LCx',
  };

  let visible = $state(false);
  let fadeTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * Derive the hint text from the current application state.
   */
  let hintText = $derived.by(() => {
    // No volume loaded — no hints
    if (!volumeStore.current) return '';

    const vessel = seedStore.activeVessel;
    const data = seedStore.activeVesselData;
    const seedCount = data.seeds.length;
    const selected = seedStore.selectedSeedIndex;

    // Seed selected — show selection actions
    if (selected !== null) {
      return 'Drag to move \u00b7 Del to remove \u00b7 Esc to deselect \u00b7 Arrow keys to cycle';
    }

    // After enough seeds: prompt for ostium if not set, or show ready state
    if (seedCount >= 5) {
      const hasOstium = data.ostiumFraction !== null;
      if (!hasOstium) {
        return 'Scroll CPR to the ostium (where coronary exits aorta) \u00b7 Click "Set Ostium" in the CPR toolbar';
      }
      if (pipelineStore.canRun && pipelineStore.status === 'idle') {
        return 'Ready to analyze \u00b7 Click "Analyze" to measure FAI';
      }
      return '';
    }

    // No seeds — prompt to start in aorta
    if (seedCount === 0) {
      return `Start in the aorta, then trace into ${VESSEL_LABELS[vessel]} \u00b7 Click on any MPR view to place points`;
    }

    // 1 seed — continue tracing
    if (seedCount === 1) {
      return 'Continue clicking along the vessel to add waypoints';
    }

    // 2-4 seeds — show CPR hints
    if (seedCount >= 2) {
      return 'CPR preview ready \u00b7 Keep adding points, or Shift+click on CPR to mark ostium';
    }

    return '';
  });

  /**
   * Track a "state key" that changes whenever the hint should re-trigger.
   * When it changes, show the hint and reset the fade timer.
   */
  let stateKey = $derived(
    `${seedStore.activeVessel}:${seedStore.activeVesselData.seeds.length}:${seedStore.selectedSeedIndex}:${seedStore.activeVesselData.ostiumFraction}`
  );

  // React to state changes: show hint and start fade timer
  $effect(() => {
    // Read stateKey to subscribe
    void stateKey;

    if (hintText) {
      visible = true;
      resetFadeTimer();
    } else {
      visible = false;
    }
  });

  function resetFadeTimer() {
    if (fadeTimer) clearTimeout(fadeTimer);
    fadeTimer = setTimeout(() => {
      visible = false;
    }, 8000);
  }

  /**
   * When mouse moves over the viewport area, re-show relevant hints.
   * This is called by the parent via binding or by listening on the
   * main area. We listen on the component's own wrapper instead.
   */
  function handleMouseActivity() {
    if (hintText && !visible) {
      visible = true;
      resetFadeTimer();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="absolute top-0 left-0 right-0 z-20 flex h-7 items-center justify-center bg-black/50 backdrop-blur-sm transition-opacity"
  class:opacity-0={!visible || !hintText}
  class:opacity-100={visible && !!hintText}
  class:duration-200={visible && !!hintText}
  class:duration-400={!visible || !hintText}
  onmousemove={handleMouseActivity}
>
  {#if hintText}
    <span class="text-xs text-white/80">{hintText}</span>
  {/if}
</div>
