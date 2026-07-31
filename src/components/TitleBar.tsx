import type {
  PlatformContextDto,
  DataLocationStateDto,
  CurrentAccountStateDto,
} from "../types/workspace";

interface TitleBarProps {
  platform: PlatformContextDto;
  dataLocation: DataLocationStateDto;
  currentAccount: CurrentAccountStateDto;
}

// 标题栏：持续显示平台、数据位置和当前账号。
// 这是规格要求的有意安全复核——让用户始终看到当前环境。
export function TitleBar({ platform, dataLocation, currentAccount }: TitleBarProps) {
  return (
    <header className="title-bar" role="banner">
      <div className="title-bar__brand">Trae Sync</div>
      <div className="title-bar__context">
        <span className="title-bar__item" data-testid="platform-context">
          平台：{platform.display_name}
          {!platform.adapter_implemented && <span className="badge badge--off">边界保留</span>}
        </span>
        <span className="title-bar__item" data-testid="data-location-context">
          {dataLocation.selected
            ? `数据位置：${dataLocation.display_name ?? ""}`
            : "数据位置未选择"}
        </span>
        <span className="title-bar__item" data-testid="current-account-context">
          {currentAccount.detected
            ? `当前账号：${currentAccount.user_fingerprint ?? ""}`
            : "当前账号未检测"}
        </span>
      </div>
    </header>
  );
}
