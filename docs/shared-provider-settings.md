# 共享模型服务 UI 设计与实施计划

## 目标与边界

以 ThinkRail Native 实际 providers 页面为结构与交互参考。LumaDesk 与 Native 接入同一份服务商栏、详情、密钥、线路、模型列表、同步选择及编辑弹层组件，删除各自相同的布局实现。保留两个宿主现有数据库、加密、协议、模型能力、价格、轮换策略和请求执行；不迁移用户数据，不合并后端。

通用视觉控件继续由 lumakit-design 提供。模型服务功能组合放在现有 lumakit-gpui::providers 私有子模块，公开窄的展示组件。宿主传入自己的控件事件处理、已保存状态、错误及业务专有字段；共享模块不读数据库、网络或密钥存储，不引入第二份业务状态机。两个宿主已有的高级模型字段不同，公共表单外壳与共同字段排列共享，专有字段作为明确的扩展内容保留，不显示无消费者的字段。

## 组件合同

- page：服务商栏与详情的响应式组合；窄窗上下排列，最小宽度为零，保留页面滚动。
- rail / provider_row：搜索与筛选、头像、选中态、状态点、更多菜单、新建服务商。
- detail / header：名称、服务商设置、独立启用；密钥、策略说明、线路、模型区按固定顺序排列。
- key_section / saved-key controls：首钥输入、获取密钥、管理、检测，默认无额外“加密保存”主按钮；已保存显示遮罩，显式查看/复制/编辑。检测结果放在密钥区，检测中保留页面。
- route_section：添加线路、可选线路选择、地址、线路设置、请求地址/错误说明。
- models：公共工具栏、分组标题与行、空状态；宿主提供真实能力元数据、筛选结果、已选状态及操作，公共组件不猜测能力。
- forms：公共字段布局、固定标题/滚动内容/固定操作、同步目录行与操作；业务校验与提交留在宿主。

组件接收 lumakit-design 的语义 Roles，不安装第二套主题。LumaDesk 从 LumaTheme 传入；Native 在宿主边界把现有同名语义角色转换成共享 Roles。所有尺寸来自 lumakit-design::metrics；不新增主题或十六进制颜色。

## 状态与敏感信息

1. 首钥按 Enter 或失焦提交；空值不删除已有密钥。保存成功才清空输入；失败保留草稿并提供重试，不伪造成功。
2. 提交绑定原服务商/线路身份；异步结果不能覆盖后来选中的对象或新输入。不因重绘重复提交；保存中阻止重复提交。
3. 服务商启用独立保存，不顺便提交地址/模型草稿。管理弹层每个密钥都有真实 Switch；停用状态持久化，停用密钥不进入聊天/生图、检测或失败轮换。全部停用但仍存有密钥时明确拒绝请求，不降级为匿名。旧密钥迁移默认启用。
4. 检测只读已保存配置；检测不替换已保存模型；同步逐项提交、失败回滚；保留取消与诊断能力。
5. 明文不进入普通页面快照、日志、Debug DTO。离开/取消清理明文；剪贴板与显式查看保持现有安全边界。
6. 通用组件使用可聚焦的原生 Kit 控件，图标有可访问名称；键盘焦点不依赖鼠标悬停才可找到操作。
7. 多密钥标签与密钥值分离：管理列表按“标签 / 遮罩 / 复制 / 编辑 / 删除 / 宿主选择控件”排列；输入为标签、遮罩密钥、添加/保存同一紧凑行。标签可选、去除首尾空白、最多 128 个 Unicode 字符且不允许控制字符。编辑时回填标签，密钥留空保持原密文；成功后清空，失败保留。旧密钥标签为空，追加 SQLite 列迁移不解密/重加密。主页面显示已选标签，不提供删除动作；管理弹层不额外提供显示原密钥的眼睛按钮。
8. 密钥管理使用约 520 逻辑宽度的内容自适应短弹层，不强制撑满设置页高度；窄窗换行、长列表滚动，关闭保持可达。

## 迁移顺序与文件职责

- 先实现共享合同与组件；不新建 crate、泛型设置引擎、仓储接口或插件机制。
- LumaDesk 将超过 1200 行的 view.rs 按输入事件、服务商、线路、模型和页面编排拆为私有模块，移除已上移的布局；现有 ModelEditor 保留业务状态，接入公共编辑布局。
- Native providers 按原私有模块迁移，同步修复首钥异步失败丢草稿；只适配共享显示，不重写 ProviderService。
- 两边完成后删除废弃 helper；不保留两套页面或兼容别名。
- Native 使用已推送的 LumaKit 完整 Git rev；本机联调只允许临时 patch，交付不得包含本地路径 patch。

## 验收

