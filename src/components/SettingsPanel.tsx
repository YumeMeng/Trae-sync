// 设置入口：T01 阶段只有规格明确要求的两个开关。
// - “自动查找新历史”默认关闭（规格第 29 节）
// - “同步成功后重新打开 TRAE”默认开启
// 这两个开关在 T01 阶段不连接真实行为，只展示 UI 边界。
export function SettingsPanel() {
  return (
    <section className="settings-panel" role="region" aria-label="设置">
      <h2>设置</h2>
      <ul className="settings-list">
        <li className="settings-item">
          <label>
            <input type="checkbox" defaultChecked={false} disabled />
            自动查找新历史（默认关闭）
          </label>
        </li>
        <li className="settings-item">
          <label>
            <input type="checkbox" defaultChecked={true} disabled />
            同步成功后重新打开 TRAE（默认开启）
          </label>
        </li>
      </ul>
    </section>
  );
}
