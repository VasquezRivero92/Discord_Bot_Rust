/**
 * Makes every interaction feel responsive without waiting for the API.
 * It adds a tactile press state immediately and ties requests started right
 * after that interaction to the clicked control until all of them settle.
 */
export function installInstantFeedback() {
  if (typeof window === 'undefined' || window.__mixtonInstantFeedbackInstalled) return;
  window.__mixtonInstantFeedbackInstalled = true;

  let pendingRequests = 0;
  let lastInteraction: { control: HTMLElement; at: number } | null = null;
  const pendingByControl = new WeakMap<HTMLElement, number>();
  const originalFetch = window.fetch.bind(window);

  const updateNetworkState = () => {
    const isPending = pendingRequests > 0;
    document.documentElement.dataset.networkPending = isPending ? 'true' : 'false';
    document.getElementById('root')?.setAttribute('aria-busy', String(isPending));
  };

  const rememberInteraction = (target: EventTarget | null) => {
    const control = target instanceof Element
      ? target.closest<HTMLElement>('button, [role="button"], select, input[type="button"], input[type="submit"], input[type="checkbox"]')
      : null;
    if (!control || control.matches(':disabled, [aria-disabled="true"]')) return;

    lastInteraction = { control, at: Date.now() };
    control.dataset.instantPressed = 'true';
    window.setTimeout(() => delete control.dataset.instantPressed, 180);
  };

  window.addEventListener('pointerdown', event => rememberInteraction(event.target), true);
  window.addEventListener('keydown', event => {
    if (event.key === 'Enter' || event.key === ' ') rememberInteraction(event.target);
  }, true);
  window.addEventListener('click', event => {
    const control = event.target instanceof Element
      ? event.target.closest<HTMLElement>('[data-request-pending="true"]')
      : null;
    if (control) {
      event.preventDefault();
      event.stopPropagation();
    }
  }, true);

  window.fetch = (...args: Parameters<typeof window.fetch>) => {
    pendingRequests += 1;
    updateNetworkState();

    const relatedControl = lastInteraction && Date.now() - lastInteraction.at < 750
      ? lastInteraction.control
      : null;
    if (relatedControl) {
      const count = (pendingByControl.get(relatedControl) || 0) + 1;
      pendingByControl.set(relatedControl, count);
      relatedControl.dataset.requestPending = 'true';
    }

    return originalFetch(...args).finally(() => {
      pendingRequests = Math.max(0, pendingRequests - 1);
      updateNetworkState();

      if (relatedControl) {
        const nextCount = (pendingByControl.get(relatedControl) || 1) - 1;
        if (nextCount > 0) {
          pendingByControl.set(relatedControl, nextCount);
        } else {
          pendingByControl.delete(relatedControl);
          delete relatedControl.dataset.requestPending;
        }
      }
    });
  };
}

declare global {
  interface Window {
    __mixtonInstantFeedbackInstalled?: boolean;
  }
}
