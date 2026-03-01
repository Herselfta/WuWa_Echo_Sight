import { useEffect } from "react";

const INTERACTIVE_TARGET_SELECTOR =
  'button, input, select, textarea, a, label, [role="button"], [contenteditable="true"]';

export function shouldDismissChainSelection(target: Element, chainScopeSelector: string): boolean {
  const isInteractiveTarget = Boolean(target.closest(INTERACTIVE_TARGET_SELECTOR));
  const inChainScope = Boolean(target.closest(chainScopeSelector));
  return !isInteractiveTarget && !inChainScope;
}

interface UseChainSelectionDismissOptions {
  chainScopeSelector: string;
  onDismiss: () => void;
  onPointerDown?: (target: Element) => void;
}

export function useChainSelectionDismiss({
  chainScopeSelector,
  onDismiss,
  onPointerDown,
}: UseChainSelectionDismissOptions) {
  useEffect(() => {
    const handler = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Element)) {
        return;
      }
      if (shouldDismissChainSelection(target, chainScopeSelector)) {
        onDismiss();
      }
      onPointerDown?.(target);
    };
    window.addEventListener("pointerdown", handler);
    return () => window.removeEventListener("pointerdown", handler);
  }, [chainScopeSelector, onDismiss, onPointerDown]);
}
