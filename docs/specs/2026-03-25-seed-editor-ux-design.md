# Seed Editor UX — Design Spec

## Overview

Redesign the seed placement interaction in PCAT Workstation v2 to be intuitive, modeless, and self-documenting. Replace the current "click to append only" model with a three-zone click detection system, drag-to-move, and a contextual hint line that fades away as the user gains confidence.

## Three-Zone Click Detection

Every click on an MPR viewport goes through priority-ordered proximity detection:

| Priority | Zone | Condition | Action |
|---|---|---|---|
| 1 | **Seed** | Click within 8px of existing seed marker | Select that seed (glow ring) |
| 2 | **Centerline** | Click within 6px of centerline polyline | Insert new waypoint at nearest position between two existing seeds |
| 3 | **Empty space** | Far from both | Append new waypoint at end of active vessel |

First click on empty space for a vessel always creates the **ostium**. Subsequent clicks create **waypoints**.

## Selected Seed Operations

| Input | Action |
|---|---|
| Drag | Move seed in real-time (spline + CPR update live) |
| Backspace / Delete | Delete selected seed |
| Escape | Deselect |
| Click empty space | Deselect + place new waypoint |
| Click another seed | Switch selection |

## Global Keyboard Shortcuts

| Input | Action |
|---|---|
| 1 / 2 / 3 | Switch vessel (RCA / LAD / LCx) |
| Ctrl+Z / Cmd+Z | Undo last action |
| Escape (no selection) | Clear active vessel's seeds |

## Visual Feedback

| State | Appearance |
|---|---|
| Normal seed | Filled shape (circle=waypoint, square=ostium), vessel color, 8px |
| Selected seed | Same + white glow ring + 10px |
| Hover near centerline | Cursor → crosshair, ghost dot at insertion point (vessel color, 50% opacity) |
| Dragging | Seed follows mouse, centerline redraws in real-time |

## Contextual Hint Line

Single shared hint line at the bottom of the main viewport area (above the status bar). Behaves like a video player overlay.

### Appearance
- Full-width translucent strip: `bg-black/50 backdrop-blur-sm`
- Small white text, 11px
- Height: ~24px
- Position: absolute bottom of the main area, overlaying the viewport grid

### Behavior
- **Fades in** (200ms) when state changes (new seed placed, vessel switched, seed selected)
- **Fades out** (400ms) after 3-4 seconds of no interaction
- **Reappears** when workflow state changes
- **Learns**: once user has placed 3+ seeds for a vessel, stop showing placement hints (user clearly knows the workflow)

### Messages by State

| State | Hint text |
|---|---|
| Volume loaded, no seeds | "Click on any view to place ostium for **RCA**" |
| Has ostium, no waypoints | "Click to add waypoints along the vessel" |
| Has 2+ seeds | "Click to add more waypoints · Click on centerline to insert" |
| Seed selected | "Drag to move · Backspace to delete · Escape to deselect" |
| Hovering near centerline | "Click to insert waypoint here" |
| Experienced (3+ seeds placed) | (hidden — no hint shown) |

### Fade Logic
```
on state change:
  show hint with fade-in
  reset 3.5s timer

on timer expire:
  fade out

on mouse move over viewport area:
  if hint is relevant and hidden:
    show with fade-in
    reset timer
```

## Files to Modify

| File | Change |
|---|---|
| `src/lib/stores/seedStore.svelte.ts` | Add `selectedSeedIndex`, `selectSeed()`, `deselectSeed()`, `insertSeedAt()`, `moveSeed()` with drag support |
| `src/components/SliceViewport.svelte` | Three-zone click detection, drag handling, hover detection near centerline |
| `src/components/SeedOverlay.svelte` | Selected seed glow, ghost insertion dot on centerline hover |
| `src/App.svelte` | Add `<HintLine />` component, update keyboard shortcuts for selection model |
| `src/components/HintLine.svelte` | New component: contextual hint with fade in/out logic |
