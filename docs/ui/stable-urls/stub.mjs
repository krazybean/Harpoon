// The page-side Tauri stub. STUB_SOURCE is a constant: no data is spliced into it. The scene travels in the URL
// (`?fixture=<percent-encoded JSON>`) and is only ever JSON.parse'd, never evaluated as code.
export const STUB_SOURCE = `(() => {
  const S = JSON.parse(new URLSearchParams(location.search).get("fixture") ?? "null");
  if (!S) return;
  const answers = {
    get_status: { state: "running", pid: 1, cpus: 2, memoryMiB: 4096, dockerReady: true, sockExists: true, lockHeld: true },
    deploy_snapshot: S.snap, external_runtimes: [], host_tcp_listeners: { verified: true, error: null, checkedMs: S.now, listeners: [] },
    list_containers: S.containers, get_counts: { containers: 1, running: 1, images: 1, volumes: 0, networks: 1 }, get_config: { cpus: 2, memory: 4096, raw: "", path: "/dev/null" },
    list_images: [], list_volumes: [], list_networks: [], get_docker_info: "27.0.1", discover_contexts: [], get_doctor: { raw: "", passed: 5, warnings: 0, failures: 0 }, get_recent_logs: "", get_log_path: "/dev/null", get_harpoon_binary_path: "/dev/null",
  };
  let cb = 0;
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    transformCallback: () => ++cb, unregisterCallback() {}, convertFileSrc: (p) => p,
    invoke: async (cmd) => { if (cmd in answers) return answers[cmd]; if (cmd.startsWith("plugin:")) return 0; throw new Error("fixture: no answer for " + cmd); },
  };
})();`;

export const fixtureUrl = (base, scene) => `${base}?fixture=${encodeURIComponent(JSON.stringify(scene))}`;
