# Windows 高性能文件搜索引擎 — 设计规格

- 日期：2026-11-17
- 状态：待用户审阅
- 平台：Windows 10 1809+ / Windows 11，x64
- 语言：Rust（edition 2021 或更新）

## 1. 目标

在 Windows 上交付一个单 exe 的桌面应用：全盘 NTFS 文件索引，输入即搜，接近 Everything 的响应速度，并具备现代化的视觉表现。

### 量化目标

| 指标 | 目标 |
|---|---|
| 首次全盘索引（500 万条） | < 15s |
| 输入到结果 P99 | < 30ms |
| USN 变更到可搜 | < 1s |
| 冷启动（mmap 预热） | < 1s |
| 进程 working set（含 UI） | < 200MB |

### 交付形态

引擎与 UI 同进程的一体化原生应用（非 IPC、非常驻服务）。UI 使用 Slint，单 exe 分发。

### 不在范围内

- 拼音、首字母、模糊匹配（typo 容忍）、正则
- 文件内容全文检索
- 按修改时间/大小排序（见 §4.2 的显式取舍）
- 非 NTFS 卷的原生索引（仅回退到目录遍历）
- 跨平台

## 2. 关键决策

| 决策 | 结论 | 理由 |
|---|---|---|
| 索引采集 | MFT 直读（`FSCTL_ENUM_USN_DATA`）+ USN Journal 增量 | 唯一能满足秒级建库 + 实时同步的路线 |
| 规模 | 全盘固定 NTFS 卷，≤500 万条 | 覆盖典型开发机/办公机 |
| 内存 | 常驻 ≤200MB，稳态纯内存 | 需紧凑布局；见 §4.1 |
| 查询能力 | 文件名子串 + 多词 AND + 通配符 | 无倒排索引即可满足延迟 |
| 匹配方式 | 全表 SIMD 子串扫描，不建倒排表 | 省内存、增量简单、Everything 验证过 |
| UI | Slint 声明式 | 颜值迭代快，单 exe，低内存 |
| 冷启动 | 磁盘快照 + mmap 预热，后台 USN 补齐 | 纯重扫需 4–8s，无法满足 <1s |
| 交付节奏 | 一次性完整实现 | 用户明确选择 |
| 权限 | 需要管理员；无权限时降级 | MFT/USN 需卷句柄 |

## 3. 架构

### 3.1 模块分层

```
fsearch-ui   (bin, Slint)     仅依赖 core 公开 API
fsearch-core (lib)
  ├─ query      解析 · 匹配 · 排序 · 高亮
  ├─ index      只读快照 · 构建器 · 名称池
  ├─ collector  Collector trait · MftScanner · UsnWatcher · DirWalker(回退)
  └─ os         卷枚举 · 提权 · NTFS ioctl · 窗口特效
```

| 单元 | 职责 | 对外接口 | 依赖 |
|---|---|---|---|
| `os` | 卷枚举、`FSCTL_*` 调用、提权检测、DWM 背景 | `list_volumes()`、`open_volume()`、`is_elevated()` | windows-sys |
| `collector` | 从卷产出扁平记录流 | `Collector::scan(&Volume) -> RecordBatch` | os |
| `index` | 名称池、快照构建、增量应用、序列化 | `IndexBuilder`、`ArcSwap<IndexSnapshot>` | collector 输出 |
| `query` | 解析、匹配、评分、高亮区间 | `query(&IndexSnapshot, &str, limit)` | index |
| `ui` | 窗口、输入、结果渲染、向导 | 订阅 `IndexHandle`，调 `QueryClient` | core |

约束：`query` 不依赖任何 Windows API，可跨平台单测；`collector` 是 trait，便于用假采集器测试。

### 3.2 线程模型

| 线程 | 职责 | 通信 |
|---|---|---|
| UI 线程 | Slint 事件循环，只渲染与转发 | 读 `IndexHandle`，收结果 channel |
| 索引线程 | 全量构建，卷内 rayon 并行 | 完成后 `index_handle.store(Arc::new(snapshot))` |
| USN 线程（每卷 1 个） | 阻塞读 journal，攒批投递 | 有界 channel → 调度线程 |
| 调度线程 | 每 100ms 应用变更生成新快照 | 原子替换 |
| 查询（rayon 池） | 分块并行扫描 | 结果带 generation 号回传 |

