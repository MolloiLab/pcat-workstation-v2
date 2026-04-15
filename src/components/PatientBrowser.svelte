<script lang="ts">
  /**
   * Patient browser modal.
   *
   * Lists patient subfolders under a configurable root directory with status
   * badges derived from each patient's saved annotation JSON. Click a patient
   * to load it (caller wires in the load handler).
   */
  import { untrack } from 'svelte';
  import { listPatients, type PatientInfo } from '$lib/api';

  type Props = {
    /** Initial root directory (the user can edit it before scanning). */
    initialRootDir?: string;
    /** Called when the user picks a patient. Parent is responsible for loading. */
    onSelect: (path: string) => void;
    /** Close the browser without selecting. */
    onClose: () => void;
  };

  let {
    initialRootDir = '/Volumes/Molloilab/Shu Nie/UCI NAEOTOM CCTA Data',
    onSelect,
    onClose,
  }: Props = $props();

  // Editable local copy — prop is only used as the initial default.
  let rootDir = $state(untrack(() => initialRootDir));
  let patients = $state<PatientInfo[]>([]);
  let loading = $state(false);
  let errorMessage = $state('');

  /**
   * Filter: text query + status.
   * Default `all` so users see everything when they open the browser.
   */
  let query = $state('');
  let statusFilter = $state<'all' | 'not_started' | 'in_progress' | 'complete'>('all');

  async function refresh() {
    if (loading) return;
    loading = true;
    errorMessage = '';
    try {
      patients = await listPatients(rootDir);
    } catch (e) {
      errorMessage = e instanceof Error ? e.message : String(e);
      patients = [];
    } finally {
      loading = false;
    }
  }

  // Auto-load on mount.
  $effect(() => {
    refresh();
  });

  let filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return patients.filter((p) => {
      if (statusFilter !== 'all' && p.status !== statusFilter) return false;
      if (q && !p.id.toLowerCase().includes(q)) return false;
      return true;
    });
  });

  let counts = $derived.by(() => {
    let ns = 0, ip = 0, cp = 0;
    for (const p of patients) {
      if (p.status === 'not_started') ns++;
      else if (p.status === 'in_progress') ip++;
      else if (p.status === 'complete') cp++;
    }
    return { ns, ip, cp, total: patients.length };
  });

  function statusLabel(s: PatientInfo['status']): string {
    if (s === 'complete') return 'Done';
    if (s === 'in_progress') return 'In progress';
    return 'Not started';
  }

  function handleSelect(p: PatientInfo) {
    onSelect(p.path);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') onClose();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- Backdrop -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="fixed inset-0 z-40 flex items-start justify-center bg-black/60 pt-16"
  onclick={onClose}
>
  <!-- Modal -->
  <div
    class="flex max-h-[80vh] w-[680px] flex-col overflow-hidden rounded-lg border border-border bg-surface-secondary shadow-2xl"
    onclick={(e: MouseEvent) => e.stopPropagation()}
  >
    <!-- Header -->
    <div class="flex shrink-0 items-center justify-between border-b border-border px-4 py-3">
      <h2 class="text-sm font-semibold text-text-primary">Patient Browser</h2>
      <button
        class="rounded px-2 py-0.5 text-xs text-text-secondary hover:bg-surface hover:text-text-primary"
        onclick={onClose}
        title="Close (Esc)"
      >
        ✕
      </button>
    </div>

    <!-- Root dir + refresh -->
    <div class="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2">
      <input
        type="text"
        class="flex-1 rounded border border-border bg-surface px-2 py-1 text-xs text-text-primary focus:border-accent focus:outline-none"
        bind:value={rootDir}
        placeholder="Patient root directory"
        onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') refresh(); }}
      />
      <button
        class="rounded bg-accent/10 px-3 py-1 text-xs font-medium text-accent hover:bg-accent/20 disabled:opacity-40"
        onclick={refresh}
        disabled={loading}
      >
        {loading ? 'Scanning...' : 'Refresh'}
      </button>
    </div>

    <!-- Filter row -->
    <div class="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2">
      <input
        type="text"
        class="flex-1 rounded border border-border bg-surface px-2 py-1 text-xs text-text-primary focus:border-accent focus:outline-none"
        bind:value={query}
        placeholder="Filter by ID..."
      />
      <select
        class="rounded border border-border bg-surface px-2 py-1 text-xs text-text-primary focus:border-accent focus:outline-none"
        bind:value={statusFilter}
      >
        <option value="all">All ({counts.total})</option>
        <option value="not_started">Not started ({counts.ns})</option>
        <option value="in_progress">In progress ({counts.ip})</option>
        <option value="complete">Done ({counts.cp})</option>
      </select>
    </div>

    <!-- Body -->
    <div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      {#if errorMessage}
        <div class="px-4 py-3 text-xs text-error">
          {errorMessage}
        </div>
      {:else if loading && patients.length === 0}
        <div class="px-4 py-6 text-center text-xs text-text-secondary">
          Scanning {rootDir}...
        </div>
      {:else if filtered.length === 0}
        <div class="px-4 py-6 text-center text-xs text-text-secondary">
          {patients.length === 0 ? 'No patient folders found.' : 'No patients match the filter.'}
        </div>
      {:else}
        {#each filtered as p (p.id)}
          <button
            class="flex items-center justify-between border-b border-border/60 px-4 py-2 text-left hover:bg-accent/10 active:bg-accent/20"
            onclick={() => handleSelect(p)}
          >
            <div class="flex flex-col gap-0.5 min-w-0">
              <span class="text-xs font-medium text-text-primary">{p.id}</span>
              <span class="text-[10px] text-text-secondary truncate" title={p.path}>{p.path}</span>
            </div>
            <div class="flex shrink-0 items-center gap-3">
              {#if p.finalized_count > 0}
                <span class="text-[10px] tabular-nums text-text-secondary">
                  {p.finalized_count} contour{p.finalized_count === 1 ? '' : 's'}{p.has_mmd ? ' · MMD' : ''}
                </span>
              {/if}
              <span
                class="rounded px-2 py-0.5 text-[10px] font-medium {p.status === 'complete'
                  ? 'bg-success/15 text-success'
                  : p.status === 'in_progress'
                    ? 'bg-warning/15 text-warning'
                    : 'bg-text-secondary/15 text-text-secondary'}"
              >
                {statusLabel(p.status)}
              </span>
            </div>
          </button>
        {/each}
      {/if}
    </div>

    <!-- Footer -->
    <div class="flex shrink-0 items-center justify-between border-t border-border px-4 py-2 text-[10px] text-text-secondary">
      <span>{counts.total} patients · {counts.cp} done · {counts.ip} in progress</span>
      <span>Esc to close</span>
    </div>
  </div>
</div>
