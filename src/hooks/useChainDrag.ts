import {
  useEffect,
  type Dispatch,
  type MutableRefObject,
  type PointerEvent as ReactPointerEvent,
  type SetStateAction,
} from "react";

export interface ChainDragState<K extends string = string> {
  kind: K;
  fromIndex: number;
  dropIndex: number;
  pointerId: number;
  x: number;
  y: number;
  label: string;
}

export const CHAIN_LONG_PRESS_MS = 150;
const LONG_PRESS_MOVE_CANCEL_DISTANCE = 8;

export function clearNativeTextSelection() {
  if (typeof window === "undefined") {
    return;
  }
  window.getSelection?.()?.removeAllRanges();
}

export function moveArrayToInsertion<T>(arr: T[], from: number, insertionIndex: number): T[] {
  if (from < 0 || from >= arr.length) {
    return arr;
  }
  const next = [...arr];
  const [item] = next.splice(from, 1);
  const clampedIndex = Math.max(0, Math.min(insertionIndex, next.length));
  next.splice(clampedIndex, 0, item);
  return next;
}

export function remapIndexAfterInsertion(
  index: number | null,
  length: number,
  from: number,
  insertionIndex: number,
): number | null {
  if (index === null) {
    return null;
  }
  const remapped = moveArrayToInsertion(
    Array.from({ length }, (_, i) => i),
    from,
    insertionIndex,
  );
  return remapped.indexOf(index);
}

function computeInsertionIndex(kind: string, fromIndex: number, clientX: number): number {
  const hosts = Array.from(document.querySelectorAll<HTMLElement>(`[data-drag-kind="${kind}"][data-drag-index]`))
    .map((host) => ({
      host,
      index: Number(host.dataset.dragIndex),
    }))
    .filter((item) => Number.isInteger(item.index) && item.index !== fromIndex)
    .sort((a, b) => a.index - b.index);

  if (hosts.length === 0) {
    return 0;
  }

  const beforeIndex = hosts.findIndex(({ host }) => {
    const rect = host.getBoundingClientRect();
    return clientX < rect.left + rect.width / 2;
  });

  return beforeIndex === -1 ? hosts.length : beforeIndex;
}

export function resolveInsertBeforeIndex(
  length: number,
  fromIndex: number,
  insertionIndex: number,
): number | null {
  const visible = Array.from({ length }, (_, i) => i).filter((idx) => idx !== fromIndex);
  if (insertionIndex < 0) {
    return visible[0] ?? null;
  }
  if (insertionIndex >= visible.length) {
    return null;
  }
  return visible[insertionIndex];
}

interface BeginLongPressDragOptions<K extends string, T extends ChainDragState<K>> {
  event: ReactPointerEvent<HTMLElement>;
  kind: K;
  fromIndex: number;
  label: string;
  longPressTimerRef: MutableRefObject<ReturnType<typeof setTimeout> | null>;
  startPosRef: MutableRefObject<{ x: number; y: number }>;
  setDragState: Dispatch<SetStateAction<T | null>>;
  longPressMs?: number;
  ignoreTagNames?: string[];
}

export function cancelLongPressDragCandidate(longPressTimerRef: MutableRefObject<ReturnType<typeof setTimeout> | null>) {
  if (longPressTimerRef.current) {
    clearTimeout(longPressTimerRef.current);
    longPressTimerRef.current = null;
  }
}

export function beginLongPressDrag<K extends string, T extends ChainDragState<K>>({
  event,
  kind,
  fromIndex,
  label,
  longPressTimerRef,
  startPosRef,
  setDragState,
  longPressMs = CHAIN_LONG_PRESS_MS,
  ignoreTagNames = [],
}: BeginLongPressDragOptions<K, T>) {
  if (event.button !== 0) {
    return;
  }
  const target = event.target;
  if (target instanceof HTMLElement && ignoreTagNames.includes(target.tagName)) {
    return;
  }

  cancelLongPressDragCandidate(longPressTimerRef);
  event.preventDefault();
  clearNativeTextSelection();
  startPosRef.current = { x: event.clientX, y: event.clientY };

  longPressTimerRef.current = setTimeout(() => {
    clearNativeTextSelection();
    setDragState({
      kind,
      fromIndex,
      dropIndex: fromIndex,
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      label,
    } as T);
    longPressTimerRef.current = null;
  }, longPressMs);
}

