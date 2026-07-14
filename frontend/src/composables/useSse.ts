import { ref, watch, onUnmounted, type Ref } from "vue";

export interface LogLine {
  line: string;
}

export function useSse(url: Ref<string | null>) {
  const lines = ref<LogLine[]>([]);
  const connected = ref(false);
  const error = ref<string | null>(null);
  let source: EventSource | null = null;

  const connect = () => {
    if (!url.value) return;
    disconnect();
    error.value = null;

    source = new EventSource(url.value);
    connected.value = true;

    source.addEventListener("log", (event: MessageEvent) => {
      const data = JSON.parse(event.data) as LogLine;
      lines.value.push(data);
      if (lines.value.length > 10_000) {
        lines.value = lines.value.slice(-5_000);
      }
    });

    source.addEventListener("error", (event: MessageEvent) => {
      try {
        const data = JSON.parse(event.data) as { message: string };
        error.value = data.message;
      } catch {
        error.value = "SSE connection error";
      }
    });

    source.addEventListener("close", () => {
      connected.value = false;
    });

    source.onerror = () => {
      connected.value = false;
      error.value = "SSE connection lost";
    };
  };

  const disconnect = () => {
    source?.close();
    source = null;
    connected.value = false;
  };

  const clear = () => {
    lines.value = [];
  };

  watch(url, () => {
    if (url.value) connect();
    else disconnect();
  });

  onUnmounted(disconnect);

  return { lines, connected, error, connect, disconnect, clear };
}
