// fakeInvoke 路由器行为自测（P12-R1c）：不 mock core 模块，直接驱动 mockCoreInvoke
// 单例，锁住三条核心契约——按路由分发、未注册命令 fail-fast、resetRoutes 清空。

import { beforeEach, describe, expect, it } from "vitest";
import { installFakeInvoke, mockCoreInvoke, mockRoute, resetRoutes } from "./helpers/fakeInvoke";

describe("fakeInvoke 路由器", () => {
  beforeEach(() => {
    resetRoutes();
    installFakeInvoke();
  });

  it("已注册命令按路由表分发，args 透传给处理器", async () => {
    mockRoute("get_workspace_state", (args) => ({ echo: args }));
    await expect(mockCoreInvoke("get_workspace_state", { a: 1 })).resolves.toEqual({
      echo: { a: 1 },
    });
  });

  it("未注册命令 fail-fast：错误信息含命令名并提示 mockRoute 注册", async () => {
    await expect(mockCoreInvoke("get_workspace_state")).rejects.toThrow(
      /未注册 mock[\s\S]*mockRoute\("get_workspace_state"/,
    );
  });

  it("resetRoutes 清空路由表：清空后已注册命令也 fail-fast，防用例间串扰", async () => {
    mockRoute("get_workspace_state", () => "ok");
    resetRoutes();
    await expect(mockCoreInvoke("get_workspace_state")).rejects.toThrow(/未注册/);
  });

  it("处理器抛错时 invoke 以同一错误 reject（可模拟后端业务失败）", async () => {
    mockRoute("get_workspace_state", () => {
      throw new Error("business:9074");
    });
    await expect(mockCoreInvoke("get_workspace_state")).rejects.toThrow("business:9074");
  });
});