共享状态只有两处：`ArcSwap<IndexSnapshot>`（读侧无锁）与有界 channel。不存在读写锁。

**线程池隔离**：索引与查询各用一个独立 `rayon::ThreadPool`，索引池 `max(1, cores/2)`，查询池 `clamp(1, 8)`（即 `min(8, cores-1)`）。若共用默认全局池，全量构建会直接吃掉查询 CPU，P99 < 30ms 必然失守。

## 4. 数据模型

### 4.1 记录布局（每卷分片）

| 字段 | 类型 | 字节 | 说明 |
|---|---|---|---|
| `name_off` | u32 | 4 | 本卷名称池内偏移 |
| `parent` | u32 | 4 | 父目录记录下标；根为 `ROOT` 哨兵 |
| `file_id` | u32 | 4 | MFT 记录号低 32 位 |
| `flags` | u16 | 2 | 目录/隐藏/系统/只读 |
| `name_len` | u16 | 2 | 文件名字节长度 |
| **合计** | | **16** | 按 4 字节对齐，无填充 |

内存预算（500 万条）：

- 记录数组：80MB
- 名称池（平均 16–18 字节/名）：80–90MB
- overlay / 卷元信息 / UI：10–20MB
- **合计约 180MB**，留有余量

关键属性：

- **文件名池与记录数组严格同序**：构建时按记录下标顺序追加字节，使扫描时两条流均为顺序访问，硬件预取有效（性能前提，见 §7.2）。
- **不存完整路径**：展示时沿 `parent` 回溯拼接，配 LRU 目录路径缓存供 UI 批量取用。
- **不存大小与时间戳**：见 §4.2。
- **按 `file_id` 升序**：构建时显式排序（§5），因此增量更新定位记录可直接二分查找，无需常驻 `FileId → 下标` 哈希表（省 32–60MB）。

### 4.2 显式取舍：无 size / mtime

`ENUM_USN_DATA` 不返回文件大小与时间戳。若逐条 `GetFileInformationByHandleEx`，500 万次调用会毁掉 15s 目标。因此：

- 初始索引不含 size/mtime；
- 结果列表只对**当前可见的 top-N（≤200 条）**惰性查询属性，结果进一个小型缓存，不写回索引；
- **排序只能依据文件名匹配度**（路径层级仅在 top-200 决胜阶段使用，见 §8.4），"按修改时间排序"明确排除在范围外。

### 4.3 名称大小写

统一做 **ASCII-only 小写化**，非 ASCII 原样保留。原因：完整 Unicode 小写化会改变字节长度（如 `İ`），导致高亮偏移失效。ASCII-only 既满足日常"大小写不敏感"，又保证偏移可用。副作用：名称池只保存小写形式，**UI 显示的文件名与路径也是小写**；原始大小写还原留作后续优化（惰性加载时用 `read_dir` 回填真实名称）。

## 5. 全量构建

单卷流程：

1. `CreateFileW("\\.\C:")` 打开卷句柄；`FSCTL_GET_NTFS_VOLUME_DATA` 取 MFT 元信息；检测提权状态。
2. `FSCTL_ENUM_USN_DATA` 顺序枚举全部 MFT 记录，得到 `FileId`、`ParentFileId`、`FileName`、`FileAttributes`，并记录返回的 `NextUsn` 作为增量起点。
3. 边枚举边追加记录与名称池；`parent` 先写占位。
4. 第二遍把 `ParentFileId` 解析为记录下标；父记录不在索引内（越权/已删除/系统保留）的记录丢弃。
5. 按 `file_id` **显式排序**记录数组（不依赖枚举顺序）；随即按排序后的新顺序**重建名称池**，恢复 §4.1 要求的"记录与名称池严格同序"（90MB 顺序拷贝，约 30ms）。之后原子替换快照并触发落盘（§6）。排序 + 重建合计约 0.6s，已计入 15s 预算。

卷筛选：`GetDriveType == DRIVE_FIXED` 且文件系统为 NTFS。可选排除 `$Recycle.Bin`、`System Volume Information`。

## 6. 持久化与冷启动

