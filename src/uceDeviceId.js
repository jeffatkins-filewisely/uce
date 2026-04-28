const UCE_DEVICE_KEY = "uce_device_id";

/** Stable per-profile device id (shared with main UCE webview — same localStorage origin). */
export function getUceDeviceId() {
  let id = localStorage.getItem(UCE_DEVICE_KEY);
  if (!id) {
    id = `uce-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
    localStorage.setItem(UCE_DEVICE_KEY, id);
  }
  return id;
}