定向编译与现有密钥保存/失败回滚/启用独立/检测同步测试；新增回归仅覆盖保存目标和异步失败草稿恢复等真实边界。真实两个宿主用隔离数据和本地回环服务验证：未配置/已保存、保存失败重试、检测成功/失败/取消、弹层取消与保存、服务商线路切换、键盘和焦点。覆盖 320/375/414/768 逻辑宽度并保存实窗证据；未观察不得标通过。最后更新实现记录并正式打包 LumaDesk。

状态：完成。共享代码已推送，Native固定依赖验证通过；LumaDesk正式构建、测试、发布及哈希校验完成，发布EXE已实窗确认标签和Switch。

### 已运行证据

- LumaKit `cargo-msvc.bat test -p lumakit-ai catalog_store --manifest-path ..\LumaKit\Cargo.toml --target-dir target --locked`：9 项通过；覆盖旧库/JSON、重启、标签变更/清空及 SQLite 失败回滚。
- `test -p lumakit-support secrets`：3 项通过；LumaDesk `test --locked model_settings::tests`：14 项通过；共享 workspace 与宿主严格 clippy 均通过。`lumakit-gpui --no-default-features` 严格 clippy 通过，不引入聊天后端。
- 实窗用旧 schema 2 的隔离 `providers.db` 启动 LumaDesk，追加标签列后原密钥 ID/密文字节一致；通过管理弹层新增第二个标签密钥、Enter 只改原标签，密钥留空且密文未变。临时 SQLite trigger 制造实际写入失败，弹层保留 `unsaved-label`，移除 trigger 后原草稿重试成功。
- 对照未修改 Native 发布程序的主页面/管理弹层截图：主行无删除、copy/edit/eye，管理行 label/mask/copy/edit/trash/Switch、无查看眼睛；管理表单改为 label/secret/submit 同行、520 宽内容自适应短弹层。LumaDesk 旧首选按钮移除，启用 Switch 与已启用计数和 Native 对齐。
- LumaDesk 本轮证据位于 `C:/Users/Administrator/AppData/Local/Temp/provider-label-final-7ey4rrsm/`；原 Native 对照位于 `C:/Users/Administrator/AppData/Local/Temp/shared-provider-ui-cda1gesx/native-reference-*.png`。均为隔离假密钥和本地回环，不是生产凭据。
- Switch 最终验证：LumaDesk 模型设置17项通过，Kit catalog筛选14项及相关集成、secrets4项通过。实窗 schema3→4 默认启用；逐项停用显示0/2、启用一项显示1/2，全部停用检测未收到任何回环请求。SQLite trigger拒写时开关恢复，重启后0/1状态、标签、ID和密文字节均保持。最终截图 `desk-key-switches-wide.png`、`desk-switch-manager-final-{320,375,414,768}.png`、`desk-switch-save-failure.png`、`desk-switch-restart.png`。
- Native 使用原后端真实启用状态；最终 `KeyRow::trailing` 将 Switch 放在同一边框内。实窗添加/标签修改/密钥替换/重启、Switch切换0/1→1/1、320/375/414/768已观察。证据 `C:/Users/Administrator/AppData/Local/Temp/native-keys-finish-l9kw_acv/screenshots/key-manager-bordered-switch-{320,wide}.png` 及 `*-final-{320,375,414,768}.png`。原本污染的中间 corrected截图不作为证据。Native workspace严格clippy及全部测试通过（16个既有外部依赖ignored保持不变）。
- 共享实现发布提交 `f263608cdc5efcb107ab7d3d060a9caba85d0afa`；Native所有LumaKit依赖固定同一rev，新增 `lumakit-gpui` 关闭chat默认feature。远程来源all-targets check、严格宿主clippy、提供方5项回归通过；workspace实测337项通过、16项既有ignored（使用同份本地共享源联调阶段）。无关 codespan 的unicode-width锁定关系保持原样，未升级外部依赖。
- 正式发布 `scripts/build-app.bat` 完成，宿主release43项、Kit release全套通过；构建标识 `0.1.0-0a0f40c28e5647f7ad7a6e2abc781005`，EXE SHA-256 `5529ac2f8348734338619dbcf6877d2eb44b1e7c145798f93d4bcb3a26e6d3b2`。发布EXE实际打开密钥管理确认标签、Switch和计数；EXE/ZIP/包内EXE校验通过。生产配置未改写。

### 模块规模与职责

共享 `providers`：页面组合160行、服务商栏131、模型列表106、表单142；均只负责展示和回调。`lumakit-design/settings/keys.rs` 约460行，包含同一凭据展示职责的 KeyRow/KeyManager；新增显式颜色/标签/尾部控件，未新增存储或请求状态。目录存储从688行拆为迁移162、事务242、读取146、校验75；schema4在迁移入口追加。宿主 `model_settings.rs` 405行为目录/任务唯一所有者，`view.rs`290；超过300的密钥332、模型列表344、同步选择304分别保留各自业务回调，模型编辑器335为同一草稿/校验所有者；没有超过500行的新增生产模块。测试按合同独立组织，不为满足行数移动资源所有权。
