# ADR-0029：多产品工作台边界与产品适配状态

## 状态

Accepted，2026-09-22。基于 `.scratch/research-multi-product-adaptation-20260922.md` 的本地只读探测、官方产品资料和网页 Agent 复核结果建立。本 ADR 不承诺跨产品凭据自动迁移；任何未来改变该边界的决定必须另开 ADR。

## 背景

Trae Sync 原定位是 TRAE Work CN 多账号日常管理工作台。现在需要逐步承载 Trae CN 等产品，但 Work CN 与 Trae CN 具有不同的安装包类型、认证通道、数据根和能力集合。两套客户端的本地认证 blob 字段结构相似，不能据此推出 token、设备身份或 native storage 可以直接跨产品使用。

## 决策

1. Trae Sync 的壳层可以承载多个 TRAE 产品，但每个产品必须有独立的 `ProductContext`。上下文至少隔离产品 ID、数据根、数据库定位、可执行文件、能力清单、当前产品账号和最近页面。
2. 产品切换器与账号切换器是两个独立状态：产品切换回答“操作哪个产品”，账号切换回答“该产品使用哪个账号”。产品切换器放在标题栏左侧；账号选择贴近当前产品账号区域。
3. 产品切换时必须销毁旧产品的数据库句柄、会话缓存、详情对象和未完成的数据请求，再加载新产品上下文。每个产品保留自己的最近页面和当前账号，但不得把旧产品的页面详情或数据库对象带入新产品。
4. 每个产品通过 `CapabilityManifest` 声明真实能力及状态。导航由当前产品能力生成；Trae CN 当前只显示已适配的账号相关能力，未适配签到不进入主导航，不用“点击后失败”的假入口。
5. `AccountIdentity` 只表示 provider、userId 和非秘密展示信息等稳定身份摘要；它不是 token、refresh token、设备密钥或 native storage blob，也不能单独表示某个产品已经登录。
6. `ProductCredential` 按 `productId + identityId` 独立记录认证状态。同一身份可以同时拥有 Work CN `verified` 与 Trae CN `need_authorization`；“身份可识别”与“产品会话可恢复”是两个不同验收状态。
7. 当前 V1.1 默认实现“身份发现 + 引导重新授权”。在严格隔离目录中满足同一身份、恢复成功、退出重启仍成功、无生产文件修改且未触发设备状态变化之前，不得把 Trae CN 自动恢复标为 `verified`，也不得向用户承诺 Work 凭据可直接用于 Trae CN。
8. 禁止通过复制或改写 access token、refresh token、native storage blob、device key、MachineID 或设备私钥实现跨产品登录；禁止对真实客户端运行时目录、生产数据库、WAL/SHM 做迁移实验。
9. 签到是产品能力和产品权益，不是账号全局属性。当前签到继续由 Work CN 管理；Trae CN 不显示签到入口。未来若服务端拆分产品资格、设备额度或账号状态，使用 `ProductEntitlement(productId, identityId)` 分开表达。
10. 跨产品数据库、WAL、会话和凭据不自动合并。Trae CN 可以先做只读数据库适配和账号身份发现，但不能越过产品边界写入 Work CN 数据或把两边会话合并成全局主库。

## 最小模型

```text
ProductContext
  product_id, data_root, db_path, executable_path
  capability_manifest, active_identity_id?, last_page

CapabilityManifest
  account, account_switch, signin, conversation, environment

AccountIdentity
  identity_id, provider, user_id, display_name

ProductCredential
  product_id, identity_id, status, credential_type, last_verified_at?
```

状态流固定为：

```text
unknown → identity_recognized → restore_testing
                         ├→ verified
                         ├→ need_authorization
                         └→ failed
```

`verified` 的门槛是：同一 provider/userId 已确认；适配器在临时目录成功恢复；生产数据未改写；隔离环境退出并重新打开后仍有效；实验未触发签到、设备注册、设备重铸或 refresh rotation。

## 后果

- Work CN 的现有功能继续是完整支持面，产品适配不会把 Work 专用判断散落到 UI。
- 产品切换可以在不污染数据库和页面状态的前提下逐步实现，后续产品只需提供边界内的适配器。
- Trae CN 第一阶段可以提供真实的账号/身份/健康检测/凭据维护能力，即使官方 native session restore 尚未证明，也不会阻塞产品壳层建设。
- 同一账号身份不再被误解为所有产品都已登录、签到或拥有相同权益。
- 如果未来完成隔离恢复实验，需要另行记录实验事实并单独评估是否开放 `verified`；本 ADR 本身不授予写生产凭据的权限。

## 实现备注（2026-09-22）

本 ADR 的“不得自动迁移凭据”边界不阻止 Trae Sync 调用 Trae CN 自己的 OAuth 通道。第二切片已按该边界实现：Trae CN 使用独立 client/参数、独立加密凭据根目录、独立账号注册表和当前账号指针；系统浏览器只作为授权入口，不能视为 Work 本地凭据复用。官方 Trae CN native storage 写回和关闭重启后的原生会话恢复仍保持未验证状态。

第三切片（2026-09-22）已把产品账号入口收敛为共享 `AccountWorkbench`：App 不再按产品挂载两套账号页，Trae CN 的账号快照、登录、取消登录和切换由小型 `AccountAdapter` 提供，Work 的签到、额度、详情和主库切号事务继续留在 Work 语义内。`ProductAccountPage` 暂作为迁移行为基线保留，不再是生产路由；Work 专有 DTO 尚未被强行抽成跨产品模型。

第四切片（2026-09-22）把健康检测与凭据维护纳入共享账号模块：两种产品都可以在同一位置查看登录状态、执行健康检测和刷新登录凭据；适配器负责选择产品自己的 OAuth client、凭据根和服务端命令。Trae CN 不显示额度与签到。Trae CN 的真实续期链路已按 `TraeCn` client 接入，但尚未把一次真实生产续期结果当作协议验收证据；失败必须显示为需要重新授权或稍后重试，不得伪称跨产品凭据迁移成功。

## 验收边界

进入 V1.1 实现的必要验收：

1. Work/CN 产品切换后标题、导航、数据源均对应当前产品。
2. Trae CN 不显示签到；Work CN 原有签到流程不变。
3. 产品切换不复用旧产品数据库句柄、会话详情或缓存。
4. Work `verified` 与 Trae CN `need_authorization` 可以并存。
5. 任何未通过隔离验证的状态都不能显示为“已登录”或“自动迁移成功”。

暂不进入 V1.1：自动复制凭据、跨产品写库、全局统一聊天库、统一签到入口和泛化插件平台。
