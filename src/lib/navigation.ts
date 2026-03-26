// Imperative navigation service — no reactive state, no $effect
type NavigateFn = (pos: [number, number, number]) => void;

let _navigate: NavigateFn | null = null;

export function registerNavigate(fn: NavigateFn) {
  _navigate = fn;
}

export function navigateToWorldPos(pos: [number, number, number]) {
  _navigate?.(pos);
}
