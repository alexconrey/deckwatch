import { ref } from "vue";

// Global snackbar state, shared across every caller. Mount a single
// <v-snackbar> at the app root and drive it through this composable so any
// component can flash a toast without wiring its own dialog plumbing.
export type SnackbarColor = "success" | "error" | "info" | "warning";

interface SnackbarState {
  show: boolean;
  message: string;
  color: SnackbarColor;
  timeout: number;
}

const DEFAULT_TIMEOUT_MS = 3000;
const ERROR_TIMEOUT_MS = 5000;

const snackbar = ref<SnackbarState>({
  show: false,
  message: "",
  color: "success",
  timeout: DEFAULT_TIMEOUT_MS,
});

function push(color: SnackbarColor, message: string, timeout: number) {
  snackbar.value = { show: true, message, color, timeout };
}

export function useSnackbar() {
  const success = (message: string, timeout = DEFAULT_TIMEOUT_MS) =>
    push("success", message, timeout);
  const error = (message: string, timeout = ERROR_TIMEOUT_MS) =>
    push("error", message, timeout);
  const info = (message: string, timeout = DEFAULT_TIMEOUT_MS) =>
    push("info", message, timeout);
  const warning = (message: string, timeout = DEFAULT_TIMEOUT_MS) =>
    push("warning", message, timeout);

  return { snackbar, success, error, info, warning };
}
