# TRAE Work CN 当前账号检测可行性

调查日期：`2026-07-30`

## 1. 结论

V1 可以在不读取账号管理器数据、不使用对话库 `project.user_id`、不保存 Token 的前提下可靠确认当前账号。

推荐方案不是强行破解认证密文，而是组合以下证据：

```text
TRAE 权威账号日志
+ 当前认证字段不可逆指纹
+ 当前 Local Storage 账号作用域
= 当前账号证据
```

账号证据不足或互相冲突时，禁止生成可执行写入计划。用户需要启动 TRAE 完成登录，待工具取得新证据后再关闭 TRAE 并同步。D-044 已确认手工选择账号不能覆盖此限制。

## 2. 已确认路径

```text
认证状态：%APPDATA%\TRAE SOLO CN\User\globalStorage\storage.json
全局状态：%APPDATA%\TRAE SOLO CN\User\globalStorage\state.vscdb
浏览器状态：%APPDATA%\TRAE SOLO CN\Local Storage\leveldb
启动日志：%APPDATA%\TRAE SOLO CN\logs\<session>
```

参考账号管理器的实际数据位于：

```text
%APPDATA%\Trae\work-cn-manager\data\accounts.json
```

此路径只用于本次交叉验证，不能成为 Trae Sync 的运行时依赖。

## 3. `storage.json` 认证格式

当前认证字段包括：

```text
iCubeAuthInfo://icube.cloudide
iCubeAuthInfo://icube-dc:<device_id>
iCubeAuthInfo://usertag
```

字段值是 Base64 包装的二进制密文。解码后以以下头部开始：

```text
74 63 05 10 00 00
 t  c
```

已确认：

- 它不是参考账号管理器写入的明文嵌套 JSON。
- 它不是可直接传给 Windows `CryptUnprotectData` 的原始 DPAPI 数据块。
- 当前 TRAE 使用定制 `@aha-kit/electron 39.2.7-release.1.46.1`。
- 定制运行时中的 `safeStorage` 在普通 Node 模式下没有完成加密服务初始化，不能作为独立工具的稳定 ABI。
- `iCubeAuthInfo://icube-dc:<id>` 中的 ID 是 `deviceId`，不是用户 `userId`。

因此 V1 不调用 TRAE 私有 Electron 运行时解密认证字段。Adapter 只保存认证字段的 SHA-256 指纹，不保存字段正文。

兼容入口：账号管理器切换时可能先写入旧式明文 JSON。若 `iCubeAuthInfo://icube.cloudide` 可解析为 JSON，Adapter 可以只读取其中的 `userId`，但仍不得保存 Token、refresh token、cookies 或完整认证对象。

## 4. 权威日志证据

产品代码已确认 `alog.log` 的 `fetchLogTask.userId` 来源是当前 iCube 用户信息对象的 `userId`：

```text
const payload = {
  machineId,
  deviceId,
  userId: currentUserInfo?.userId,
  ...
}
```

可使用的严格事件：

```text
alog.log: fetchLogTask { ..., "userId":"<id>" }
renderer.log: [RouteService] User info loaded { "userId":"<id>" }
main.log: [updateUserInfo] / [getUserInfo] 中的 userId
```

禁止搜索任意 `user_id` 后直接采用；实时会话、历史事件或迁移数据也可能包含该字段。

## 5. 多账号真实验证

检查了 `2026-07-23` 至 `2026-07-30` 的 10 个真实 TRAE 启动会话：

```text
2578820706078841：6 个启动会话
1804778984702451：4 个启动会话
每个会话至少两类独立日志来源一致：10 / 10
同一会话出现冲突账号：0
```

启动序列确实在两个账号之间多次切换。最新会话的三类来源均确认：

```text
current userId: 1804778984702451
deviceId: 2490859781907946
```

参考账号管理器的 `current_account_id` 也映射到 `1804778984702451`，仅作为交叉验证。

## 6. 不能单独使用的证据

### `state.vscdb`

备份库同时保留以下三个账号的作用域键：

```text
1804778984702451
2578820706078841
3559551364241212
```

它证明账号曾被使用，不证明当前登录账号。

### 对话数据库 `project.user_id`

同步会主动改变该字段。它只能描述活动项目当前归属，不能证明 TRAE 当前登录身份。

### 账号管理器 `current_account_id`

第三方工具可能未安装、数据过期或与 TRAE 启动结果不一致。只允许用于诊断提示，不能解锁写入。

### Local Storage

当前 LevelDB 的账号作用域记录只命中 `1804778984702451`，与日志一致。它适合作为第二证据，但必须通过 LevelDB 逻辑读取，不能扫描 `.log`/`.ldb` 原始字节后采用最后一次命中。

## 7. 推荐检测协议

```text
1. 读取产品路径、进程和最新启动会话。
2. 只解析白名单账号事件；至少两类来源一致时得到 verified_user_id。
3. 读取三个认证字段，生成字段名、值和产品版本绑定的 SHA-256 指纹。
4. 保存 fingerprint -> verified_user_id、证据时间和日志会话 ID；不保存认证正文。
5. 逻辑读取 Local Storage 当前账号作用域，要求与 verified_user_id 一致或标记为缺失。
6. TRAE 关闭后重新读取认证指纹；指纹未变才允许沿用已验证账号。
7. 明文兼容格式只提取 userId；密文指纹未知、证据过期或来源冲突时停止。
8. 同步预览和提交前均重新读取账号证据；变化使旧计划失效。
```

允许保守失败，不允许猜测。首次使用时若 TRAE 已关闭且没有可绑定的新证据，界面提示用户启动 TRAE 完成登录，再关闭后继续。

## 8. Gate B 结果

```text
真实路径：通过
真实账号字段语义：通过
多账号切换变化：通过
独立于 project.user_id：通过
账号管理器非依赖：通过
认证密文直接解密：未采用，非 V1 阻塞项
生产解析器及 fixture：实现阶段待完成
```

实现 Gate 必须包含：两个真实账号 fixture、明文兼容格式、`tc` 密文格式、未知密文指纹、日志缺失、证据冲突、认证字段变化和提交前账号漂移。