- 路径：`%LOCALAPPDATA%\fsearch\index\vol-<serial>.bin`
- 头部包含：`magic`、`format_version`、卷序列号、USN Journal ID、`NextUsn`、记录数、名称池长度。
- 启动流程：
  1. mmap 快照 → 以切片形式直接暴露给查询层 → UI 可搜，满足 <1s。属性仍走 §4.2 的惰性查询，不受影响。
  2. 校验卷序列号与 Journal ID；不一致则丢弃该卷快照并转为全量重建。
  3. 以快照中的 `NextUsn` 为起点补读 USN 差异，应用 overlay。
  4. 后台把热数据读入内存生成纯内存快照并替换，随后解除 mmap。稳态回到 §4.1 的纯内存形态。
- **快照抽象**：`IndexSnapshot` 对查询层只暴露 `&[Record]` 与 `&[u8]`（名称池）两个切片，内存快照与 mmap 快照由**同一套查询代码**消费。这要求 `Record` 为 `#[repr(C)]` 纯 POD、无指针字段，且快照文件头部记录字节序，与本机 x64 小端不一致时拒绝加载。
- 写入时机：全量构建完成时、overlay 合并时、进程退出时（尽力而为）。写入采用"临时文件 + 原子重命名"，避免半截文件。
- **内存峰值说明**：mmap 预热期间映射页属于文件缓存，不计入进程 working set 的近似稳态值；验收测量在预热完成 10s 后进行。

## 7. 增量同步

### 7.1 USN 消费

- 每卷一个线程：`FSCTL_CREATE_USN_JOURNAL` 确保启用 → 循环 `FSCTL_READ_USN_JOURNAL` 阻塞等待。
- 起点为全量构建/快照中的 `NextUsn`，不重不漏。
- 关注原因位：`FILE_CREATE`、`FILE_DELETE`、`RENAME_OLD_NAME`、`RENAME_NEW_NAME`、属性变更。内容写入只需让属性缓存失效，不动索引。

### 7.2 overlay 增量层

- 主快照不可变。新增记录进入 `Vec<Record>` + **overlay 自带的独立名称池**（mmap 形态下主名称池不可写）；删除进入 tombstone 集合（记录下标的排序 `Vec<u32>`，二分判定）。变更事件先按 `file_id` 二分定位到记录下标（§4.1），再写入 overlay。
- 查询：先扫主表（跳过 tombstone），再扫 overlay 新增。overlay 常态几十到几百条，开销可忽略。
- 合并阈值：overlay 超过总量 2%（约 10 万条）或 tombstone 比例过高 → 后台合并成新主表并原子替换。
- **重命名**：`RENAME_OLD_NAME` + `RENAME_NEW_NAME` 以同一 `FileId` 配对；缓冲配对，超时 2s 则降级为删除+新增。
- **目录删除级联**：目录的 `FILE_DELETE` 事件不保证为其子树逐个上报。收到目录删除时需按 `parent` 关系级联标记整棵子树为 tombstone；实现为一次全表遍历（180MB，数十毫秒），异步执行，不阻塞查询。
- **溢出重建**：journal 删除/重建、`NextUsn` 跳变、休眠唤醒后 Journal ID 变化、读到 `ERROR_JOURNAL_DELETE_IN_PROGRESS` → 该卷判定增量链断裂，直接全量重建。

## 8. 查询引擎

### 8.1 解析

按空格切分 term（支持引号包裹短语），全部 **AND**。含 `*`/`?` 的 term 编译为通配符匹配器，否则为子串匹配。保留 `dir:`/`size:` 前缀操作符的语法扩展位，本期不实现——未知前缀按字面量处理。

### 8.2 匹配

- 子串：`memmem::find` 在 `[name_off, name_off+name_len)` 区间内查找。
- 通配符：按 `*` 切段。连续的字面量与 `?` 必须**紧邻**匹配；只有位于 `*` 之后的字面段才允许用 `memmem` 向后搜索。末尾若不是 `*`，必须锚定到名称结尾。禁止跳字符误匹配（`a?c` 不得匹配 `abbc`）。
- 多 term：首 term 全表扫描，其余 term 仅验证候选集，AND 短路。

### 8.3 性能要点

单核扫 180MB 的瓶颈是随机访存而非算法。因此强制约束"记录数组与名称池同序"（§4.1），使两条流顺序访问。按 64KB 记录块切分给 rayon，8 核预估 3–8ms。

### 8.4 排序与输出

