# ADR-0031：账号级远程设备管理能力

## 状态

Accepted，2026-09-23。基于 `.scratch/research-trae-device-management-20260923.md` 的官方页面只读分析与本地凭据实测（Trae CN 两组凭据调 ListDevices 成功，占用 7/10、4/10）建立。依赖 ADR-0029（多产品工作台边界）与 ADR-0030（Trae CN 凭据维护边界）；不修改 ADR-0019 的 Work CN 签到设备策略与 P1-4 的 v6 手动重置语义。

## 背景

设备重铸与多产品登录使同一账号的服务端设备占用持续增长，达到上限（官方文档：10 台）时登录/续期返回业务码 20401/20408，此前被笼统显示为网络错误。官方授权页提供设备列表与退出能力；其前端脚本确认 `ListDevices` 与 `ClearRefreshToken` 两个接口。实测证明本地 Trae CN access token（`x-cloudide-token`）可直接调用 ListDevices；ClearRefreshToken 会真实改变服务端状态，无法无副作用探针验证。

## 决策（2026-09-23 grill 九问定案）

1. 设备管理是账号级共享能力，第一版即在 Work CN 与 Trae CN 同时提供；Work CN 凭据调用 ListDevices 的可行性在实现后实测补证，不可用时按产品能力开关隐藏模块。
2. 原生 ListDevices 为主路径；原生失败、字段不兼容或认证失效时显示可理解原因，并提供官方授权页兜底入口。
3. 本机设备禁止远程退出：设备行凭本机凭据 device_id 匹配加「本机」标记并隐藏退出按钮；后端 ClearRefreshToken 调用前做同一比对作为纵深防御，目标是本机设备时拒绝执行。
4. 其他设备的退出必须逐设备、单次确认；确认弹窗列明设备名称、绑定产品与「该设备上所有 TRAE 产品需重新登录」的影响。不提供批量退出，不自动选择最旧/同名设备。
5. 设备列表按账号全量展示，行内以标签显示绑定产品；不提供产品过滤。
6. 远程退出成功后重拉设备列表并触发该账号健康检查；「本机凭据失效 → need_authorization」标记保留为防御性兜底（仅在 device_id 比对漏判时生效），与 ADR-0029 的 ProductCredential 状态流一致。
7. 协议实现放产品中立的 infrastructure `remote_device` 模块（端口 + ListDevices/ClearRefreshToken 实现）；`checkin_http.rs` 保持签到专用，不把账号级能力塞进签到模块。产品适配器提供 OAuth client 与凭据上下文。
8. 共享接入走 ADR-0029 能力清单：适配器声明 `supports_remote_devices`，不支持的产品不渲染该模块。
9. UI 不显示 DeviceID、client ID、业务码等内部标识；主信息为设备名称、设备类型、绑定产品、最近活跃时间、已用/上限（界面表达纪律）。列表信息丰富度以官方授权网页为基线只增不减：官方页具备的（数量/上限汇总、设备名、登录应用、最近活跃、逐行退出、退出确认）全部保留，另增加本机徽章、设备类型独立展示、相对活跃时间等官方页没有的信息。
10. ClearRefreshToken 验证方案：mock/单测固化请求形状（字段、头、错误映射）；真实退出由用户在真机明确选定可牺牲设备、工具内单次确认后执行验收，结果落档 `.scratch/`。
11. remint 生命周期本次不动：自动重铸链已在 P1-4 删除，20401 文案映射已存在。重铸前「会创建新服务端设备」提示与「重铸时退出旧设备」选项作为独立后续任务另行决策。
12. 设备面板挂载在**账号详情视图**内、绑定具体账号（Work CN 内嵌 AccountDetail；Trae CN 卡片点击进入轻量 AdapterAccountDetail），每个账号均可查看与管理自己的远程设备；账号列表页不设独立的设备区块（2026-09-23 用户拍板的 UI 结构，取代首版「账号页级独立区块」实现）。登录失败（20401/20408）文案引导用户打开对应账号的详情页操作。能力清单声明（决策 8）继续生效。

## 最小模型

```text
RemoteDeviceManager (port, infrastructure/remote_device)
  list_devices(auth: ProductAuthContext) -> RemoteDeviceSnapshot
  clear_refresh_token(auth: ProductAuthContext, target: DeviceRef) -> ClearResult

RemoteDeviceSnapshot
  devices[], used_count, max_count

RemoteDevice
  device_id            # 仅内部流转与本机比对，不进 UI
  device_type, device_name
  bound_products[]
  last_active_at
  is_local             # device_id 匹配本机凭据 → UI 隐藏退出按钮
```

## 后果

- 用户可以在工具内查看设备占用并在超限时自救，20401 不再是死路。
- 误退本机设备由双重防线（UI 隐藏 + 后端拒绝）防护；误退其他在用设备的风险由单次确认文案缓解但不消除。
- Work CN 凭据调用 ListDevices 的兼容性是开放事实，实现后实测补证。
- 接口名/路径来自官方前端脚本逆向，可能随官网版本变化；实现必须保留失败降级与官方页兜底，不得把接口形状当长期契约写死。

## 验收边界

1. 20401/20408 登录失败显示「设备数量已达上限」与设备管理入口，不再显示为普通网络错误。
2. 两产品工作台都能打开设备面板（能力声明生效）；不支持的产品不渲染该模块。
3. 设备列表显示设备名、绑定产品标签、最近活跃、已用/上限；无内部标识进入主视野。
4. 本机设备行无退出按钮；后端对本机目标的退出请求返回明确拒绝。
5. 非本机设备退出：单次确认 → 成功后列表刷新 + 该账号健康检查触发。
6. mock/单测固化 ClearRefreshToken 请求形状；真机验收记录落档 `.scratch/`。
7. remint 行为与 P1-4 现状完全一致，无新增自动触网路径。
