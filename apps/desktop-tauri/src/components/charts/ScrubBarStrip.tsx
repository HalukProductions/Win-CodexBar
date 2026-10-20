import { type KeyboardEvent, type PointerEvent, useEffect, useRef } from "react";

/**
 * Bar strip the pointer can hover or drag across to inspect one bar (upstream
 * 0.75.0 `MenuUsageBarStrip`). The parent owns the selection so it can swap
 * its summary line for the selected bar's summary; ids keep the selection on
 * the same bar across refreshes and drop it when that bar leaves the window.
 */

export interface ScrubBar {
  id: string;
  value: number;
  /** Not reported yet: a full-height outline that never sets the scale. */
  isPending?: boolean;
}

export interface ScrubBarStripProps {
  bars: readonly ScrubBar[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  /** BEM block for `__bar`, `__bar--pending` and `__bar--selected`. */
  classPrefix: string;
  className: string;
  label: string;
  /** Id of the summary line that reads out the selected bar. */
  describedBy?: string;
  testId?: string;
}

export function ScrubBarStrip({
  bars,
  selectedId,
  onSelect,
  classPrefix,
  className,
  label,
  describedBy,
  testId,
}: ScrubBarStripProps) {
  const dragging = useRef(false);
  const max = Math.max(1, ...bars.filter((bar) => !bar.isPending).map((bar) => bar.value));
  const selectedIndex = bars.findIndex((bar) => bar.id === selectedId);

  useEffect(() => {
    if (selectedId !== null && selectedIndex < 0) onSelect(null);
  }, [selectedId, selectedIndex, onSelect]);

  const indexAt = (event: PointerEvent<SVGSVGElement>): number | null => {
    const rect = event.currentTarget.getBoundingClientRect();
    if (rect.width <= 0 || bars.length === 0) return null;
    const ratio = (event.clientX - rect.left) / rect.width;
    if (!Number.isFinite(ratio)) return null;
    return Math.min(bars.length - 1, Math.max(0, Math.floor(ratio * bars.length)));
  };

  const selectAt = (event: PointerEvent<SVGSVGElement>) => {
    const index = indexAt(event);
    if (index !== null) onSelect(bars[index].id);
  };

  const onPointerDown = (event: PointerEvent<SVGSVGElement>) => {
    dragging.current = true;
    // Capture keeps a drag scrubbing after the pointer slips off the strip.
    event.currentTarget.setPointerCapture?.(event.pointerId);
    selectAt(event);
  };

  const endDrag = (event: PointerEvent<SVGSVGElement>) => {
    dragging.current = false;
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  const onPointerUp = (event: PointerEvent<SVGSVGElement>) => {
    endDrag(event);
    // Touch and pen have no hover to fall back to once the finger lifts.
    if (event.pointerType !== "mouse") {
      onSelect(null);
      return;
    }
    // Capture keeps pointerup on the strip even when the drag ended outside
    // it; a mouse released out there is not hovering any bar anymore. A
    // zero-size rect (no layout yet) says nothing, so it keeps the selection.
    const rect = event.currentTarget.getBoundingClientRect();
    if (
      rect.width > 0 &&
      (event.clientX < rect.left ||
        event.clientX > rect.right ||
        event.clientY < rect.top ||
        event.clientY > rect.bottom)
    ) {
      onSelect(null);
    }
  };

  const onPointerLeave = () => {
    if (!dragging.current) onSelect(null);
  };

  const onPointerCancel = (event: PointerEvent<SVGSVGElement>) => {
    endDrag(event);
    onSelect(null);
  };

  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    if (bars.length === 0) return;
    let next: number | null = null;
    if (event.key === "ArrowLeft") next = selectedIndex < 0 ? bars.length - 1 : Math.max(0, selectedIndex - 1);
    else if (event.key === "ArrowRight") next = selectedIndex < 0 ? 0 : Math.min(bars.length - 1, selectedIndex + 1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = bars.length - 1;
    else if (event.key === "Escape" && selectedIndex >= 0) {
      event.preventDefault();
      // Only clear the inspected bar; with nothing selected Escape still
      // reaches the window listener that dismisses the tray panel.
      event.stopPropagation();
      onSelect(null);
      return;
    }
    if (next === null) return;
    event.preventDefault();
    onSelect(bars[next].id);
  };

  return (
    <svg
      className={className}
      viewBox={`0 0 ${bars.length} 100`}
      preserveAspectRatio="none"
      role="group"
      tabIndex={0}
      aria-label={label}
      aria-describedby={describedBy}
      data-testid={testId}
      onPointerDown={onPointerDown}
      onPointerMove={selectAt}
      onPointerUp={onPointerUp}
      onPointerLeave={onPointerLeave}
      onPointerCancel={onPointerCancel}
      onKeyDown={onKeyDown}
      onBlur={() => onSelect(null)}
    >
      {bars.map((bar, index) => {
        const classes = [`${classPrefix}__bar`];
        if (bar.isPending) classes.push(`${classPrefix}__bar--pending`);
        if (index === selectedIndex) classes.push(`${classPrefix}__bar--selected`);
        const height = bar.isPending ? 100 : (bar.value / max) * 100;
        return (
          <rect
            key={bar.id}
            className={classes.join(" ")}
            x={index + 0.1}
            y={100 - height}
            width={0.8}
            height={height}
          />
        );
      })}
    </svg>
  );
}
