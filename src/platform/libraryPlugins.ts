/**
 * 库页插件 tab 的命令入口（P4e-1 拆分件）：已装列表 / 市场浏览 / 安装 /
 * 全账号卸载传播四条命令在 ./environmentPlugins 已有类型化封装，此处只做
 * 库域 re-export，不重复包装。卸载的全账号传播（fail-soft 逐账号）与确认
 * 文案「将同时从 N 个账号移除」由调用方负责，安装保持零确认。
 */

export {
  browsePluginMarket,
  installPlugin,
  readPluginTabState,
  uninstallPluginEverywhere,
} from "./environmentPlugins";