interface UpdateLongPressDragCandidateOptions {
  event: ReactPointerEvent<HTMLElement>;
  longPressTimerRef: MutableRefObject<ReturnType<typeof setTimeout> | null>;
  startPosRef: MutableRefObject<{ x: number; y: number }>;
  moveCancelDistance?: number;
}

export function updateLongPressDragCandidate({
  event,
  longPressTimerRef,
  startPosRef,
  moveCancelDistance = LONG_PRESS_MOVE_CANCEL_DISTANCE,
}: UpdateLongPressDragCandidateOptions) {
  if (!longPressTimerRef.current) {
    return;
  }
  event.preventDefault();
  clearNativeTextSelection();
  const dx = event.clientX - startPosRef.current.x;
  const dy = event.clientY - startPosRef.current.y;
  if (Math.hypot(dx, dy) > moveCancelDistance) {
    cancelLongPressDragCandidate(longPressTimerRef);
  }
}

interface CompleteLongPressTapOptions {
  longPressTimerRef: MutableRefObject<ReturnType<typeof setTimeout> | null>;
  onTap: () => void;
}

export function completeLongPressTap({ longPressTimerRef, onTap }: CompleteLongPressTapOptions) {
  if (!longPressTimerRef.current) {
    return;
  }
  clearTimeout(longPressTimerRef.current);
  longPressTimerRef.current = null;
  onTap();
}

interface UseChainDragSessionOptions<K extends string, T extends ChainDragState<K>> {
  dragState: T | null;
  dragStateRef: MutableRefObject<T | null>;
  setDragState: Dispatch<SetStateAction<T | null>>;
  getRowElement: (kind: K) => HTMLElement | null;
  onApplyDrag: (state: T) => void;
}

export function useChainDragSession<K extends string, T extends ChainDragState<K>>({
  dragState,
  dragStateRef,
  setDragState,
  getRowElement,
  onApplyDrag,
}: UseChainDragSessionOptions<K, T>) {
  useEffect(() => {
    dragStateRef.current = dragState;
  }, [dragState, dragStateRef]);

  useEffect(() => {
    document.body.classList.toggle("is-dragging-chain", dragState !== null);
    return () => document.body.classList.remove("is-dragging-chain");
  }, [dragState]);

  useEffect(() => {
    if (!dragState) {
      return;
    }

    const finishDragging = (apply: boolean) => {
      const current = dragStateRef.current;
      if (!current) {
        setDragState(null);
        return;
      }
      if (apply) {
        onApplyDrag(current);
      }
      setDragState(null);
    };

    const handlePointerMove = (event: PointerEvent) => {
      const current = dragStateRef.current;
      if (!current || current.pointerId !== event.pointerId) {
        return;
      }

      event.preventDefault();
      clearNativeTextSelection();

      if (event.buttons === 0) {
        finishDragging(true);
        return;
      }

      const row = getRowElement(current.kind);
      let nextDropIndex = current.dropIndex;
      if (row) {
        const rect = row.getBoundingClientRect();
        const nearRow =
          event.clientY >= rect.top - 28 &&
          event.clientY <= rect.bottom + 28 &&
          event.clientX >= rect.left - 80 &&
          event.clientX <= rect.right + 80;
        if (nearRow) {
          const edgeThreshold = 26;
          if (event.clientX < rect.left + edgeThreshold) {
            row.scrollLeft = Math.max(0, row.scrollLeft - 18);
          } else if (event.clientX > rect.right - edgeThreshold) {
            row.scrollLeft += 18;
          }
          nextDropIndex = computeInsertionIndex(current.kind, current.fromIndex, event.clientX);
        }
      }

      setDragState((prev) =>
        prev && prev.pointerId === event.pointerId
          ? {
              ...prev,
              x: event.clientX,
              y: event.clientY,
              dropIndex: nextDropIndex,
            }
          : prev,
      );
    };

    const handlePointerUp = (event: PointerEvent) => {
      const current = dragStateRef.current;
      if (!current || current.pointerId !== event.pointerId) {
        return;
      }
      clearNativeTextSelection();
      finishDragging(true);
    };

    const handleSelectStart = (event: Event) => event.preventDefault();
    const handleBlur = () => finishDragging(false);

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    document.addEventListener("selectstart", handleSelectStart);
    window.addEventListener("blur", handleBlur);
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      document.removeEventListener("selectstart", handleSelectStart);
      window.removeEventListener("blur", handleBlur);
    };
  }, [dragState?.pointerId, dragStateRef, getRowElement, onApplyDrag, setDragState]);
}
