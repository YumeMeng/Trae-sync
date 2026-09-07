/**
 * 健康检测 / 刷新额度共用结果卡（G9，2026-09-03 grill）。
 *
 * 结构：首行结论（"12 个账号正常，4 个需要处理"）+ 下方只列异常账号
 * （名字 + 一句人话原因）；正常账号不占空间。
 * 卡片本身是纯展示组件：异常判定与原因文案由调用方（AccountCenter）
 * 用 G10 状态机（derive* 枚举态）与 safeUiError 词表求出，保证
 * 结果卡、徽章、详情页三处同义同色。
 */
export interface OperationResultIssue {
  /** 稳定 key（profile_id）：两账号同名（同 screen_name 无备注）时不冲突。 */
  readonly key: string;
  /** 展示名（备注名优先，与卡片徽章同一口径）。 */
  readonly name: string;
  /** 一句人话原因；与 G10 状态机及 safeUiError 词表共用文案。 */
  readonly reason: string;
}

export function OperationResultCard({
  title,
  okCount,
  issues,
}: {
  /** 操作名（"健康检测" / "额度刷新"），只用于结论行前缀。 */
  readonly title: string;
  /** 正常账号数（总数 - 异常数，由调用方求出）。 */
  readonly okCount: number;
  /** 异常账号列表；空 = 全部正常（只渲染结论行）。 */
  readonly issues: readonly OperationResultIssue[];
}) {
  return (
    <div className="result-card" role="status" data-testid="operation-result-card">
      <p className="result-card__headline">
        {title}完成：{okCount} 个账号正常
        {issues.length > 0 ? `，${issues.length} 个需要处理` : ""}。
      </p>
      {issues.length > 0 && (
        <ul className="result-card__issues">
          {issues.map((issue) => (
            <li key={issue.key} className="result-card__issue">
              <span className="result-card__issue-name">{issue.name}</span>
              <span className="result-card__issue-reason">{issue.reason}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