- 固定容量最小堆维护 top-200，不收集全量后排序。
- 评分优先级：文件名完全相等 > 前缀命中 > 子串命中且位置靠前 > 文件优先于目录。**路径层级只在 top-200 的最终决胜阶段计算**（最多 200 次回溯，可忽略），不参与全表扫描打分——否则每条命中都要遍历父链。
- 输出 `Hit { record_idx, volume_id, score, highlight: SmallVec<[(u32,u32);4]> }`。
- 整轮查询绑定一个 `Arc` 快照 generation；结果回传带 generation，UI 丢弃过期结果。

## 9. UI 与视觉

### 9.1 视觉规范

- 无边框窗口、12px 圆角、自定义标题栏（拖拽区 + 最小化/关闭）。Win11 通过 `DWMWA_SYSTEMBACKDROP_TYPE` 启用 Mica；Win10 回退纯色。该能力由 `os` 层封装，UI 仅接收"背景可用"布尔量。
- 设计令牌：浅/深双主题、单一强调色、圆角仅 8/12 两档、间距走 8 的倍数；UI 字体用系统中文字体，路径用等宽字体。
- 主界面单栏，无侧边栏：
  - 顶部大号搜索框（20px，无边框，内嵌图标，聚焦时底部强调色填充动画）
  - 结果行：类型图标 · 文件名（匹配段高亮）· 次级灰色路径 · 右端大小（惰性加载后淡入）
  - 底部状态栏：索引条数 / 各卷状态点 / 上次索引耗时
  - 空态：一句引导 + 快捷键提示
- 动效：窗口淡入 150ms、结果行 60ms 错峰淡入、选中条位移动画。
- 只渲染 top-200，不依赖虚拟化控件（规避 Slint 版本间控件差异）。

### 9.2 交互

- 输入防抖 30ms 后查询。
- `↑↓` 选择、`Enter` 打开、`Ctrl+Enter` 打开所在目录、`Ctrl+C` 复制完整路径、`Esc` 清空（再按关闭）。
- 首次启动向导：申请管理员权限 + 选择索引卷。
- 全局热键唤起为可选增强，本期不实现。

## 10. 错误处理

| 场景 | 处理 |
|---|---|
| 非管理员启动 | 全盘 MFT 不可读 → 提示并提供一键 UAC 重启；同时提供"仅索引用户目录"的降级模式 |
| 非 NTFS 卷 | USN 不可用 → 该卷回退 `DirWalker` + `ReadDirectoryChangesW`，能力降级但不失败 |
| 单卷读取失败 | 该卷在 UI 标红提示，其余卷正常 |
| 内存不足 | `try_reserve` 失败时保留旧快照并提示，不崩溃 |
| 快照文件损坏/版本不符 | 丢弃该卷快照，转全量重建 |
| 索引线程 panic | 捕获后标记该卷失败，其他卷不受影响 |

## 11. 测试与验收

### 11.1 跨平台单测（CI 常态运行）

- `size_of::<Record>() == 16` 的布局守卫
- 构建后 `file_id` 升序断言
- 路径回溯拼接与目录缓存正确性
- 查询解析、通配符匹配、**高亮偏移**（覆盖中文、`İ`、emoji）
- overlay + tombstone 的增删改语义、重命名配对
- 快照序列化 round-trip、头部版本校验
- 假 `Collector` 喂 fixture 的端到端链路

### 11.2 Windows 集成测试

- 真机卷操作：卷枚举、提权检测
- USN 路径：用 VHD 挂载卷验证 create/delete/rename/溢出重建

### 11.3 验收门槛（真机）

| 项 | 门槛 | 测量方式 |
|---|---|---|
| 全量索引 500 万条 | < 15s | 冷缓存首次构建计时 |
| 查询 P99 | < 30ms | 100 条混合样本（含通配符），预热后 |
| USN → 可搜 | < 1s | 外部创建文件计时 |
| 冷启动 | < 1s | 从进程启动到可输入 |
| 内存 | < 200MB | 预热完成 10s 后 working set |

基准不进 CI 门禁（机器差异大），用 `cargo bench` + 报告归档。

## 12. 依赖（实现阶段用 `cargo add` 锁定版本并验证）

| 用途 | 候选 crate |
|---|---|
| Win32 API | `windows-sys` |
| UI | `slint` |
| 并行 | `rayon` |
| 字节搜索 | `memchr` |
| 无锁快照 | `arc-swap` |
| 线程通道 | `crossbeam-channel` |
| mmap | `memmap2` |
| 小容器 | `smallvec` |
| 哈希（构建期） | `ahash` |

