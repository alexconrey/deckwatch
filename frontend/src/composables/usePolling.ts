import { onMounted, onUnmounted } from "vue";

export function usePolling(fn: () => Promise<void>, intervalMs = 5000) {
  let timer: ReturnType<typeof setInterval> | null = null;

  const clear = () => {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  };

  const start = () => {
    clear();
    void fn();
    timer = setInterval(() => void fn(), intervalMs);
  };

  const stop = () => {
    clear();
  };

  // Fire the callback immediately without disturbing the interval cadence.
  const refresh = () => {
    void fn();
  };

  // Pause polling while the tab is hidden so background tabs don't flood the
  // API server. Resume with an immediate fetch on visibility so the user
  // sees fresh data the moment they come back.
  const onVisibilityChange = () => {
    if (document.hidden) {
      clear();
    } else {
      start();
    }
  };

  onMounted(() => {
    start();
    document.addEventListener("visibilitychange", onVisibilityChange);
  });

  onUnmounted(() => {
    document.removeEventListener("visibilitychange", onVisibilityChange);
    stop();
  });

  return { start, stop, refresh };
}
