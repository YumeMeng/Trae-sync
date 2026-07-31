import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceStateDto } from "./types/workspace";
import { TitleBar } from "./components/TitleBar";
import { HistoryWorkbench } from "./components/HistoryWorkbench";
import { OperationsPanel } from "./components/OperationsPanel";
import { SettingsPanel } from "./components/SettingsPanel";

// 应用根组件：负责拉取工作台状态并分发到各功能区。
// T01 阶段所有真实能力禁用，UI 只展示空工作台的诚实状态。
export default function App() {
  const [state, setState] = useState<WorkspaceStateDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    // 通过 Tauri command 拉取工作台状态，前端不直接访问文件/数据库/认证
    invoke<WorkspaceStateDto>("get_workspace_state")
      .then((s) => {
        if (!cancelled) setState(s);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (error) {
    return (
      <div className="app-error" role="alert">
        加载工作台状态失败：{error}
      </div>
    );
  }

  if (!state) {
    return <div className="app-loading">正在加载 Trae Sync 工作台…</div>;
  }

  return (
    <div className="app-shell">
      <TitleBar
        platform={state.platform}
        dataLocation={state.data_location}
        currentAccount={state.current_account}
      />
      <main className="app-main">
        <HistoryWorkbench
          history={state.history}
          capabilities={state.capabilities}
          honestStatus={state.honest_status}
        />
        <OperationsPanel capabilities={state.capabilities} />
        <SettingsPanel />
      </main>
    </div>
  );
}
