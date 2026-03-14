import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

type DaemonStatus = {
  pid?: number;
  uptime_secs?: number;
  version?: string;
  server_count?: number;
};

type Panel = "dashboard" | "servers" | "settings" | "logs";

export default function App() {
  const [panel, setPanel] = useState<Panel>("dashboard");
  const [running, setRunning] = useState(false);
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [servers, setServers] = useState<Array<{ name: string }>>([]);

  async function refreshStatus() {
    try {
      const resp = await invoke<{ ok: boolean; data?: DaemonStatus }>("daemon_status");
      setRunning(true);
      setStatus(resp.data ?? null);
    } catch {
      setRunning(false);
      setStatus(null);
    }
  }

  async function refreshServers() {
    try {
      const resp = await invoke<{ data?: Array<{ name: string }> }>("mcp_list");
      setServers(resp.data ?? []);
    } catch {
      setServers([]);
    }
  }

  useEffect(() => {
    refreshStatus();
    const id = setInterval(refreshStatus, 5000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    if (panel === "servers") refreshServers();
  }, [panel]);

  return (
    <div style={{ display: "flex", height: "100vh", fontFamily: "sans-serif" }}>
      {/* Sidebar */}
      <nav style={{ width: 160, background: "#1e1e2e", color: "#cdd6f4", padding: "1rem 0" }}>
        {(["dashboard", "servers", "settings", "logs"] as Panel[]).map((p) => (
          <div
            key={p}
            onClick={() => setPanel(p)}
            style={{
              padding: "0.6rem 1rem",
              cursor: "pointer",
              background: panel === p ? "#313244" : "transparent",
              textTransform: "capitalize",
            }}
          >
            {p}
          </div>
        ))}
      </nav>

      {/* Content */}
      <main style={{ flex: 1, padding: "1.5rem" }}>
        {panel === "dashboard" && (
          <Dashboard running={running} status={status} onRefresh={refreshStatus} />
        )}
        {panel === "servers" && <Servers servers={servers} onRefresh={refreshServers} />}
        {panel === "settings" && <Settings />}
        {panel === "logs" && <Logs />}
      </main>
    </div>
  );
}

function Dashboard({
  running,
  status,
  onRefresh,
}: {
  running: boolean;
  status: DaemonStatus | null;
  onRefresh: () => void;
}) {
  const border = running ? "2px solid #a6e3a1" : "2px solid #6c7086";

  async function start() {
    await invoke("daemon_start");
    onRefresh();
  }
  async function stop() {
    await invoke("daemon_stop");
    onRefresh();
  }
  async function restart() {
    await invoke("daemon_restart");
    onRefresh();
  }

  return (
    <div>
      <h2>Dashboard</h2>
      <div
        style={{
          border,
          borderRadius: 8,
          padding: "1rem",
          maxWidth: 400,
          marginBottom: "1rem",
        }}
      >
        <div>
          <strong>Status:</strong> {running ? "Running" : "Stopped"}
        </div>
        {status && (
          <>
            <div>
              <strong>PID:</strong> {status.pid}
            </div>
            <div>
              <strong>Uptime:</strong> {Math.floor((status.uptime_secs ?? 0) / 60)}m{" "}
              {(status.uptime_secs ?? 0) % 60}s
            </div>
            <div>
              <strong>Version:</strong> {status.version}
            </div>
            <div>
              <strong>Servers:</strong> {status.server_count} connected
            </div>
          </>
        )}
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <button onClick={start}>Start</button>
        <button onClick={stop}>Stop</button>
        <button onClick={restart}>Restart</button>
      </div>
    </div>
  );
}

function Servers({
  servers,
  onRefresh,
}: {
  servers: Array<{ name: string }>;
  onRefresh: () => void;
}) {
  return (
    <div>
      <h2>Servers</h2>
      {servers.length === 0 ? (
        <p>No servers connected.</p>
      ) : (
        <ul>
          {servers.map((s) => (
            <li key={s.name}>{s.name}</li>
          ))}
        </ul>
      )}
      <button onClick={onRefresh}>Refresh</button>
    </div>
  );
}

function Settings() {
  return (
    <div>
      <h2>Settings</h2>
      <p>Start at login and port configuration coming soon.</p>
    </div>
  );
}

function Logs() {
  return (
    <div>
      <h2>Logs</h2>
      <p>Log streaming is not yet implemented.</p>
    </div>
  );
}
