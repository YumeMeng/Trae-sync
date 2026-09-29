// ============================================================================
// fakeInvoke：测试共享的 Tauri invoke mock 路由器（P12-R1c）
// ============================================================================
// 新测试文件用法：
//   import { installFakeInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";
//
//   // 注意工厂必须内联 async + 动态 import，不能直接引用文件顶部的 import 绑定：
//   // Vitest 转换会把静态 import 变成顺序赋值语句，工厂引用外部 import 绑定时
//   // 若被 mock 模块先于 helper 初始化就会 ReferenceError；工厂体内动态 import
//   // 则在被 mock 模块首次导入时才解析，时序永远安全（官方推荐模式）。
//   vi.mock("@tauri-apps/api/core", async () => {
//     const fake = await import("./helpers/fakeInvoke");
//     return { invoke: fake.mockCoreInvoke };
//   });
//
//   beforeEach(() => {
//     resetRoutes();
//     installFakeInvoke();
//   });
//   // 测试内按命令注册响应：
//   mockRoute("get_workspace_state", (args) => ({ ...响应... }));
//
// 未注册命令 fail-fast：分发时直接抛错（错误信息含命令名并提示 mockRoute 注册），
// 避免静默返回 undefined 掩盖漏配。
//
// 旧测试文件迁移策略（渐进、可随时中断，P12-R1c 定案不一次性迁移旧 322 处）：
//   1. 每文件独立小改：把 `() => ({ invoke: vi.fn() })` 工厂替换为上面的
//      async 工厂（invoke 替身从文件内私有 vi.fn() 换成共享 mockCoreInvoke 单例）；
//   2. beforeEach 的 mockInvoke.mockReset() 换成 resetRoutes() + installFakeInvoke()
//      （等价：installFakeInvoke 内部 mockClear 清调用历史 + 重新安装分发实现）；
//   3. mockImplementation 里的 if (command === "x") {...} 分支逐条替换为
//      mockRoute("x", (args) => {...}) 注册（行为等价：Map 按 cmd 精确查找，
//      与 if 链同序命中）；
//   4. 迁移互不影响：vitest 每个测试文件独立模块图，本模块的单例互不串扰；
//      迁一半也安全——未迁移文件仍走自己的 vi.fn()，已迁移文件走本路由器。

import { vi } from "vitest";
import type { CommandName } from "../../src/types/commandNames";

/** invoke 替身单例：分发实现由 installFakeInvoke 安装；调用记录可直接用于断言。 */
export const mockCoreInvoke = vi.fn();

/** 命令响应处理器：接收 invoke 的 args 参数，返回值即 invoke 的 resolve 值（抛错则 reject）。 */
type RouteHandler = (args: any) => unknown;

/** 命令 → 处理器路由表。key 受 CommandName 联合类型约束，命令名写错编译期即报错。 */
const routes = new Map<CommandName, RouteHandler>();

/** 注册一条命令路由（同名重复注册按 Map 语义覆盖，后注册者生效）。 */
export function mockRoute(cmd: CommandName, handler: RouteHandler): void {
  routes.set(cmd, handler);
}

/** 清空路由表，供 beforeEach 使用，防止用例间路由串扰。 */
export function resetRoutes(): void {
  routes.clear();
}

/**
 * 安装按路由表分发的 invoke 实现（幂等）：清空调用历史并覆盖实现。
 * 未注册命令 fail-fast 抛错，错误信息含命令名与注册提示。
 */
export function installFakeInvoke(): void {
  mockCoreInvoke.mockClear();
  mockCoreInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    const handler = routes.get(cmd as CommandName);
    if (!handler) {
      throw new Error(
        `[fakeInvoke] 命令 "${cmd}" 未注册 mock。请在测试中先 mockRoute("${cmd}", (args) => ...)。`,
      );
    }
    return handler(args);
  });
}