以上均为候选，实现前需确认许可证、目标平台支持与最新兼容版本。

## 13. 风险与后备方案

| 风险 | 影响 | 后备 |
|---|---|---|
| 内存实测超 200MB | 违背验收 | 名称池启用 8 字节分块共享前缀（front coding），预估再省 25% |
| `ENUM_USN_DATA` 吞吐低于预期 | 索引慢于 15s | 改为直接解析 `$MFT` 原始记录（可同时拿到 size/mtime），实现量显著增加 |
| USN 在部分机型频繁溢出 | 反复全量重建 | 缩短合并周期、提高 overlay 阈值、增加重建并发度 |
| Slint 无边框/透明窗口在 Win10 表现不佳 | 视觉降级 | Win10 走纯色背景分支；不接受则转 egui 方案（引擎不受影响） |
| 查询随机访存成为瓶颈 | P99 超标 | 引入"上一轮结果集复用"（前缀扩展时只在候选集重扫） |

## 14. 实现状态（2026-11-17）

**已在 Linux aarch64 验证**：`fsearch-core` 全部模块（模型、名称池、索引构建、路径解析、查询解析/匹配/排序、快照序列化、mmap 加载、overlay、重命名配对、USN 解析与事件分类、引擎、惰性属性）。`cargo test` 68 passed / 0 failed；`cargo clippy -D warnings` 无告警；`cargo fmt --check` 通过。

**已实现但未验证**：

- `fsearch-win`：卷枚举、提权、MFT 枚举、USN 监听、DWM 背板。仅 `cargo check` 通过（Linux 下 crate 为空），需 Windows x64 真机编译与运行。
- `fsearch-ui`：Slint 界面、防抖、结果行拆分、动作、视觉令牌。Linux 上因缺 `fontconfig` 开发库无法编译（Slint 的 Linux 后端依赖），必须在 Windows 上 `cargo build --release -p fsearch-ui` 验证。

**与设计不一致的已知限制**：

1. **单卷索引**：`IndexEngine` 当前持有单个 `IndexSnapshot`，UI 只索引第一个 NTFS 卷。多卷容器（`Vec<Arc<IndexSnapshot>>` + 跨卷合并 top-N）尚未实现，是"全盘"目标的主要缺口。
2. **名称显示为小写**：见 §4.3 副作用说明。
3. **通配符语义**：实现为"紧邻 + 仅 `*` 后可跳跃"，比原设计的锚定策略更严格也更正确（`a?c` 不再误匹配 `abbc`）。
4. **查询线程池**：上限从 4 调整为 8（§3.2 已同步）。

**性能实测（本机 2 核，查询池仅 1 线程）**：

- 100 万条合成数据：索引构建 351ms，查询 p50 26.6ms、p99 38.7ms。
- 单核吞吐约 26M 记录/秒。按 8 核 7 线程线性外推，500 万条约 27ms，理论上刚好达到 P99 < 30ms；**必须在目标 Windows 机器上用 `cargo run --release --bin bench -- 5000000` 复测确认**。
- 冷启动 < 1s、USN < 1s、内存 < 200MB 三项均未验证（依赖 mmap 落盘流程在 UI 侧接线，当前 UI 仅在内存中构建快照）。

### 打包（Windows）

本项目的构建环境是 aarch64 Linux，缺少 MSVC / mingw 链接器，`dnf` 源也不可达，因此**无法在构建环境内交叉编译出 Windows exe**。打包必须在 Windows x64 上完成：

1. 安装 Rust stable（rustup）与 Visual Studio Build Tools（勾选"使用 C++ 的桌面开发"）。
2. 仓库根目录执行 `scripts\build-exe.cmd`，或 `powershell -ExecutionPolicy Bypass -File scripts\build-exe.ps1`。
3. 产物为 `dist\fsearch.exe`。

根 `Cargo.toml` 的 release profile 已启用 `lto = "thin"`、`codegen-units = 1`、`strip = true`、`panic = "abort"`；二进制名由 `crates/fsearch-ui/Cargo.toml` 中的 `[[bin]] name = "fsearch"` 决定。首次构建会下载 Slint 与其依赖，耗时较长。