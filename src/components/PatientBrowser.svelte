<script lang="ts">
  /**
   * Patient browser modal.
   *
   * Lists patient subfolders under a configurable root directory with status
   * badges derived from each patient's saved annotation JSON. Click a patient
   * to load it (caller wires in the load handler).
   */
  import { untrack } from 'svelte';
  import {
    listPatients,
    listSeriesDirs,
    type PatientInfo,
    type SeriesDirInfo,
  } from '$lib/api';

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

  // Patient-level click is handled inline by toggleExpand; series picks call
  // onSelect via handleSelectSeries. No separate single-click handler needed.

  /**
   * Per-patient expansion state and series cache.
   *
   * A patient folder typically contains several series subfolders (e.g.
   * `MonoPlus_70keV`, `MonoPlus_150keV`, `CCTA_*`). The DICOM loader expects a
   * single-series folder, so we let the user expand a patient row and pick
   * the specific series to load.
   */
  let expanded = $state<Record<string, boolean>>({});
  let seriesCache = $state<Record<string, SeriesDirInfo[]>>({});
  let seriesLoading = $state<Record<string, boolean>>({});
  let seriesError = $state<Record<string, string>>({});

  async function toggleExpand(p: PatientInfo) {
    const key = p.id;
    expanded[key] = !expanded[key];
    if (expanded[key] && !seriesCache[key] && !seriesLoading[key]) {
      seriesLoading[key] = true;
      seriesError[key] = '';
      try {
        seriesCache[key] = await listSeriesDirs(p.path);
      } catch (e) {
        seriesError[key] = e instanceof Error ? e.message : String(e);
      } finally {
        seriesLoading[key] = false;
      }
    }
  }

  function handleSelectSeries(_p: PatientInfo, s: SeriesDirInfo) {
    onSelect(s.path);
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
          {@const isOpen = !!expanded[p.id]}
          <div class="border-b border-border/60">
            <!-- Patient row: click expands to show series subfolders -->
            <button
              class="flex w-full items-center justify-between px-4 py-2 text-left hover:bg-accent/10 active:bg-accent/20"
              onclick={() => toggleExpand(p)}
            >
              <div class="flex items-center gap-2 min-w-0">
                <span class="inline-block w-4 text-center text-sm font-bold text-accent">
                  {isOpen ? '▾' : '▸'}
                </span>
                <div class="flex flex-col gap-0.5 min-w-0">
                  <span class="text-xs font-medium text-text-primary">{p.id}</span>
                  <span class="text-[10px] text-text-secondary truncate" title={p.path}>
                    {isOpen ? 'click a series below to load' : 'click to expand series'}
                  </span>
                </div>
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

            <!-- Series subfolders, shown when expanded -->
            {#if isOpen}
              <div class="bg-surface/40 px-6 pb-2">
                {#if seriesLoading[p.id]}
                  <div class="py-2 text-[11px] text-text-secondary">Listing series...</div>
                {:else if seriesError[p.id]}
                  <div class="py-2 text-[11px] text-error">{seriesError[p.id]}</div>
                {:else if (seriesCache[p.id]?.length ?? 0) === 0}
                  <div class="py-2 text-[11px] text-text-secondary">No series subfolders.</div>
                {:else}
                  {#each seriesCache[p.id]! as s (s.path)}
                    <button
                      class="flex w-full items-center justify-between rounded px-2 py-1 text-left hover:bg-accent/10 active:bg-accent/20"
                      onclick={() => handleSelectSeries(p, s)}
                    >
                      <span class="text-[11px] text-text-primary truncate" title={s.path}>{s.name}</span>
                      <span class="text-[10px] tabular-nums text-text-secondary">
                        {s.num_files} {s.num_files === 1 ? 'file' : 'files'}
                      </span>
                    </button>
                  {/each}
                {/if}
              </div>
            {/if}
          </div>
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
