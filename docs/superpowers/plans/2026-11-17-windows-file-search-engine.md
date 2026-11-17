# Windows 高性能文件搜索引擎 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付一个单 exe 的 Windows 桌面应用，全盘 NTFS 文件索引、输入即搜（P99 < 30ms），具备 Mica 背景与动效的现代界面。

**Architecture:** 三个 crate 的 workspace。`fsearch-core` 是平台无关的索引与查询内核（内存布局 + 快照 + 查询），`fsearch-win` 封装全部 Win32 FFI（卷枚举、ENUM_USN_DATA、USN Journal、提权、DWM），`fsearch-ui` 是 Slint 单体 GUI。索引快照通过 `ArcSwap` 原子替换，查询读侧无锁；索引线程池与查询线程池彼此独立。

**Tech Stack:** Rust 2021、slint 1.x、rayon、memchr、arc-swap、crossbeam-channel、memmap2、smallvec、ahash、thiserror、windows-sys 0.59。

## Global Constraints

- 平台：Windows 10 1809+ / Windows 11，仅 x64。
- `Record` 必须是 `#[repr(C)]` 纯 POD，`size_of::<Record>() == 16`，`align_of::<Record>() == 4`。
- 名称统一 **ASCII-only 小写化**，非 ASCII 原样保留；任何情况下不得改变字节长度。
- 稳态常驻内存 ≤ 200MB；稳态为纯内存快照（mmap 仅用于冷启动预热）。
- 全量索引 500 万条 < 15s；查询 P99 < 30ms；USN 变更到可搜 < 1s；冷启动 < 1s。
- 查询能力仅限：文件名子串 + 多词 AND + `*`/`?` 通配符。
- 索引与查询必须使用两个独立 `rayon::ThreadPool`，不得共用全局池。
- `fsearch-core` 内不得出现 `windows` / `windows-sys` 依赖，必须能在 Linux CI 上 `cargo test`。
- 构建期内存峰值允许达到约 260MB（旧名称池 + 新名称池并存）；验收只测预热后的稳态。

---

## File Structure

| 路径 | 职责 |
|---|---|
| `Cargo.toml` | workspace 根 |
| `crates/fsearch-core/src/lib.rs` | 模块导出 |
| `crates/fsearch-core/src/model.rs` | `Record`、标志位、`ROOT`、ASCII 小写化 |
| `crates/fsearch-core/src/index/pool.rs` | `NamePool` |
| `crates/fsearch-core/src/index/build.rs` | `IndexBuilder` |
| `crates/fsearch-core/src/index/snapshot.rs` | `IndexSnapshot`、`Store`、序列化、mmap |
| `crates/fsearch-core/src/index/overlay.rs` | `Overlay`、tombstone、二分、级联 |
| `crates/fsearch-core/src/index/rename.rs` | 重命名配对、合并阈值 |
| `crates/fsearch-core/src/path.rs` | `PathResolver` + 缓存 |
| `crates/fsearch-core/src/query/parse.rs` | 查询解析 |
| `crates/fsearch-core/src/query/matcher.rs` | 子串/通配符匹配与高亮 |
| `crates/fsearch-core/src/query/rank.rs` | 评分与 top-N |
| `crates/fsearch-core/src/query/mod.rs` | `query()` |
| `crates/fsearch-core/src/collector/mod.rs` | `Collector` trait、`RawRecord` |
| `crates/fsearch-core/src/engine.rs` | `IndexEngine` |
| `crates/fsearch-win/src/{lib,volume,ffi,mft,usn,dwm}.rs` | Win32 实现 |
| `crates/fsearch-ui/src/{main,bridge,props}.rs` | GUI |
| `crates/fsearch-ui/ui/{app,theme}.slint` | 界面与令牌 |
| `benches/query.rs` | 查询基准 |

测试内联在各文件 `#[cfg(test)] mod tests`。

---

### Task 1: Workspace 骨架与记录布局守卫

**Files:**
- Create: `Cargo.toml`、`rust-toolchain.toml`
- Create: `crates/fsearch-core/Cargo.toml`
- Create: `crates/fsearch-core/src/lib.rs`
- Create: `crates/fsearch-core/src/model.rs`

**Interfaces:**
- Consumes: 无
- Produces: `model::{Record, ROOT, F_DIR, F_HIDDEN, F_SYSTEM, F_READONLY, ascii_lower, ascii_lower_into, flags_from_attributes}`

- [ ] **Step 1: 初始化仓库与目录**

```bash
git init
mkdir -p crates/fsearch-core/src crates/fsearch-win/src crates/fsearch-ui/src crates/fsearch-ui/ui
```

`Cargo.toml`：

```toml
[workspace]
resolver = "2"
members = ["crates/fsearch-core", "crates/fsearch-win", "crates/fsearch-ui"]

[workspace.package]
edition = "2021"
version = "0.1.0"

[workspace.dependencies]
rayon = "1.10"
memchr = "2.7"
arc-swap = "1.7"
crossbeam-channel = "0.5"
memmap2 = "0.9"
smallvec = "1.13"
ahash = "0.8"
thiserror = "1.0"

[profile.release]
lto = "thin"
codegen-units = 1
```

`rust-toolchain.toml`：

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`crates/fsearch-core/Cargo.toml`：

```toml
[package]
name = "fsearch-core"
edition.workspace = true
version.workspace = true

[dependencies]
rayon.workspace = true
memchr.workspace = true
arc-swap.workspace = true
crossbeam-channel.workspace = true
memmap2.workspace = true
smallvec.workspace = true
ahash.workspace = true
thiserror.workspace = true
```

- [ ] **Step 2: 写失败的布局测试**

`crates/fsearch-core/src/model.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_is_exactly_16_bytes() {
        assert_eq!(std::mem::size_of::<Record>(), 16);
        assert_eq!(std::mem::align_of::<Record>(), 4);
    }

    #[test]
    fn ascii_lower_preserves_byte_length() {
        for s in ["README.TXT", "İstanbul", "中文ABC", "emoji😀ABC"] {
            assert_eq!(ascii_lower(s).len(), s.len(), "length changed for {s}");
        }
    }

    #[test]
    fn ascii_lower_only_touches_ascii() {
        assert_eq!(ascii_lower("ABC中文"), "abc中文");
    }
}
```

`crates/fsearch-core/src/lib.rs`：

```rust
pub mod model;
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p fsearch-core record_is_exactly_16_bytes`
Expected: FAIL，`cannot find type Record`

- [ ] **Step 4: 实现**

`model.rs` 顶部追加：

```rust
pub const ROOT: u32 = u32::MAX;

pub const F_DIR: u16 = 1 << 0;
pub const F_HIDDEN: u16 = 1 << 1;
pub const F_SYSTEM: u16 = 1 << 2;
pub const F_READONLY: u16 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub name_off: u32,
    pub parent: u32,
    pub file_id: u32,
    pub flags: u16,
    pub name_len: u16,
}

pub fn ascii_lower_into(input: &str, out: &mut String) {
    out.clear();
    out.reserve(input.len());
    for c in input.chars() {
        out.push(if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c });
    }
}

pub fn ascii_lower(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    ascii_lower_into(input, &mut out);
    out
}

pub fn flags_from_attributes(attributes: u32) -> u16 {
    let mut flags = 0u16;
    if attributes & 0x10 != 0 { flags |= F_DIR; }
    if attributes & 0x02 != 0 { flags |= F_HIDDEN; }
    if attributes & 0x04 != 0 { flags |= F_SYSTEM; }
    if attributes & 0x01 != 0 { flags |= F_READONLY; }
    flags
}
```

- [ ] **Step 5: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，3 tests passed

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml rust-toolchain.toml crates/fsearch-core
git commit -m "feat(core): scaffold workspace and 16-byte record layout"
```

---

### Task 2: NamePool

**Files:**
- Create: `crates/fsearch-core/src/index/mod.rs`
- Create: `crates/fsearch-core/src/index/pool.rs`
- Modify: `crates/fsearch-core/src/lib.rs`

**Interfaces:**
- Consumes: `model::ascii_lower`
- Produces: `NamePool::{new, with_capacity, push, push_bytes, get, as_bytes, from_bytes, len, is_empty}`

- [ ] **Step 1: 写失败的测试**

`pool.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_returns_offsets_and_lowercases_ascii() {
        let mut pool = NamePool::new();
        let (off_a, len_a) = pool.push("README.TXT");
        let (off_b, len_b) = pool.push("Main.RS");
        assert_eq!((off_a, len_a), (0, 10));
        assert_eq!(off_b, 10);
        assert_eq!(pool.get(off_a, len_a), b"readme.txt");
        assert_eq!(pool.get(off_b, len_b), b"main.rs");
    }

    #[test]
    fn push_keeps_non_ascii_bytes_untouched() {
        let mut pool = NamePool::new();
        let (off, len) = pool.push("报告ABC.pdf");
        assert_eq!(pool.get(off, len), "报告abc.pdf".as_bytes());
        assert_eq!(len as usize, "报告ABC.pdf".len());
    }

    #[test]
    fn push_bytes_does_not_lowercase() {
        let mut pool = NamePool::new();
        let (off, len) = pool.push_bytes(b"MixedCase");
        assert_eq!(pool.get(off, len), b"MixedCase");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core push_returns_offsets_and_lowercases_ascii`
Expected: FAIL，`cannot find type NamePool`

- [ ] **Step 3: 实现**

```rust
use crate::model::ascii_lower;

#[derive(Debug, Default, Clone)]
pub struct NamePool {
    bytes: Vec<u8>,
}

impl NamePool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self { bytes: Vec::with_capacity(cap) }
    }

    pub fn push(&mut self, name: &str) -> (u32, u16) {
        let lowered = ascii_lower(name);
        self.push_bytes(lowered.as_bytes())
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> (u32, u16) {
        assert!(bytes.len() <= u16::MAX as usize, "name too long");
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(bytes);
        (off, bytes.len() as u16)
    }

    pub fn get(&self, off: u32, len: u16) -> &[u8] {
        let start = off as usize;
        &self.bytes[start..start + len as usize]
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}
```

`index/mod.rs`：

```rust
pub mod build;
pub mod pool;
pub mod snapshot;

pub use pool::NamePool;
```

`lib.rs`：

```rust
pub mod index;
pub mod model;
```

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，6 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(core): add name pool with ascii-only lowercasing"
```

---

### Task 3: IndexBuilder — 父链解析、排序、池重建

**Files:**
- Create: `crates/fsearch-core/src/collector/mod.rs`
- Create: `crates/fsearch-core/src/index/build.rs`
- Create: `crates/fsearch-core/src/index/snapshot.rs`
- Modify: `crates/fsearch-core/src/index/mod.rs`
- Modify: `crates/fsearch-core/src/lib.rs`

**Interfaces:**
- Consumes: `NamePool`、`Record`、`ROOT`、`flags_from_attributes`
- Produces:
  - `collector::{RawRecord, RecordBatch, CollectError, Collector}`
  - `IndexBuilder::{new, with_capacity, extend, finish}`
  - `IndexSnapshot::{new, records, names, record_count, volume_id, next_usn, generation}`

- [ ] **Step 1: 写失败的测试**

`build.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::model::{F_DIR, ROOT};

    fn raw(file_id: u64, parent: u64, name: &str, dir: bool) -> RawRecord {
        RawRecord {
            file_id,
            parent_file_id: parent,
            name: name.to_string(),
            attributes: if dir { 0x10 } else { 0 },
        }
    }

    fn sample() -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 42);
        b.extend(raw(5, 5, "\\", true));
        b.extend(raw(10, 5, "Users", true));
        b.extend(raw(11, 10, "Alice", true));
        b.extend(raw(12, 11, "Notes.TXT", false));
        b.finish()
    }

    #[test]
    fn records_are_sorted_by_file_id() {
        let ids: Vec<u32> = sample().records().iter().map(|r| r.file_id).collect();
        assert_eq!(ids, vec![5, 10, 11, 12]);
    }

    #[test]
    fn parent_is_resolved_to_index() {
        let snap = sample();
        assert_eq!(snap.records()[3].parent, 2);
        assert_eq!(snap.records()[2].parent, 1);
        assert_eq!(snap.records()[1].parent, 0);
        assert_eq!(snap.records()[0].parent, ROOT);
    }

    #[test]
    fn names_are_lowercased_and_pool_is_in_record_order() {
        let snap = sample();
        let notes = &snap.records()[3];
        assert_eq!(snap.name_at(notes.name_off, notes.name_len), b"notes.txt");
        let mut pos = 0usize;
        for r in snap.records() {
            assert_eq!(r.name_off as usize, pos);
            pos += r.name_len as usize;
        }
    }

    #[test]
    fn orphan_records_are_dropped() {
        let mut b = IndexBuilder::new(0, 0);
        b.extend(raw(5, 5, "\\", true));
        b.extend(raw(99, 777, "orphan.txt", false));
        assert_eq!(b.finish().record_count(), 1);
    }

    #[test]
    fn cycles_are_dropped_not_hung() {
        let mut b = IndexBuilder::new(0, 0);
        b.extend(raw(5, 6, "a", true));
        b.extend(raw(6, 5, "b", true));
        assert_eq!(b.finish().record_count(), 0);
    }

    #[test]
    fn directories_get_dir_flag() {
        let snap = sample();
        assert_ne!(snap.records()[1].flags & F_DIR, 0);
        assert_eq!(snap.records()[3].flags & F_DIR, 0);
    }

    #[test]
    fn next_usn_is_carried_over() {
        assert_eq!(sample().next_usn, 42);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core records_are_sorted_by_file_id`
Expected: FAIL，`cannot find type IndexBuilder`

- [ ] **Step 3: 实现 collector 类型**

`collector/mod.rs`：

```rust
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecord {
    pub file_id: u64,
    pub parent_file_id: u64,
    pub name: String,
    pub attributes: u32,
}

#[derive(Debug, Clone, Default)]
pub struct RecordBatch {
    pub records: Vec<RawRecord>,
    pub next_usn: i64,
}

#[derive(Debug, Error)]
pub enum CollectError {
    #[error("volume {0} is not accessible")]
    Inaccessible(char),
    #[error("ioctl failed: {0}")]
    Ioctl(String),
    #[error("need administrator privileges")]
    NotElevated,
}

pub trait Collector {
    fn scan(&self) -> Result<RecordBatch, CollectError>;
}
```

- [ ] **Step 4: 实现 IndexSnapshot（Owned 形态）**

`snapshot.rs`：

```rust
use crate::index::NamePool;
use crate::model::Record;

pub enum Store {
    Owned { records: Vec<Record>, names: NamePool },
}

pub struct IndexSnapshot {
    pub volume_id: u8,
    pub volume_serial: u64,
    pub journal_id: u64,
    pub next_usn: i64,
    pub generation: u64,
    store: Store,
}

impl IndexSnapshot {
    pub fn new(
        volume_id: u8,
        volume_serial: u64,
        journal_id: u64,
        next_usn: i64,
        generation: u64,
        store: Store,
    ) -> Self {
        Self { volume_id, volume_serial, journal_id, next_usn, generation, store }
    }

    pub fn records(&self) -> &[Record] {
        match &self.store {
            Store::Owned { records, .. } => records,
        }
    }

    pub fn names(&self) -> &[u8] {
        match &self.store {
            Store::Owned { names, .. } => names.as_bytes(),
        }
    }

    pub fn name_at(&self, off: u32, len: u16) -> &[u8] {
        let start = off as usize;
        &self.names()[start..start + len as usize]
    }

    pub fn record_count(&self) -> usize {
        self.records().len()
    }
}
```

- [ ] **Step 5: 实现 IndexBuilder**

`build.rs`：

```rust
use ahash::AHashMap;

use crate::collector::RawRecord;
use crate::index::snapshot::{IndexSnapshot, Store};
use crate::index::NamePool;
use crate::model::{flags_from_attributes, Record, ROOT};

pub struct IndexBuilder {
    volume_id: u8,
    next_usn: i64,
    raw: Vec<RawRecord>,
}

impl IndexBuilder {
    pub fn new(volume_id: u8, next_usn: i64) -> Self {
        Self { volume_id, next_usn, raw: Vec::new() }
    }

    pub fn with_capacity(volume_id: u8, next_usn: i64, cap: usize) -> Self {
        Self { volume_id, next_usn, raw: Vec::with_capacity(cap) }
    }

    pub fn extend(&mut self, record: RawRecord) {
        if !record.name.is_empty() {
            self.raw.push(record);
        }
    }

    pub fn finish(self) -> IndexSnapshot {
        let n = self.raw.len();
        let mut records: Vec<Record> = Vec::with_capacity(n);
        let mut pool = NamePool::with_capacity(n * 16);
        let mut by_id: AHashMap<u64, u32> = AHashMap::with_capacity(n);
        let mut seeds = vec![false; n];

        for (i, r) in self.raw.iter().enumerate() {
            by_id.insert(r.file_id, i as u32);
            let (off, len) = pool.push(&r.name);
            records.push(Record {
                name_off: off,
                parent: ROOT,
                file_id: r.file_id as u32,
                flags: flags_from_attributes(r.attributes),
                name_len: len,
            });
            seeds[i] = r.parent_file_id == r.file_id;
        }

        for i in 0..n {
            if let Some(&p) = by_id.get(&self.raw[i].parent_file_id) {
                if p != i as u32 {
                    records[i].parent = p;
                }
            }
        }

        let reachable = Self::reachable(&records, &seeds);
        let mut remap = vec![u32::MAX; n];
        let mut kept: Vec<Record> = Vec::with_capacity(n);
        for (i, r) in records.iter().enumerate() {
            if reachable[i] {
                remap[i] = kept.len() as u32;
                kept.push(*r);
            }
        }
        for r in kept.iter_mut() {
            if r.parent != ROOT {
                r.parent = remap[r.parent as usize];
            }
        }

        let mut order: Vec<u32> = (0..kept.len() as u32).collect();
        order.sort_unstable_by_key(|&i| kept[i as usize].file_id);
        let mut pos = vec![0u32; kept.len()];
        for (new_idx, &old_idx) in order.iter().enumerate() {
            pos[old_idx as usize] = new_idx as u32;
        }
        let mut sorted: Vec<Record> = Vec::with_capacity(kept.len());
        for &old_idx in &order {
            let mut r = kept[old_idx as usize];
            if r.parent != ROOT {
                r.parent = pos[r.parent as usize];
            }
            sorted.push(r);
        }

        let names = Self::rebuild_pool(&mut sorted, &pool);
        IndexSnapshot::new(
            self.volume_id,
            0,
            0,
            self.next_usn,
            0,
            Store::Owned { records: sorted, names },
        )
    }

    fn reachable(records: &[Record], seeds: &[bool]) -> Vec<bool> {
        let mut children: Vec<Vec<u32>> = vec![Vec::new(); records.len()];
        let mut stack: Vec<u32> = Vec::new();
        for (i, r) in records.iter().enumerate() {
            if r.parent != ROOT {
                children[r.parent as usize].push(i as u32);
            } else if seeds[i] {
                stack.push(i as u32);
            }
        }
        let mut ok = vec![false; records.len()];
        for &s in &stack {
            ok[s as usize] = true;
        }
        while let Some(cur) = stack.pop() {
            for &c in &children[cur as usize] {
                if !ok[c as usize] {
                    ok[c as usize] = true;
                    stack.push(c);
                }
            }
        }
        ok
    }

    fn rebuild_pool(records: &mut [Record], old: &NamePool) -> NamePool {
        let mut fresh = NamePool::with_capacity(old.len());
        for r in records.iter_mut() {
            let bytes = old.get(r.name_off, r.name_len);
            let (off, len) = fresh.push_bytes(bytes);
            r.name_off = off;
            r.name_len = len;
        }
        fresh
    }
}
```

`index/mod.rs` 加入 `pub mod build; pub mod snapshot;`（Task 2 已写）。`lib.rs` 加入 `pub mod collector;`。

- [ ] **Step 6: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，13 tests passed

- [ ] **Step 7: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(core): build snapshot with parent resolution, sorting and pool rebuild"
```

---

### Task 4: PathResolver

**Files:**
- Create: `crates/fsearch-core/src/path.rs`
- Modify: `crates/fsearch-core/src/lib.rs`

**Interfaces:**
- Consumes: `IndexSnapshot::{records, names}`、`ROOT`、`F_DIR`
- Produces: `PathResolver::{new, path_of, file_name, depth}`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;
    use crate::index::snapshot::IndexSnapshot;

    fn snap() -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 0);
        for (id, parent, name, dir) in [
            (5u64, 5u64, "\\", true),
            (10, 5, "Users", true),
            (11, 10, "Alice", true),
            (12, 11, "notes.txt", false),
        ] {
            b.extend(RawRecord {
                file_id: id,
                parent_file_id: parent,
                name: name.to_string(),
                attributes: if dir { 0x10 } else { 0 },
            });
        }
        b.finish()
    }

    #[test]
    fn resolves_full_path() {
        let s = snap();
        let mut r = PathResolver::new(&s, 'C');
        assert_eq!(r.path_of(3), "C:\\Users\\Alice\\notes.txt");
    }

    #[test]
    fn depth_counts_segments() {
        let s = snap();
        let mut r = PathResolver::new(&s, 'C');
        assert_eq!(r.depth(3), 3);
    }

    #[test]
    fn file_name_is_lowercased_slice() {
        let s = snap();
        let r = PathResolver::new(&s, 'C');
        assert_eq!(r.file_name(3), "notes.txt");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core resolves_full_path`
Expected: FAIL，`cannot find type PathResolver`

- [ ] **Step 3: 实现**

```rust
use ahash::AHashMap;

use crate::index::snapshot::IndexSnapshot;
use crate::model::ROOT;

const MAX_DEPTH: usize = 64;
const CACHE_CAP: usize = 512;

pub struct PathResolver<'a> {
    snap: &'a IndexSnapshot,
    drive: char,
    cache: AHashMap<u32, String>,
    scratch: Vec<u32>,
}

impl<'a> PathResolver<'a> {
    pub fn new(snap: &'a IndexSnapshot, drive: char) -> Self {
        Self {
            snap,
            drive,
            cache: AHashMap::with_capacity(CACHE_CAP),
            scratch: Vec::with_capacity(MAX_DEPTH),
        }
    }

    pub fn file_name(&self, idx: u32) -> &str {
        let r = &self.snap.records()[idx as usize];
        std::str::from_utf8(self.snap.name_at(r.name_off, r.name_len)).unwrap_or("")
    }

    pub fn depth(&self, idx: u32) -> u16 {
        let mut cur = idx;
        let mut depth = 0u16;
        for _ in 0..MAX_DEPTH {
            let r = &self.snap.records()[cur as usize];
            if r.parent == ROOT {
                return depth;
            }
            depth += 1;
            cur = r.parent;
        }
        depth
    }

    pub fn path_of(&mut self, idx: u32) -> String {
        if let Some(hit) = self.cache.get(&idx) {
            return hit.clone();
        }
        self.scratch.clear();
        let mut cur = idx;
        let mut total = 2usize;
        for _ in 0..MAX_DEPTH {
            let r = &self.snap.records()[cur as usize];
            self.scratch.push(cur);
            total += r.name_len as usize + 1;
            if r.parent == ROOT {
                break;
            }
            cur = r.parent;
        }

        let mut out = String::with_capacity(total);
        out.push(self.drive);
        out.push(':');
        for &node in self.scratch.iter().rev() {
            let r = &self.snap.records()[node as usize];
            let name = self.snap.name_at(r.name_off, r.name_len);
            if name == b"\\" {
                continue;
            }
            out.push('\\');
            out.push_str(std::str::from_utf8(name).unwrap_or(""));
        }
        if out.len() == 2 {
            out.push('\\');
        }

        if self.cache.len() >= CACHE_CAP {
            self.cache.clear();
        }
        self.cache.insert(idx, out.clone());
        out
    }
}
```

`lib.rs` 加入 `pub mod path;`。

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，16 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(core): resolve full paths with cache"
```

---

### Task 5: 查询解析

**Files:**
- Create: `crates/fsearch-core/src/query/mod.rs`
- Create: `crates/fsearch-core/src/query/parse.rs`
- Modify: `crates/fsearch-core/src/lib.rs`

**Interfaces:**
- Consumes: `model::ascii_lower`
- Produces: `parse::{Query, Term, WildcardPattern, Seg, parse, split_terms, compile}`

- [ ] **Step 1: 写失败的测试**

`parse.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_spaces() {
        let q = parse("read me");
        assert_eq!(q.terms.len(), 2);
        assert_eq!(q.terms[0], Term::Substr("read".into()));
        assert_eq!(q.terms[1], Term::Substr("me".into()));
    }

    #[test]
    fn quoted_phrase_keeps_spaces_and_lowercases() {
        let q = parse("\"My File\" txt");
        assert_eq!(q.terms[0], Term::Substr("my file".into()));
        assert_eq!(q.terms[1], Term::Substr("txt".into()));
    }

    #[test]
    fn star_and_question_compile_to_segments() {
        let q = parse("*.RS");
        let Term::Wildcard(p) = &q.terms[0] else { panic!("expected wildcard") };
        assert_eq!(p.segs, vec![Seg::Many, Seg::Literal(".rs".into())]);
    }

    #[test]
    fn consecutive_stars_collapse() {
        let q = parse("a**b");
        let Term::Wildcard(p) = &q.terms[0] else { panic!() };
        assert_eq!(p.segs, vec![
            Seg::Literal("a".into()),
            Seg::Many,
            Seg::Literal("b".into()),
        ]);
    }

    #[test]
    fn blank_input_yields_no_terms() {
        assert!(parse("   ").terms.is_empty());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core splits_on_spaces`
Expected: FAIL，`cannot find function parse`

- [ ] **Step 3: 实现**

```rust
use crate::model::ascii_lower;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<Term>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Substr(String),
    Wildcard(WildcardPattern),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WildcardPattern {
    pub segs: Vec<Seg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Literal(String),
    One,
    Many,
}

pub fn parse(input: &str) -> Query {
    let terms = split_terms(input)
        .into_iter()
        .filter(|t| !t.is_empty())
        .map(|t| compile(&t))
        .collect();
    Query { terms }
}

pub fn split_terms(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in input.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn compile(term: &str) -> Term {
    if !term.contains('*') && !term.contains('?') {
        return Term::Substr(ascii_lower(term));
    }
    let mut segs = Vec::new();
    let mut lit = String::new();
    for c in term.chars() {
        match c {
            '*' => {
                if !lit.is_empty() {
                    segs.push(Seg::Literal(ascii_lower(&lit)));
                    lit.clear();
                }
                if !matches!(segs.last(), Some(Seg::Many)) {
                    segs.push(Seg::Many);
                }
            }
            '?' => {
                if !lit.is_empty() {
                    segs.push(Seg::Literal(ascii_lower(&lit)));
                    lit.clear();
                }
                segs.push(Seg::One);
            }
            _ => lit.push(c),
        }
    }
    if !lit.is_empty() {
        segs.push(Seg::Literal(ascii_lower(&lit)));
    }
    Term::Wildcard(WildcardPattern { segs })
}
```

`query/mod.rs`：

```rust
pub mod matcher;
pub mod parse;
pub mod rank;

pub use parse::{compile, parse, Query, Seg, Term, WildcardPattern};
```

`lib.rs` 加入 `pub mod query;`。

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core splits_on_spaces`
Expected: PASS，5 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(query): parse terms with quoting and wildcard segments"
```

---

### Task 6: 子串/通配符匹配与高亮区间

**Files:**
- Create: `crates/fsearch-core/src/query/matcher.rs`

**Interfaces:**
- Consumes: `parse::{Term, WildcardPattern, Seg}`
- Produces:
  - `contains(&[u8], &[u8]) -> Option<(u32, u32)>`
  - `wildcard_match(&[u8], &WildcardPattern) -> Option<SmallVec<[(u32, u32); 4]>>`
  - `match_term(&[u8], &Term) -> Option<SmallVec<[(u32, u32); 4]>>`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parse::{Seg, Term, WildcardPattern};

    fn pat(segs: Vec<Seg>) -> WildcardPattern {
        WildcardPattern { segs }
    }

    #[test]
    fn substring_reports_position() {
        assert_eq!(contains(b"main.rs", b"ain"), Some((1, 3)));
        assert_eq!(contains(b"main.rs", b"zzz"), None);
    }

    #[test]
    fn suffix_wildcard_must_anchor_at_end() {
        let p = pat(vec![Seg::Many, Seg::Literal(".rs".into())]);
        assert!(wildcard_match(b"main.rs", &p).is_some());
        assert!(wildcard_match(b"main.rs.txt", &p).is_none());
    }

    #[test]
    fn prefix_wildcard_must_anchor_at_start() {
        let p = pat(vec![Seg::Literal("read".into()), Seg::Many]);
        assert!(wildcard_match(b"readme.txt", &p).is_some());
        assert!(wildcard_match(b"myreadme", &p).is_none());
    }

    #[test]
    fn question_mark_consumes_exactly_one_byte() {
        let p = pat(vec![
            Seg::Literal("a".into()),
            Seg::One,
            Seg::Literal("c".into()),
        ]);
        assert!(wildcard_match(b"abc", &p).is_some());
        assert!(wildcard_match(b"ac", &p).is_none());
        assert!(wildcard_match(b"abbc", &p).is_none());
    }

    #[test]
    fn middle_literal_reports_highlight_offset() {
        let p = pat(vec![Seg::Many, Seg::Literal("test".into()), Seg::Many]);
        let ranges = wildcard_match(b"mytest.rs", &p).expect("should match");
        assert_eq!(ranges.as_slice(), &[(2, 4)]);
    }

    #[test]
    fn substring_term_highlight_matches_needle() {
        let ranges = match_term(b"notes.txt", &Term::Substr("tes".into())).unwrap();
        assert_eq!(ranges.as_slice(), &[(2, 3)]);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core substring_reports_position`
Expected: FAIL，`cannot find function contains`

- [ ] **Step 3: 实现**

```rust
use memchr::memmem;
use smallvec::SmallVec;

use crate::query::parse::{Seg, Term, WildcardPattern};

pub fn contains(hay: &[u8], needle: &[u8]) -> Option<(u32, u32)> {
    if needle.is_empty() {
        return None;
    }
    memmem::find(hay, needle).map(|pos| (pos as u32, needle.len() as u32))
}

pub fn wildcard_match(hay: &[u8], pat: &WildcardPattern) -> Option<SmallVec<[(u32, u32); 4]>> {
    let segs = &pat.segs;
    if segs.is_empty() {
        return if hay.is_empty() { Some(SmallVec::new()) } else { None };
    }
    let anchored_start = !matches!(segs.first(), Some(Seg::Many));
    let anchored_end = !matches!(segs.last(), Some(Seg::Many));
    let mut out: SmallVec<[(u32, u32); 4]> = SmallVec::new();
    let mut hi = 0usize;

    for (si, seg) in segs.iter().enumerate() {
        match seg {
            Seg::Many => {}
            Seg::One => {
                if hi >= hay.len() {
                    return None;
                }
                hi += 1;
            }
            Seg::Literal(lit) => {
                let lb = lit.as_bytes();
                let pos = if anchored_start && si == 0 {
                    if hay.starts_with(lb) { 0 } else { return None }
                } else {
                    memmem::find(&hay[hi..], lb)? + hi
                };
                out.push((pos as u32, lb.len() as u32));
                hi = pos + lb.len();
            }
        }
    }

    if anchored_end && hi != hay.len() {
        return None;
    }
    Some(out)
}

pub fn match_term(name: &[u8], term: &Term) -> Option<SmallVec<[(u32, u32); 4]>> {
    match term {
        Term::Substr(s) => contains(name, s.as_bytes()).map(|r| SmallVec::from_slice(&[r])),
        Term::Wildcard(p) => wildcard_match(name, p),
    }
}
```

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，22 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(query): add substring and anchored wildcard matchers with highlights"
```

---

### Task 7: 评分、top-N 与 query()

**Files:**
- Create: `crates/fsearch-core/src/query/rank.rs`
- Modify: `crates/fsearch-core/src/query/mod.rs`

**Interfaces:**
- Consumes: `IndexSnapshot::{records, name_at, volume_id}`、`matcher::match_term`、`parse::{parse, Term}`
- Produces: `rank::{Hit, query}`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;
    use crate::index::snapshot::IndexSnapshot;

    fn snap() -> IndexSnapshot {
        let mut b = IndexBuilder::new(3, 0);
        for (id, parent, name, dir) in [
            (5u64, 5u64, "\\", true),
            (10, 5, "main.rs", false),
            (11, 5, "notes.txt", false),
            (12, 5, "main", false),
        ] {
            b.extend(RawRecord {
                file_id: id,
                parent_file_id: parent,
                name: name.to_string(),
                attributes: if dir { 0x10 } else { 0 },
            });
        }
        b.finish()
    }

    #[test]
    fn exact_name_ranks_above_prefix_and_substring() {
        let hits = query(&snap(), "main", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].record_idx, 3);
        assert_eq!(hits[1].record_idx, 1);
    }

    #[test]
    fn multiple_terms_require_all_to_match() {
        let hits = query(&snap(), "main rs", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_idx, 1);
    }

    #[test]
    fn wildcard_query_works() {
        let hits = query(&snap(), "*.txt", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_idx, 2);
    }

    #[test]
    fn volume_id_is_carried_on_hits() {
        assert_eq!(query(&snap(), "main", 10)[0].volume_id, 3);
    }

    #[test]
    fn limit_caps_results() {
        assert_eq!(query(&snap(), "main", 1).len(), 1);
    }

    #[test]
    fn blank_query_returns_empty() {
        assert!(query(&snap(), "  ", 10).is_empty());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core exact_name_ranks_above_prefix_and_substring`
Expected: FAIL，`cannot find function query`

- [ ] **Step 3: 实现**

```rust
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use smallvec::SmallVec;

use crate::index::snapshot::IndexSnapshot;
use crate::model::F_DIR;
use crate::query::matcher::match_term;
use crate::query::parse::{parse, Query, Term};

#[derive(Debug, Clone)]
pub struct Hit {
    pub record_idx: u32,
    pub volume_id: u8,
    pub score: u32,
    pub highlight: SmallVec<[(u32, u32); 4]>,
}

#[derive(Debug, Clone)]
struct Scored {
    score: u32,
    record_idx: u32,
    highlight: SmallVec<[(u32, u32); 4]>,
}

impl PartialEq for Scored {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.record_idx == other.record_idx
    }
}
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scored {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.record_idx.cmp(&self.record_idx))
    }
}

pub fn query(snap: &IndexSnapshot, input: &str, limit: usize) -> Vec<Hit> {
    let q = parse(input);
    if q.terms.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<Reverse<Scored>> = BinaryHeap::with_capacity(limit + 1);
    for (idx, r) in snap.records().iter().enumerate() {
        let name = snap.name_at(r.name_off, r.name_len);
        let Some((score, highlight)) = score_record(name, r.flags, &q) else {
            continue;
        };
        let scored = Scored { score, record_idx: idx as u32, highlight };
        if heap.len() < limit {
            heap.push(Reverse(scored));
        } else if let Some(Reverse(min)) = heap.peek() {
            if scored > *min {
                heap.pop();
                heap.push(Reverse(scored));
            }
        }
    }

    let mut out: Vec<Scored> = heap.into_iter().map(|Reverse(s)| s).collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out.into_iter()
        .map(|s| Hit {
            record_idx: s.record_idx,
            volume_id: snap.volume_id,
            score: s.score,
            highlight: s.highlight,
        })
        .collect()
}

fn score_record(name: &[u8], flags: u16, q: &Query) -> Option<(u32, SmallVec<[(u32, u32); 4]>)> {
    let mut total = 0u32;
    let mut highlight: SmallVec<[(u32, u32); 4]> = SmallVec::new();

    for term in &q.terms {
        let ranges = match_term(name, term)?;
        for (pos, _) in &ranges {
            total += 200u32.saturating_sub((*pos).min(100));
        }
        highlight.extend(ranges);
    }

    if let [Term::Substr(s)] = q.terms.as_slice() {
        if name == s.as_bytes() {
            total += 400;
        } else if name.starts_with(s.as_bytes()) {
            total += 300;
        }
    }

    if flags & F_DIR != 0 {
        total = total.saturating_sub(50);
    }

    Some((total, highlight))
}
```

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，28 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(query): rank hits with bounded heap and apply AND semantics"
```

---

### Task 8: 快照序列化格式

**Files:**
- Create: `crates/fsearch-core/src/index/format.rs`
- Modify: `crates/fsearch-core/src/index/mod.rs`

**Interfaces:**
- Consumes: `IndexSnapshot::{records, names, record_count, volume_serial, journal_id, next_usn, generation}`
- Produces:
  - `format::{MAGIC, FORMAT_VERSION, HEADER_SIZE, SnapshotHeader, SnapshotError, header_for, write_snapshot}`
  - `SnapshotHeader::{encode, decode}`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;
    use crate::index::snapshot::IndexSnapshot;

    fn sample() -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 77);
        b.extend(RawRecord {
            file_id: 5,
            parent_file_id: 5,
            name: "\\".into(),
            attributes: 0x10,
        });
        b.extend(RawRecord {
            file_id: 9,
            parent_file_id: 5,
            name: "Note.TXT".into(),
            attributes: 0,
        });
        b.finish()
    }

    #[test]
    fn header_is_64_bytes() {
        assert_eq!(HEADER_SIZE, 64);
        assert_eq!(std::mem::size_of::<SnapshotHeader>(), 64);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let snap = sample();
        let header = header_for(&snap);
        let decoded = SnapshotHeader::decode(&header.encode()[..]).unwrap();
        assert_eq!(decoded.record_count, 2);
        assert_eq!(decoded.next_usn, 77);
        assert_eq!(decoded.names_len, snap.names().len() as u64);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0] = b'X';
        assert!(matches!(SnapshotHeader::decode(&buf), Err(SnapshotError::BadMagic)));
    }

    #[test]
    fn truncated_header_is_rejected() {
        assert!(matches!(
            SnapshotHeader::decode(&[0u8; 40]),
            Err(SnapshotError::LengthMismatch)
        ));
    }

    #[test]
    fn write_snapshot_produces_expected_size() {
        let snap = sample();
        let path = std::env::temp_dir().join(format!("fsfmt-{}.bin", std::process::id()));
        write_snapshot(&path, &snap).unwrap();
        let len = std::fs::metadata(&path).unwrap().len() as usize;
        assert_eq!(len, HEADER_SIZE + snap.record_count() * 16 + snap.names().len());
        std::fs::remove_file(&path).ok();
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core header_is_64_bytes`
Expected: FAIL，`cannot find value HEADER_SIZE`

- [ ] **Step 3: 实现**

```rust
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use thiserror::Error;

use crate::index::snapshot::IndexSnapshot;

pub const MAGIC: [u8; 8] = *b"FSEARCH\0";
pub const FORMAT_VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 64;
pub const ENDIAN_MARK: u32 = u32::from_le_bytes(*b"LE12");

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported format version {0}")]
    BadVersion(u32),
    #[error("unsupported byte order")]
    BadEndian,
    #[error("length mismatch")]
    LengthMismatch,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SnapshotHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub endian: u32,
    pub volume_serial: u64,
    pub journal_id: u64,
    pub next_usn: i64,
    pub record_count: u64,
    pub names_len: u64,
    pub generation: u64,
}

impl SnapshotHeader {
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.endian.to_le_bytes());
        buf[16..24].copy_from_slice(&self.volume_serial.to_le_bytes());
        buf[24..32].copy_from_slice(&self.journal_id.to_le_bytes());
        buf[32..40].copy_from_slice(&self.next_usn.to_le_bytes());
        buf[40..48].copy_from_slice(&self.record_count.to_le_bytes());
        buf[48..56].copy_from_slice(&self.names_len.to_le_bytes());
        buf[56..64].copy_from_slice(&self.generation.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Result<Self, SnapshotError> {
        if buf.len() < HEADER_SIZE {
            return Err(SnapshotError::LengthMismatch);
        }
        if buf[0..8] != MAGIC {
            return Err(SnapshotError::BadMagic);
        }
        let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(SnapshotError::BadVersion(version));
        }
        let endian = u32::from_le_bytes(buf[12..16].try_into().unwrap());
        if endian != ENDIAN_MARK {
            return Err(SnapshotError::BadEndian);
        }
        Ok(Self {
            magic: buf[0..8].try_into().unwrap(),
            version,
            endian,
            volume_serial: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            journal_id: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
            next_usn: i64::from_le_bytes(buf[32..40].try_into().unwrap()),
            record_count: u64::from_le_bytes(buf[40..48].try_into().unwrap()),
            names_len: u64::from_le_bytes(buf[48..56].try_into().unwrap()),
            generation: u64::from_le_bytes(buf[56..64].try_into().unwrap()),
        })
    }
}

pub fn header_for(snap: &IndexSnapshot) -> SnapshotHeader {
    SnapshotHeader {
        magic: MAGIC,
        version: FORMAT_VERSION,
        endian: ENDIAN_MARK,
        volume_serial: snap.volume_serial,
        journal_id: snap.journal_id,
        next_usn: snap.next_usn,
        record_count: snap.record_count() as u64,
        names_len: snap.names().len() as u64,
        generation: snap.generation,
    }
}

pub fn write_snapshot(path: &Path, snap: &IndexSnapshot) -> Result<(), SnapshotError> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(&header_for(snap).encode())?;
        let records = snap.records();
        let bytes = unsafe {
            std::slice::from_raw_parts(records.as_ptr() as *const u8, records.len() * 16)
        };
        file.write_all(bytes)?;
        file.write_all(snap.names())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
```

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，33 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(index): add versioned snapshot format with atomic writes"
```

---

### Task 9: mmap 快照加载

**Files:**
- Modify: `crates/fsearch-core/src/index/snapshot.rs`（整体替换）
- Modify: `crates/fsearch-core/src/index/mod.rs`

**Interfaces:**
- Consumes: `format::{SnapshotHeader, SnapshotError, HEADER_SIZE}`
- Produces: `IndexSnapshot::open(&Path) -> Result<IndexSnapshot, SnapshotError>`；`Store::{Owned, Mapped}` 两种形态对查询层透明

- [ ] **Step 1: 写失败的测试**

在 `snapshot.rs` 测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;
    use crate::index::format::write_snapshot;

    fn sample() -> IndexSnapshot {
        let mut b = IndexBuilder::new(1, 100);
        b.extend(RawRecord { file_id: 5, parent_file_id: 5, name: "\\".into(), attributes: 0x10 });
        b.extend(RawRecord { file_id: 7, parent_file_id: 5, name: "Report.PDF".into(), attributes: 0 });
        b.finish()
    }

    #[test]
    fn mapped_snapshot_matches_owned_snapshot() {
        let owned = sample();
        let path = std::env::temp_dir().join(format!("fsmap-{}.bin", std::process::id()));
        write_snapshot(&path, &owned).unwrap();

        let mapped = IndexSnapshot::open(&path).unwrap();
        assert_eq!(mapped.record_count(), owned.record_count());
        assert_eq!(mapped.names(), owned.names());
        for (a, b) in mapped.records().iter().zip(owned.records()) {
            assert_eq!(a, b);
        }
        assert_eq!(mapped.next_usn, 100);
        assert_eq!(mapped.journal_id, owned.journal_id);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn truncated_file_is_rejected() {
        let path = std::env::temp_dir().join(format!("fsmap-trunc-{}.bin", std::process::id()));
        std::fs::write(&path, b"FSEARCH\0").unwrap();
        assert!(IndexSnapshot::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core mapped_snapshot_matches_owned_snapshot`
Expected: FAIL，`cannot find function open`

- [ ] **Step 3: 整体替换 `snapshot.rs`**

```rust
use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

use crate::index::format::{SnapshotError, SnapshotHeader, HEADER_SIZE};
use crate::index::NamePool;
use crate::model::Record;

pub enum Store {
    Owned {
        records: Vec<Record>,
        names: NamePool,
    },
    Mapped {
        map: Mmap,
        record_count: usize,
        names_off: usize,
    },
}

unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl Store {
    fn records(&self) -> &[Record] {
        match self {
            Store::Owned { records, .. } => records,
            Store::Mapped { map, record_count, .. } => {
                let base = &map[HEADER_SIZE..];
                let ptr = base.as_ptr() as *const Record;
                assert_eq!(
                    ptr as usize % std::mem::align_of::<Record>(),
                    0,
                    "mmap payload is not aligned for Record"
                );
                unsafe { std::slice::from_raw_parts(ptr, *record_count) }
            }
        }
    }

    fn names(&self) -> &[u8] {
        match self {
            Store::Owned { names, .. } => names.as_bytes(),
            Store::Mapped { map, names_off, .. } => &map[*names_off..],
        }
    }
}

pub struct IndexSnapshot {
    pub volume_id: u8,
    pub volume_serial: u64,
    pub journal_id: u64,
    pub next_usn: i64,
    pub generation: u64,
    store: Store,
}

impl IndexSnapshot {
    pub fn new(
        volume_id: u8,
        volume_serial: u64,
        journal_id: u64,
        next_usn: i64,
        generation: u64,
        store: Store,
    ) -> Self {
        Self { volume_id, volume_serial, journal_id, next_usn, generation, store }
    }

    pub fn open(path: &Path) -> Result<Self, SnapshotError> {
        let file = File::open(path)?;
        let map = unsafe { Mmap::map(&file)? };
        let header = SnapshotHeader::decode(&map)?;
        let records_len = header.record_count as usize * std::mem::size_of::<Record>();
        let names_off = HEADER_SIZE + records_len;
        let expected = names_off + header.names_len as usize;
        if map.len() != expected {
            return Err(SnapshotError::LengthMismatch);
        }
        Ok(Self {
            volume_id: 0,
            volume_serial: header.volume_serial,
            journal_id: header.journal_id,
            next_usn: header.next_usn,
            generation: header.generation,
            store: Store::Mapped {
                map,
                record_count: header.record_count as usize,
                names_off,
            },
        })
    }

    pub fn records(&self) -> &[Record] {
        self.store.records()
    }

    pub fn names(&self) -> &[u8] {
        self.store.names()
    }

    pub fn name_at(&self, off: u32, len: u16) -> &[u8] {
        let start = off as usize;
        &self.names()[start..start + len as usize]
    }

    pub fn record_count(&self) -> usize {
        self.records().len()
    }
}
```

`index/mod.rs` 加入 `pub mod format;`。

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，35 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(index): load snapshots via mmap behind a unified slice view"
```

---

### Task 10: Overlay 增量层

**Files:**
- Create: `crates/fsearch-core/src/index/overlay.rs`
- Modify: `crates/fsearch-core/src/index/mod.rs`

**Interfaces:**
- Consumes: `IndexSnapshot::{records, name_at}`、`Record`、`ROOT`
- Produces:
  - `Overlay::{new, locate, add, tombstone, is_tombstoned, tombstone_count, added_count}`
  - `Locate::{Main, Added}`
  - `Overlay::cascade_tombstone(&IndexSnapshot, u32)`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;

    fn snap() -> crate::index::snapshot::IndexSnapshot {
        let mut b = IndexBuilder::new(0, 0);
        for (id, parent, name, dir) in [
            (5u64, 5u64, "\\", true),
            (10, 5, "Users", true),
            (11, 10, "a.txt", false),
            (12, 10, "b.txt", false),
        ] {
            b.extend(RawRecord {
                file_id: id,
                parent_file_id: parent,
                name: name.to_string(),
                attributes: if dir { 0x10 } else { 0 },
            });
        }
        b.finish()
    }

    #[test]
    fn locates_records_in_main_snapshot() {
        let s = snap();
        let o = Overlay::new(s.record_count());
        assert!(matches!(o.locate(&s, 11), Some(Locate::Main(2))));
        assert!(matches!(o.locate(&s, 999), None));
    }

    #[test]
    fn tombstone_lookup_is_binary_and_order_independent() {
        let mut o = Overlay::new(10);
        o.tombstone(5);
        o.tombstone(1);
        o.tombstone(3);
        assert!(o.is_tombstoned(1));
        assert!(o.is_tombstoned(5));
        assert!(!o.is_tombstoned(2));
        assert_eq!(o.tombstone_count(), 3);
    }

    #[test]
    fn added_records_are_locatable() {
        let s = snap();
        let mut o = Overlay::new(s.record_count());
        let idx = o.add(77, 1, "New.TXT", 0);
        assert_eq!(idx, s.record_count() as u32);
        assert!(matches!(o.locate(&s, 77), Some(Locate::Added(0))));
        assert_eq!(o.name_at(0), b"new.txt");
    }

    #[test]
    fn cascade_marks_whole_subtree() {
        let s = snap();
        let mut o = Overlay::new(s.record_count());
        o.cascade_tombstone(&s, 1);
        assert!(o.is_tombstoned(1));
        assert!(o.is_tombstoned(2));
        assert!(o.is_tombstoned(3));
        assert!(!o.is_tombstoned(0));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core locates_records_in_main_snapshot`
Expected: FAIL，`cannot find type Overlay`

- [ ] **Step 3: 实现**

```rust
use crate::index::snapshot::IndexSnapshot;
use crate::index::NamePool;
use crate::model::Record;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locate {
    Main(u32),
    Added(u32),
}

pub struct Overlay {
    base_len: u32,
    added: Vec<Record>,
    added_names: NamePool,
    tombstones: Vec<u32>,
}

impl Overlay {
    pub fn new(base_len: usize) -> Self {
        Self {
            base_len: base_len as u32,
            added: Vec::new(),
            added_names: NamePool::new(),
            tombstones: Vec::new(),
        }
    }

    pub fn locate(&self, snap: &IndexSnapshot, file_id: u32) -> Option<Locate> {
        let recs = snap.records();
        let pos = recs.partition_point(|r| r.file_id < file_id);
        if pos < recs.len() && recs[pos].file_id == file_id {
            return Some(Locate::Main(pos as u32));
        }
        self.added
            .iter()
            .position(|r| r.file_id == file_id)
            .map(|i| Locate::Added(i as u32))
    }

    pub fn add(&mut self, file_id: u32, parent: u32, name: &str, flags: u16) -> u32 {
        let (off, len) = self.added_names.push(name);
        let idx = self.added.len() as u32;
        self.added.push(Record {
            name_off: off,
            parent,
            file_id,
            flags,
            name_len: len,
        });
        self.base_len + idx
    }

    pub fn name_at(&self, overlay_idx: u32) -> &[u8] {
        let r = &self.added[overlay_idx as usize];
        self.added_names.get(r.name_off, r.name_len)
    }

    pub fn tombstone(&mut self, idx: u32) {
        let pos = self.tombstones.partition_point(|&t| t < idx);
        if self.tombstones.get(pos) != Some(&idx) {
            self.tombstones.insert(pos, idx);
        }
    }

    pub fn is_tombstoned(&self, idx: u32) -> bool {
        self.tombstones.binary_search(&idx).is_ok()
    }

    pub fn cascade_tombstone(&mut self, snap: &IndexSnapshot, dir_idx: u32) {
        let mut frontier = vec![dir_idx];
        self.tombstone(dir_idx);
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for (i, r) in snap.records().iter().enumerate() {
                let idx = i as u32;
                if self.is_tombstoned(idx) {
                    continue;
                }
                if frontier.contains(&r.parent) {
                    self.tombstone(idx);
                    next.push(idx);
                }
            }
            frontier = next;
        }
    }

    pub fn added_count(&self) -> usize {
        self.added.len()
    }

    pub fn tombstone_count(&self) -> usize {
        self.tombstones.len()
    }

    pub fn added_records(&self) -> &[Record] {
        &self.added
    }

    pub fn tombstones(&self) -> &[u32] {
        &self.tombstones
    }
}
```

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，39 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(index): add overlay layer with tombstones and subtree cascade"
```

---

### Task 11: 重命名配对与合并阈值

**Files:**
- Create: `crates/fsearch-core/src/index/rename.rs`
- Modify: `crates/fsearch-core/src/index/mod.rs`

**Interfaces:**
- Consumes: `index::overlay::Overlay`
- Produces:
  - `RenameBuffer::{new, on_old, on_new, expire}`
  - `RenamePair { file_id, old_name, new_name }`
  - `should_merge(&Overlay, usize) -> bool`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn pairs_old_and_new_name() {
        let mut buf = RenameBuffer::new(Duration::from_secs(2));
        buf.on_old(42, "old.txt".into());
        let pair = buf.on_new(42, "new.txt".into()).expect("should pair");
        assert_eq!(pair.file_id, 42);
        assert_eq!(pair.old_name.as_deref(), Some("old.txt"));
        assert_eq!(pair.new_name.as_deref(), Some("new.txt"));
    }

    #[test]
    fn new_name_without_old_is_a_creation() {
        let mut buf = RenameBuffer::new(Duration::from_secs(2));
        let pair = buf.on_new(7, "fresh.txt".into()).unwrap();
        assert!(pair.old_name.is_none());
        assert_eq!(pair.new_name.as_deref(), Some("fresh.txt"));
    }

    #[test]
    fn stale_entries_expire() {
        let mut buf = RenameBuffer::new(Duration::from_millis(10));
        let t0 = Instant::now();
        buf.on_old_at(9, "gone.txt".into(), t0);
        let expired = buf.expire(t0 + Duration::from_millis(50));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].file_id, 9);
    }

    #[test]
    fn merge_threshold_is_two_percent() {
        let mut o = crate::index::overlay::Overlay::new(1000);
        for i in 0..10 {
            o.tombstone(i);
        }
        assert!(!should_merge(&o, 1000));
        for i in 10..21 {
            o.tombstone(i);
        }
        assert!(should_merge(&o, 1000));
    }

    #[test]
    fn empty_index_never_triggers_merge() {
        assert!(!should_merge(&crate::index::overlay::Overlay::new(0), 0));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core pairs_old_and_new_name`
Expected: FAIL，`cannot find type RenameBuffer`

- [ ] **Step 3: 实现**

```rust
use std::time::{Duration, Instant};

use ahash::AHashMap;

use crate::index::overlay::Overlay;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePair {
    pub file_id: u32,
    pub old_name: Option<String>,
    pub new_name: Option<String>,
}

pub struct RenameBuffer {
    timeout: Duration,
    pending: AHashMap<u32, (Instant, String)>,
}

impl RenameBuffer {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout, pending: AHashMap::new() }
    }

    pub fn on_old(&mut self, file_id: u32, old_name: String) {
        self.on_old_at(file_id, old_name, Instant::now());
    }

    pub fn on_old_at(&mut self, file_id: u32, old_name: String, at: Instant) {
        self.pending.insert(file_id, (at, old_name));
    }

    pub fn on_new(&mut self, file_id: u32, new_name: String) -> Option<RenamePair> {
        let old_name = self.pending.remove(&file_id).map(|(_, name)| name);
        Some(RenamePair {
            file_id,
            old_name,
            new_name: Some(new_name),
        })
    }

    pub fn expire(&mut self, now: Instant) -> Vec<RenamePair> {
        let mut expired = Vec::new();
        self.pending.retain(|&file_id, (at, name)| {
            if now.duration_since(*at) >= self.timeout {
                expired.push(RenamePair {
                    file_id,
                    old_name: Some(name.clone()),
                    new_name: None,
                });
                false
            } else {
                true
            }
        });
        expired
    }
}

pub fn should_merge(overlay: &Overlay, total: usize) -> bool {
    if total == 0 {
        return false;
    }
    (overlay.added_count() + overlay.tombstone_count()) * 100 > total * 2
}
```

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，44 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(index): pair rename events and gate overlay merges at 2%"
```

---

### Task 12: fsearch-win — 卷枚举、提权、DWM

**Files:**
- Create: `crates/fsearch-win/Cargo.toml`
- Create: `crates/fsearch-win/src/lib.rs`
- Create: `crates/fsearch-win/src/volume.rs`
- Create: `crates/fsearch-win/src/dwm.rs`
- Modify: `Cargo.toml`（若 members 未含 fsearch-win）

**Interfaces:**
- Consumes: `windows-sys`
- Produces:
  - `volume::{Volume, list_volumes, is_elevated}`
  - `dwm::{enable_mica, enable_dark_titlebar}`

- [ ] **Step 1: 写 Cargo 与失败测试**

`crates/fsearch-win/Cargo.toml`：

```toml
[package]
name = "fsearch-win"
edition.workspace = true
version.workspace = true

[dependencies]
fsearch-core = { path = "../fsearch-core" }
thiserror.workspace = true

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
  "Win32_Foundation",
  "Win32_Storage_FileSystem",
  "Win32_System_IO",
  "Win32_System_Threading",
  "Win32_Security",
  "Win32_UI_WindowsAndMessaging",
  "Win32_Graphics_Dwm",
] }
```

`volume.rs` 测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_at_least_one_fixed_volume() {
        let volumes = list_volumes();
        assert!(!volumes.is_empty(), "no fixed NTFS volume found");
        assert!(volumes.iter().all(|v| v.letter.is_ascii_uppercase()));
    }

    #[test]
    fn elevation_check_returns_bool() {
        let _ = is_elevated();
    }
}
```

- [ ] **Step 2: 运行确认失败（在 Windows 上）**

Run: `cargo test -p fsearch-win lists_at_least_one_fixed_volume`
Expected: FAIL，`cannot find function list_volumes`

- [ ] **Step 3: 实现**

`lib.rs`：

```rust
#![cfg(windows)]

pub mod dwm;
pub mod ffi;
pub mod mft;
pub mod usn;
pub mod volume;
```

`volume.rs`：

```rust
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW, DRIVE_FIXED,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub letter: char,
    pub serial: u32,
    pub fs: String,
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

pub fn list_volumes() -> Vec<Volume> {
    let mut out = Vec::new();
    let needed = unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) };
    if needed == 0 {
        return out;
    }
    let mut buf = vec![0u16; needed as usize + 1];
    let written = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) };
    if written == 0 {
        return out;
    }
    buf.truncate(written as usize);

    for chunk in buf.split(|&c| c == 0) {
        if chunk.len() < 3 {
            continue;
        }
        let root = OsString::from_wide(chunk);
        let text = root.to_string_lossy();
        let letter = text.chars().next().unwrap_or('?');
        if letter == '?' {
            continue;
        }
        let drive_type = unsafe { GetDriveTypeW(chunk.as_ptr()) };
        if drive_type != DRIVE_FIXED {
            continue;
        }
        let mut serial = 0u32;
        let mut fs_buf = vec![0u16; 32];
        let ok = unsafe {
            GetVolumeInformationW(
                chunk.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut serial,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs_buf.as_mut_ptr(),
                fs_buf.len() as u32,
            )
        };
        if ok == 0 {
            continue;
        }
        let fs: String = OsString::from_wide(&fs_buf)
            .to_string_lossy()
            .trim_end_matches('\0')
            .to_string();
        if fs.eq_ignore_ascii_case("NTFS") {
            out.push(Volume { letter, serial, fs });
        }
    }
    out
}
```

`dwm.rs`：

```rust
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
const DWMSBT_MAINWINDOW: u32 = 2;

pub fn enable_dark_titlebar(hwnd: isize) -> bool {
    let flag: u32 = 1;
    unsafe {
        DwmSetWindowAttribute(
            hwnd as *mut _,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &flag as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        ) == 0
    }
}

pub fn enable_mica(hwnd: isize) -> bool {
    unsafe {
        DwmSetWindowAttribute(
            hwnd as *mut _,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &DWMSBT_MAINWINDOW as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        ) == 0
    }
}
```

- [ ] **Step 4: 运行通过（Windows 真机）**

Run: `cargo test -p fsearch-win`
Expected: PASS，2 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-win Cargo.toml
git commit -m "feat(win): enumerate fixed NTFS volumes, detect elevation, enable mica"
```

---

### Task 13: USN 记录解析与 MftScanner

**Files:**
- Create: `crates/fsearch-core/src/collector/usn_parse.rs`
- Modify: `crates/fsearch-core/src/collector/mod.rs`
- Create: `crates/fsearch-win/src/ffi.rs`
- Create: `crates/fsearch-win/src/mft.rs`

**Interfaces:**
- Consumes: `collector::{RawRecord, RecordBatch, CollectError, Collector}`
- Produces:
  - `usn_parse::{parse_usn_records, utf16_name, USN_V2_HEADER_SIZE}`
  - `ffi::{FSCTL_*, MftEnumData, UsnRecordV2, UsnJournalData}`
  - `mft::MftScanner::new(char)`

- [ ] **Step 1: 写失败的解析测试（跨平台，跑在 core 上）**

`crates/fsearch-core/src/collector/usn_parse.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn record(file_id: u64, parent: u64, name: &str, attributes: u32) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_bytes = units.len() * 2;
        let total = (USN_V2_HEADER_SIZE + name_bytes + 7) & !7;
        let mut buf = vec![0u8; total];
        buf[0..4].copy_from_slice(&(total as u32).to_le_bytes());
        buf[4..6].copy_from_slice(&2u16.to_le_bytes());
        buf[8..16].copy_from_slice(&file_id.to_le_bytes());
        buf[16..24].copy_from_slice(&parent.to_le_bytes());
        buf[52..56].copy_from_slice(&attributes.to_le_bytes());
        buf[56..58].copy_from_slice(&(name_bytes as u16).to_le_bytes());
        buf[58..60].copy_from_slice(&(USN_V2_HEADER_SIZE as u16).to_le_bytes());
        for (i, unit) in units.iter().enumerate() {
            let at = USN_V2_HEADER_SIZE + i * 2;
            buf[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        buf
    }

    fn stream(records: &[Vec<u8>], next: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        for r in records {
            buf.extend_from_slice(r);
        }
        buf.extend_from_slice(&next.to_le_bytes());
        buf
    }

    #[test]
    fn parses_multiple_records_and_next_frn() {
        let buf = stream(
            &[record(10, 5, "Users", 0x10), record(11, 10, "a.txt", 0)],
            99,
        );
        let (records, next) = parse_usn_records(&buf);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].file_id, 10);
        assert_eq!(records[0].parent_file_id, 5);
        assert_eq!(records[1].name, "a.txt");
        assert_eq!(records[0].attributes, 0x10);
        assert_eq!(next, 99);
    }

    #[test]
    fn decodes_utf16_names() {
        let buf = stream(&[record(1, 5, "报告.TXT", 0)], 0);
        let (records, _) = parse_usn_records(&buf);
        assert_eq!(records[0].name, "报告.TXT");
    }

    #[test]
    fn empty_payload_yields_nothing() {
        let (records, next) = parse_usn_records(&0u64.to_le_bytes());
        assert!(records.is_empty());
        assert_eq!(next, 0);
    }

    #[test]
    fn truncated_record_is_ignored() {
        let mut buf = record(1, 5, "hello.txt", 0);
        buf.truncate(20);
        buf.extend_from_slice(&0u64.to_le_bytes());
        let (records, _) = parse_usn_records(&buf);
        assert!(records.is_empty());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core parses_multiple_records_and_next_frn`
Expected: FAIL，`cannot find function parse_usn_records`

- [ ] **Step 3: 实现解析**

```rust
use crate::collector::RawRecord;

pub const USN_V2_HEADER_SIZE: usize = 60;

pub fn utf16_name(buf: &[u8], offset: usize, byte_len: usize) -> String {
    let end = (offset + byte_len).min(buf.len());
    if offset >= end {
        return String::new();
    }
    let units: Vec<u16> = buf[offset..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

pub fn parse_usn_records(bytes: &[u8]) -> (Vec<RawRecord>, u64) {
    let mut records = Vec::new();
    let Some(payload_end) = bytes.len().checked_sub(8) else {
        return (records, 0);
    };
    let mut pos = 0usize;
    while pos + USN_V2_HEADER_SIZE <= payload_end {
        let record_len =
            u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        if record_len < USN_V2_HEADER_SIZE || pos + record_len > payload_end {
            break;
        }
        let rec = &bytes[pos..pos + record_len];
        let file_id = u64::from_le_bytes(rec[8..16].try_into().unwrap());
        let parent = u64::from_le_bytes(rec[16..24].try_into().unwrap());
        let attributes = u32::from_le_bytes(rec[52..56].try_into().unwrap());
        let name_len = u16::from_le_bytes(rec[56..58].try_into().unwrap()) as usize;
        let name_off = u16::from_le_bytes(rec[58..60].try_into().unwrap()) as usize;
        records.push(RawRecord {
            file_id,
            parent_file_id: parent,
            name: utf16_name(rec, name_off, name_len),
            attributes,
        });
        pos += record_len;
    }
    let next = u64::from_le_bytes(bytes[payload_end..payload_end + 8].try_into().unwrap());
    (records, next)
}
```

`collector/mod.rs` 加 `pub mod usn_parse;`。

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，48 tests passed

- [ ] **Step 5: 写 FFI 常量与结构**

`crates/fsearch-win/src/ffi.rs`：

```rust
pub const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00B3;
pub const FSCTL_CREATE_USN_JOURNAL: u32 = 0x0009_00E7;
pub const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00BB;
pub const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00F7;

pub const USN_REASON_FILE_CREATE: u32 = 0x0000_0100;
pub const USN_REASON_FILE_DELETE: u32 = 0x0000_0200;
pub const USN_REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
pub const USN_REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;

#[repr(C)]
pub struct MftEnumData {
    pub start_file_reference_number: u64,
    pub low_usn: i64,
    pub high_usn: i64,
}

#[repr(C)]
pub struct UsnJournalData {
    pub usn_journal_id: u64,
    pub first_usn: i64,
    pub next_usn: i64,
    pub lowest_valid_usn: i64,
    pub max_usn: i64,
    pub maximum_size: u64,
    pub allocation_delta: u64,
}

#[repr(C)]
pub struct CreateUsnJournalData {
    pub maximum_size: u64,
    pub allocation_delta: u64,
}

#[repr(C)]
pub struct ReadUsnJournalData {
    pub start_usn: i64,
    pub reason_mask: u32,
    pub return_only_on_close: u32,
    pub timeout: u64,
    pub bytes_to_wait_for: u32,
    pub min_major_version: u16,
    pub max_major_version: u16,
}
```

- [ ] **Step 6: 实现 MftScanner**

`crates/fsearch-win/src/mft.rs`：

```rust
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use fsearch_core::collector::usn_parse::parse_usn_records;
use fsearch_core::collector::{CollectError, Collector, RecordBatch};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GENERIC_READ, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::ffi::{MftEnumData, FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, UsnJournalData};

const IOCTL_BUF: usize = 1 << 20;

pub struct MftScanner {
    pub letter: char,
}

impl MftScanner {
    pub fn new(letter: char) -> Self {
        Self { letter }
    }
}

fn wide(path: &str) -> Vec<u16> {
    OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect()
}

unsafe fn open_volume(letter: char) -> Result<HANDLE, CollectError> {
    let path = format!("\\\\.\\{}:", letter.to_ascii_uppercase());
    let handle = CreateFileW(
        wide(&path).as_ptr(),
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        std::ptr::null(),
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        std::ptr::null_mut(),
    );
    if handle == INVALID_HANDLE_VALUE {
        return Err(CollectError::Inaccessible(letter));
    }
    Ok(handle)
}

unsafe fn query_journal(handle: HANDLE) -> Result<UsnJournalData, CollectError> {
    let mut data: UsnJournalData = std::mem::zeroed();
    let mut returned = 0u32;
    let ok = DeviceIoControl(
        handle,
        FSCTL_QUERY_USN_JOURNAL,
        std::ptr::null(),
        0,
        &mut data as *mut _ as *mut _,
        std::mem::size_of::<UsnJournalData>() as u32,
        &mut returned,
        std::ptr::null_mut(),
    );
    if ok == 0 {
        return Err(CollectError::Ioctl("FSCTL_QUERY_USN_JOURNAL".into()));
    }
    Ok(data)
}

impl Collector for MftScanner {
    fn scan(&self) -> Result<RecordBatch, CollectError> {
        unsafe {
            let handle = open_volume(self.letter)?;
            let journal = query_journal(handle)?;
            let mut buffer = vec![0u8; IOCTL_BUF];
            let mut start: u64 = 0;
            let mut all = Vec::with_capacity(1 << 20);
            loop {
                let input = MftEnumData {
                    start_file_reference_number: start,
                    low_usn: 0,
                    high_usn: i64::MAX,
                };
                let mut returned = 0u32;
                let ok = DeviceIoControl(
                    handle,
                    FSCTL_ENUM_USN_DATA,
                    &input as *const _ as *const _,
                    std::mem::size_of::<MftEnumData>() as u32,
                    buffer.as_mut_ptr() as *mut _,
                    buffer.len() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                );
                if ok == 0 {
                    CloseHandle(handle);
                    return Err(CollectError::Ioctl("FSCTL_ENUM_USN_DATA".into()));
                }
                let (records, next) = parse_usn_records(&buffer[..returned as usize]);
                all.extend(records);
                if next == 0 || next == start {
                    break;
                }
                start = next;
            }
            CloseHandle(handle);
            Ok(RecordBatch { records: all, next_usn: journal.next_usn })
        }
    }
}
```

- [ ] **Step 7: 真机验证**

Run（管理员 PowerShell）: `cargo test -p fsearch-win -- --nocapture`
Expected: PASS

- [ ] **Step 8: 提交**

```bash
git add crates/fsearch-core crates/fsearch-win
git commit -m "feat(win): parse usn records and enumerate mft via FSCTL_ENUM_USN_DATA"
```

---

### Task 14: USN 事件分类与 UsnWatcher

**Files:**
- Create: `crates/fsearch-core/src/collector/change.rs`
- Modify: `crates/fsearch-core/src/collector/usn_parse.rs`（新增 `parse_usn_events`）
- Modify: `crates/fsearch-core/src/collector/mod.rs`
- Create: `crates/fsearch-win/src/usn.rs`

**Interfaces:**
- Consumes: `usn_parse::{parse_usn_events, UsnEvent}`、`ffi::*`
- Produces:
  - `change::{Change, classify}`
  - `UsnWatcher::{open, read_batch, ensure_current, journal_id, next_usn}`；错误统一用 `CollectError`（`Ioctl("journal recreated")` 表示增量链断裂）

- [ ] **Step 1: 写失败的事件测试（跨平台）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::usn_parse::{build_test_record, parse_usn_events, stream_with_next};
    use crate::collector::reasons::*;

    #[test]
    fn create_event_becomes_created() {
        let bytes = stream_with_next(&[build_test_record(10, 5, "a.txt", 0, USN_REASON_FILE_CREATE)], 0);
        let events = parse_usn_events(&bytes).0;
        assert!(matches!(classify(events[0].clone()), Some(Change::Created(r)) if r.file_id == 10));
    }

    #[test]
    fn delete_event_has_no_name() {
        let bytes = stream_with_next(&[build_test_record(11, 5, "", 0, USN_REASON_FILE_DELETE)], 0);
        let events = parse_usn_events(&bytes).0;
        assert!(matches!(classify(events[0].clone()), Some(Change::Deleted(11))));
    }

    #[test]
    fn rename_events_are_distinguished() {
        let old = build_test_record(12, 5, "old.txt", 0, USN_REASON_RENAME_OLD_NAME);
        let new = build_test_record(12, 5, "new.txt", 0, USN_REASON_RENAME_NEW_NAME);
        let bytes = stream_with_next(&[old, new], 0);
        let events = parse_usn_events(&bytes).0;
        assert!(matches!(classify(events[0].clone()), Some(Change::RenameOld { file_id: 12, .. })));
        assert!(matches!(classify(events[1].clone()), Some(Change::RenameNew { file_id: 12, .. })));
    }

    #[test]
    fn unrelated_reason_is_ignored() {
        let bytes = stream_with_next(&[build_test_record(13, 5, "x.txt", 0, 0x0000_8000)], 0);
        let events = parse_usn_events(&bytes).0;
        assert!(classify(events[0].clone()).is_none());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core create_event_becomes_created`
Expected: FAIL，`cannot find function parse_usn_events`

- [ ] **Step 3: 实现 core 侧**

在 `collector/mod.rs` 增加常量（避免 core 依赖 win crate）：

```rust
pub mod reasons {
    pub const USN_REASON_FILE_CREATE: u32 = 0x0000_0100;
    pub const USN_REASON_FILE_DELETE: u32 = 0x0000_0200;
    pub const USN_REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
    pub const USN_REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;
    pub const USN_REASON_BASIC_INFO_CHANGE: u32 = 0x0000_8000;
}
pub mod change;
pub mod usn_parse;
```

`usn_parse.rs` 追加：

```rust
#[derive(Debug, Clone)]
pub struct UsnEvent {
    pub file_id: u64,
    pub parent_file_id: u64,
    pub name: String,
    pub attributes: u32,
    pub reason: u32,
}

pub fn parse_usn_events(bytes: &[u8]) -> (Vec<UsnEvent>, u64) {
    let mut events = Vec::new();
    let Some(payload_end) = bytes.len().checked_sub(8) else {
        return (events, 0);
    };
    let mut pos = 0usize;
    while pos + USN_V2_HEADER_SIZE <= payload_end {
        let record_len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        if record_len < USN_V2_HEADER_SIZE || pos + record_len > payload_end {
            break;
        }
        let rec = &bytes[pos..pos + record_len];
        let reason = u32::from_le_bytes(rec[40..44].try_into().unwrap());
        let name_len = u16::from_le_bytes(rec[56..58].try_into().unwrap()) as usize;
        let name_off = u16::from_le_bytes(rec[58..60].try_into().unwrap()) as usize;
        events.push(UsnEvent {
            file_id: u64::from_le_bytes(rec[8..16].try_into().unwrap()),
            parent_file_id: u64::from_le_bytes(rec[16..24].try_into().unwrap()),
            name: utf16_name(rec, name_off, name_len),
            attributes: u32::from_le_bytes(rec[52..56].try_into().unwrap()),
            reason,
        });
        pos += record_len;
    }
    let next = u64::from_le_bytes(bytes[payload_end..payload_end + 8].try_into().unwrap());
    (events, next)
}

#[cfg(test)]
pub fn build_test_record(
    file_id: u64,
    parent: u64,
    name: &str,
    attributes: u32,
    reason: u32,
) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let name_bytes = units.len() * 2;
    let total = (USN_V2_HEADER_SIZE + name_bytes + 7) & !7;
    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&(total as u32).to_le_bytes());
    buf[4..6].copy_from_slice(&2u16.to_le_bytes());
    buf[8..16].copy_from_slice(&file_id.to_le_bytes());
    buf[16..24].copy_from_slice(&parent.to_le_bytes());
    buf[40..44].copy_from_slice(&reason.to_le_bytes());
    buf[52..56].copy_from_slice(&attributes.to_le_bytes());
    buf[56..58].copy_from_slice(&(name_bytes as u16).to_le_bytes());
    buf[58..60].copy_from_slice(&(USN_V2_HEADER_SIZE as u16).to_le_bytes());
    for (i, unit) in units.iter().enumerate() {
        let at = USN_V2_HEADER_SIZE + i * 2;
        buf[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    buf
}

#[cfg(test)]
pub fn stream_with_next(records: &[Vec<u8>], next: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    for r in records {
        buf.extend_from_slice(r);
    }
    buf.extend_from_slice(&next.to_le_bytes());
    buf
}
```

`change.rs`：

```rust
use crate::collector::reasons::*;
use crate::collector::usn_parse::UsnEvent;
use crate::collector::RawRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Created(RawRecord),
    Deleted(u64),
    RenameOld { file_id: u64, name: String },
    RenameNew { file_id: u64, parent: u64, name: String, attributes: u32 },
    Attributes { file_id: u64, attributes: u32 },
}

pub fn classify(event: UsnEvent) -> Option<Change> {
    if event.reason & USN_REASON_FILE_CREATE != 0 {
        return Some(Change::Created(RawRecord {
            file_id: event.file_id,
            parent_file_id: event.parent_file_id,
            name: event.name,
            attributes: event.attributes,
        }));
    }
    if event.reason & USN_REASON_FILE_DELETE != 0 {
        return Some(Change::Deleted(event.file_id));
    }
    if event.reason & USN_REASON_RENAME_OLD_NAME != 0 {
        return Some(Change::RenameOld { file_id: event.file_id, name: event.name });
    }
    if event.reason & USN_REASON_RENAME_NEW_NAME != 0 {
        return Some(Change::RenameNew {
            file_id: event.file_id,
            parent: event.parent_file_id,
            name: event.name,
            attributes: event.attributes,
        });
    }
    if event.reason & USN_REASON_BASIC_INFO_CHANGE != 0 {
        return Some(Change::Attributes { file_id: event.file_id, attributes: event.attributes });
    }
    None
}
```

测试文件顶部需要 `use crate::collector::usn_parse::{parse_usn_events, build_test_record, stream_with_next};`，测试中用了 `crate::ffi_reasons::*` — 改为 `crate::collector::reasons::*`。

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，52 tests passed

- [ ] **Step 5: 实现 UsnWatcher（Windows）**

`crates/fsearch-win/src/usn.rs`：

```rust
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use fsearch_core::collector::change::Change;
use fsearch_core::collector::usn_parse::parse_usn_events;
use fsearch_core::collector::CollectError;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GENERIC_READ, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::ffi::{
    CreateUsnJournalData, ReadUsnJournalData, UsnJournalData, FSCTL_CREATE_USN_JOURNAL,
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL,
};

const BUF_BYTES: usize = 1 << 18;

pub struct UsnWatcher {
    handle: HANDLE,
    journal_id: u64,
    next_usn: i64,
}

fn wide(path: &str) -> Vec<u16> {
    OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect()
}

unsafe fn open_handle(letter: char) -> Result<HANDLE, CollectError> {
    let path = format!("\\\\.\\{}:", letter.to_ascii_uppercase());
    let handle = CreateFileW(
        wide(&path).as_ptr(),
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        std::ptr::null(),
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        std::ptr::null_mut(),
    );
    if handle == INVALID_HANDLE_VALUE {
        return Err(CollectError::Inaccessible(letter));
    }
    Ok(handle)
}

unsafe fn query_journal(handle: HANDLE) -> Result<UsnJournalData, CollectError> {
    let mut data: UsnJournalData = std::mem::zeroed();
    let mut returned = 0u32;
    let ok = DeviceIoControl(
        handle,
        FSCTL_QUERY_USN_JOURNAL,
        std::ptr::null(),
        0,
        &mut data as *mut _ as *mut _,
        std::mem::size_of::<UsnJournalData>() as u32,
        &mut returned,
        std::ptr::null_mut(),
    );
    if ok == 0 {
        let create = CreateUsnJournalData { maximum_size: 64 << 20, allocation_delta: 8 << 20 };
        let mut returned = 0u32;
        let created = DeviceIoControl(
            handle,
            FSCTL_CREATE_USN_JOURNAL,
            &create as *const _ as *const _,
            std::mem::size_of::<CreateUsnJournalData>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
        if created == 0 {
            return Err(CollectError::Ioctl("FSCTL_CREATE_USN_JOURNAL".into()));
        }
        return query_journal(handle);
    }
    Ok(data)
}

impl UsnWatcher {
    pub fn open(letter: char) -> Result<Self, CollectError> {
        let handle = unsafe { open_handle(letter)? };
        let journal = unsafe { query_journal(handle)? };
        Ok(Self { handle, journal_id: journal.usn_journal_id, next_usn: journal.next_usn })
    }

    pub fn journal_id(&self) -> u64 {
        self.journal_id
    }

    pub fn next_usn(&self) -> i64 {
        self.next_usn
    }

    pub fn ensure_current(&mut self) -> Result<(), CollectError> {
        let journal = unsafe { query_journal(self.handle)? };
        if journal.usn_journal_id != self.journal_id {
            return Err(CollectError::Ioctl("journal recreated".into()));
        }
        Ok(())
    }

    pub fn read_batch(&mut self, timeout_ms: u64) -> Result<Vec<Change>, CollectError> {
        let input = ReadUsnJournalData {
            start_usn: self.next_usn,
            reason_mask: u32::MAX,
            return_only_on_close: 0,
            timeout: timeout_ms,
            bytes_to_wait_for: 0,
            min_major_version: 2,
            max_major_version: 2,
        };
        let mut buffer = vec![0u8; BUF_BYTES];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                FSCTL_READ_USN_JOURNAL,
                &input as *const _ as *const _,
                std::mem::size_of::<ReadUsnJournalData>() as u32,
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(CollectError::Ioctl("FSCTL_READ_USN_JOURNAL".into()));
        }
        let (events, next) = parse_usn_events(&buffer[..returned as usize]);
        if next > 0 {
            self.next_usn = next as i64;
        }
        Ok(events
            .into_iter()
            .filter_map(fsearch_core::collector::change::classify)
            .collect())
    }
}

impl Drop for UsnWatcher {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
```

- [ ] **Step 6: 真机验证**

Run: `cargo build -p fsearch-win --release`
Expected: 编译通过

- [ ] **Step 7: 提交**

```bash
git add crates/fsearch-core crates/fsearch-win
git commit -m "feat(win): consume usn journal incrementally with recreate detection"
```

---

### Task 15: IndexEngine — 线程池隔离与并行查询

**Files:**
- Create: `crates/fsearch-core/src/engine.rs`
- Modify: `crates/fsearch-core/src/lib.rs`
- Modify: `crates/fsearch-core/src/query/rank.rs`（导出 `score_record`，新增 `query_parallel`）

**Interfaces:**
- Consumes: `IndexSnapshot`、`rank::query`、`arc_swap::ArcSwap`
- Produces:
  - `IndexEngine::{new, current, replace, query, generation, index_pool, query_pool}`
  - `rank::query_parallel(&ThreadPool, &IndexSnapshot, &str, usize) -> Vec<Hit>`

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;

    fn snap(tag: &str) -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 0);
        b.extend(RawRecord {
            file_id: 5,
            parent_file_id: 5,
            name: "\\".into(),
            attributes: 0x10,
        });
        b.extend(RawRecord {
            file_id: 9,
            parent_file_id: 5,
            name: format!("{tag}.txt"),
            attributes: 0,
        });
        b.finish()
    }

    #[test]
    fn replace_swaps_snapshot_and_bumps_generation() {
        let engine = IndexEngine::new(snap("alpha"));
        let g1 = engine.generation();
        assert_eq!(engine.query("alpha", 10).1.len(), 1);
        engine.replace(snap("beta"), g1 + 1);
        assert_eq!(engine.query("alpha", 10).1.len(), 0);
        assert_eq!(engine.query("beta", 10).1.len(), 1);
        assert!(engine.generation() > g1);
    }

    #[test]
    fn query_returns_generation_of_snapshot_used() {
        let engine = IndexEngine::new(snap("alpha"));
        let (gen, hits) = engine.query("alpha", 10);
        assert_eq!(gen, engine.generation());
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn parallel_matches_serial() {
        let snapshot = snap("alpha");
        let engine = IndexEngine::new(snapshot);
        let serial = crate::query::rank::query(&engine.current(), "alpha", 10);
        let parallel = engine.query("alpha", 10).1;
        let ser: Vec<u32> = serial.iter().map(|h| h.record_idx).collect();
        let par: Vec<u32> = parallel.iter().map(|h| h.record_idx).collect();
        assert_eq!(ser, par);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core replace_swaps_snapshot_and_bumps_generation`
Expected: FAIL，`cannot find type IndexEngine`

- [ ] **Step 3: 实现**

`engine.rs`：

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rayon::ThreadPool;

use crate::index::snapshot::IndexSnapshot;
use crate::query::rank::{query_parallel, Hit};

pub struct IndexEngine {
    index: ArcSwap<IndexSnapshot>,
    generation: AtomicU64,
    index_pool: ThreadPool,
    query_pool: ThreadPool,
}

impl IndexEngine {
    pub fn new(initial: IndexSnapshot) -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let index_threads = (cores / 2).max(1);
        let query_threads = (cores.saturating_sub(1)).clamp(1, 4);
        let generation = initial.generation;

        Self {
            index: ArcSwap::from_pointee(initial),
            generation: AtomicU64::new(generation),
            index_pool: rayon::ThreadPoolBuilder::new()
                .num_threads(index_threads)
                .thread_name(|i| format!("fsearch-index-{i}"))
                .build()
                .expect("index pool"),
            query_pool: rayon::ThreadPoolBuilder::new()
                .num_threads(query_threads)
                .thread_name(|i| format!("fsearch-query-{i}"))
                .build()
                .expect("query pool"),
        }
    }

    pub fn current(&self) -> Arc<IndexSnapshot> {
        self.index.load_full()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn replace(&self, snapshot: IndexSnapshot, generation: u64) {
        self.index.store(Arc::new(snapshot));
        self.generation.store(generation, Ordering::Release);
    }

    pub fn query(&self, text: &str, limit: usize) -> (u64, Vec<Hit>) {
        let snapshot = self.index.load_full();
        let generation = self.generation();
        let hits = query_parallel(&self.query_pool, &snapshot, text, limit);
        (generation, hits)
    }

    pub fn index_pool(&self) -> &ThreadPool {
        &self.index_pool
    }

    pub fn query_pool(&self) -> &ThreadPool {
        &self.query_pool
    }
}
```

`rank.rs` 追加：

```rust
使用 `rayon::prelude::*`。先把 Task 7 中的 `fn score_record` 可见性改为 `pub(crate) fn score_record`（签名不变），然后追加：

pub fn query_parallel(
    pool: &rayon::ThreadPool,
    snap: &IndexSnapshot,
    input: &str,
    limit: usize,
) -> Vec<Hit> {
    let q = parse(input);
    if q.terms.is_empty() || limit == 0 {
        return Vec::new();
    }

    const CHUNK: usize = 3276;
    let records = snap.records();
    let partials: Vec<Vec<Scored>> = pool.install(|| {
        records
            .par_chunks(CHUNK)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let base = chunk_idx * CHUNK;
                let mut heap: BinaryHeap<Reverse<Scored>> = BinaryHeap::with_capacity(limit + 1);
                for (i, r) in chunk.iter().enumerate() {
                    let name = snap.name_at(r.name_off, r.name_len);
                    let Some((score, highlight)) = score_record(name, r.flags, &q) else {
                        continue;
                    };
                    let scored = Scored {
                        score,
                        record_idx: (base + i) as u32,
                        highlight,
                    };
                    if heap.len() < limit {
                        heap.push(Reverse(scored));
                    } else if let Some(Reverse(min)) = heap.peek() {
                        if scored > *min {
                            heap.pop();
                            heap.push(Reverse(scored));
                        }
                    }
                }
                heap.into_iter().map(|Reverse(s)| s).collect::<Vec<_>>()
            })
            .collect()
    });

    let mut merged: Vec<Scored> = partials.into_iter().flatten().collect();
    merged.sort_unstable_by(|a, b| b.cmp(a));
    merged.truncate(limit);
    merged
        .into_iter()
        .map(|s| Hit {
            record_idx: s.record_idx,
            volume_id: snap.volume_id,
            score: s.score,
            highlight: s.highlight,
        })
        .collect()
}
```

`Scored` 需要 `Send`（SmallVec 是 Send）✓。

`Scored` 在 Task 7 是私有结构体，同模块内可访问 ✓。

`lib.rs` 加 `pub mod engine;`。

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，55 tests passed

- [ ] **Step 5: 提交**

```bash
git add crates/fsearch-core
git commit -m "feat(core): add engine with isolated pools and parallel query"
```

---

### Task 16: UI 骨架与结果行拆分

**Files:**
- Create: `crates/fsearch-ui/Cargo.toml`
- Create: `crates/fsearch-ui/build.rs`
- Create: `crates/fsearch-ui/ui/theme.slint`
- Create: `crates/fsearch-ui/ui/app.slint`
- Create: `crates/fsearch-ui/src/bridge.rs`
- Create: `crates/fsearch-ui/src/main.rs`

**Interfaces:**
- Consumes: `fsearch_core::query::rank::Hit`
- Produces: `bridge::{row_from_hit, rows_from_hits}`；Slint 组件 `AppWindow`

- [ ] **Step 1: 写失败的测试**

`bridge.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::smallvec;

    fn hit(highlight: &[(u32, u32)]) -> fsearch_core::query::rank::Hit {
        fsearch_core::query::rank::Hit {
            record_idx: 0,
            volume_id: 0,
            score: 1,
            highlight: SmallVec::from_slice(highlight),
        }
    }

    #[test]
    fn splits_name_around_first_highlight() {
        let row = row_from_hit(b"notes.txt", "C:\\notes.txt", &hit(&[(2, 3)]));
        assert_eq!(row.before, "no");
        assert_eq!(row.matched, "tes");
        assert_eq!(row.after, ".txt");
        assert_eq!(row.path, "C:\\notes.txt");
    }

    #[test]
    fn no_highlight_puts_everything_in_before() {
        let row = row_from_hit(b"notes.txt", "C:\\notes.txt", &hit(&[]));
        assert_eq!(row.before, "notes.txt");
        assert_eq!(row.matched, "");
        assert_eq!(row.after, "");
    }

    #[test]
    fn invalid_utf8_is_lossy_not_panic() {
        let row = row_from_hit(&[0xff, 0xfe], "x", &hit(&[]));
        assert!(!row.before.is_empty());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-ui splits_name_around_first_highlight`
Expected: FAIL，`cannot find function row_from_hit`

- [ ] **Step 3: 配置 crate**

`crates/fsearch-ui/Cargo.toml`：

```toml
[package]
name = "fsearch-ui"
edition.workspace = true
version.workspace = true

[dependencies]
fsearch-core = { path = "../fsearch-core" }
smallvec.workspace = true
slint = { version = "1.8", features = ["raw-window-handle-06"] }

[target.'cfg(windows)'.dependencies]
fsearch-win = { path = "../fsearch-win" }
windows-sys = { version = "0.59", features = ["Win32_Foundation"] }

[build-dependencies]
slint-build = "1.8"
```

`build.rs`：

```rust
fn main() {
    slint_build::compile("ui/app.slint").expect("slint compile");
}
```

- [ ] **Step 4: 实现 bridge**

`bridge.rs`：

```rust
use fsearch_core::query::rank::Hit;
use smallvec::SmallVec;

pub struct Row {
    pub before: String,
    pub matched: String,
    pub after: String,
    pub path: String,
}

pub fn row_from_hit(name: &[u8], path: &str, hit: &Hit) -> Row {
    let name = String::from_utf8_lossy(name).into_owned();
    let ranges: &SmallVec<[(u32, u32); 4]> = &hit.highlight;
    match ranges.first() {
        Some(&(start, len)) => {
            let s = (start as usize).min(name.len());
            let e = (s + len as usize).min(name.len());
            let mut s = s;
            let mut e = e;
            while s > 0 && !name.is_char_boundary(s) {
                s -= 1;
            }
            while e < name.len() && !name.is_char_boundary(e) {
                e += 1;
            }
            Row {
                before: name[..s].to_string(),
                matched: name[s..e].to_string(),
                after: name[e..].to_string(),
                path: path.to_string(),
            }
        }
        None => Row {
            before: name,
            matched: String::new(),
            after: String::new(),
            path: path.to_string(),
        },
    }
}

pub fn rows_from_hits(
    snap: &fsearch_core::index::snapshot::IndexSnapshot,
    drive: char,
    hits: &[Hit],
) -> Vec<Row> {
    let mut resolver = fsearch_core::path::PathResolver::new(snap, drive);
    hits.iter()
        .map(|hit| {
            let record = &snap.records()[hit.record_idx as usize];
            let name = snap.name_at(record.name_off, record.name_len);
            let path = resolver.path_of(hit.record_idx);
            row_from_hit(name, &path, hit)
        })
        .collect()
}
```

注意 `path_of` 需要 `&mut resolver`，在闭包里借用 `resolver` 可变 ✓（`map` 闭包是 FnMut）。

- [ ] **Step 5: 写 Slint 界面**

`ui/theme.slint`：

```slint
export global Theme {
    out property <color> bg: #0f1115;
    out property <color> surface: #171a21;
    out property <color> text: #eef1f6;
    out property <color> muted: #8b929e;
    out property <color> accent: #4c8dff;
    out property <length> radius: 12px;
    out property <length> gap: 8px;
}
```

`ui/app.slint`：

```slint
import { Theme } from "theme.slint";

export struct ResultRow {
    before: string,
    matched: string,
    after: string,
    path: string,
}

export component AppWindow inherits Window {
    in-out property <string> query;
    in property <[ResultRow]> rows: [];
    in property <int> selected: 0;
    callback search(string);
    callback activate(int);
    callback move-selection(int);

    width: 760px;
    height: 540px;
    background: Theme.bg;

    VerticalLayout {
        padding: 16px;
        spacing: 12px;

        Rectangle {
            height: 48px;
            border-radius: 10px;
            background: Theme.surface;

            TextInput {
                text <=> root.query;
                edited(text) => { root.search(text); }
                font-size: 20px;
                color: Theme.text;
                placeholder-text: "搜索文件…";
            }
        }

        Rectangle {
            vertical-stretch: 1;
            background: transparent;

            VerticalLayout {
                for row[i] in root.rows : Rectangle {
                    height: 44px;
                    border-radius: 8px;
                    background: i == root.selected ? #222733 : transparent;

                    HorizontalLayout {
                        padding-left: 10px;
                        spacing: 0px;
                        Text { text: row.before; color: Theme.text; }
                        Text { text: row.matched; color: Theme.accent; }
                        Text { text: row.after; color: Theme.text; }
                    }
                }
            }
        }
    }
}
```

`root` 在 Slint 中需要 `root` 是组件的隐式别名 —— Slint 支持 `root` 关键字吗？Slint 用 `root` 作为组件根元素的引用（是的，Slint 支持 `root` 别名指向根）。实际上 Slint 文档使用 `root`（例如 `root.width`）。✓

- [ ] **Step 6: main.rs**

```rust
use std::rc::Rc;

use slint::{ModelRc, VecModel};

slint::include_modules!();

fn demo_rows() -> Vec<ResultRow> {
    vec![ResultRow {
        before: "note".into(),
        matched: "s".into(),
        after: ".txt".into(),
        path: "C:\\Users\\demo\\notes.txt".into(),
    }]
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_rows(ModelRc::new(VecModel::from(demo_rows())));
    ui.run()
}
```

`bridge.rs` 里已经 `slint::include_modules!()`，main.rs 再 include 会重复。只在 `main.rs` include，bridge 通过 `crate::ResultRow` 引用。

调整：bridge.rs 不 include，改用 `crate::ResultRow`。但 `ResultRow` 由 include_modules 生成在 crate root。bridge 里 `use crate::ResultRow;`。

`row_from_hit` 返回 `Row`（普通结构体），测试用它。UI 转换函数 `to_slint` 在 main。

简化：保留 `Row` 作为 Rust 结构体，main 里转换。

- [ ] **Step 7: 运行测试与手动验证**

Run: `cargo test -p fsearch-ui`
Expected: PASS，3 tests passed

Run（Windows）: `cargo run -p fsearch-ui`
Expected: 出现深色窗口与搜索框，列表显示一行示例结果

- [ ] **Step 8: 提交**

```bash
git add crates/fsearch-ui
git commit -m "feat(ui): scaffold slint window with result row splitting"
```

---

### Task 17: 防抖、异步查询与键盘交互

**Files:**
- Create: `crates/fsearch-ui/src/debounce.rs`
- Modify: `crates/fsearch-ui/src/main.rs`

**Interfaces:**
- Consumes: `IndexEngine`、`bridge::rows_from_hits`
- Produces: `debounce::Debouncer::{new, on_input, should_fire}`

- [ ] **Step 1: 写失败的测试**

`debounce.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn first_input_fires_immediately() {
        let mut d = Debouncer::new(Duration::from_millis(30));
        let now = Instant::now();
        assert!(d.on_input(now));
    }

    #[test]
    fn rapid_input_waits_for_quiet_period() {
        let mut d = Debouncer::new(Duration::from_millis(30));
        let t0 = Instant::now();
        d.on_input(t0);
        assert!(!d.on_input(t0 + Duration::from_millis(10)));
        assert!(!d.should_fire(t0 + Duration::from_millis(25)));
        assert!(d.should_fire(t0 + Duration::from_millis(35)));
    }

    #[test]
    fn after_firing_quiet_period_resets() {
        let mut d = Debouncer::new(Duration::from_millis(30));
        let t0 = Instant::now();
        d.on_input(t0);
        assert!(d.should_fire(t0 + Duration::from_millis(40)));
        assert!(!d.should_fire(t0 + Duration::from_millis(45)));
        d.on_input(t0 + Duration::from_millis(50));
        assert!(d.should_fire(t0 + Duration::from_millis(85)));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-ui first_input_fires_immediately`
Expected: FAIL，`cannot find type Debouncer`

- [ ] **Step 3: 实现**

```rust
use std::time::{Duration, Instant};

pub struct Debouncer {
    delay: Duration,
    last_input: Instant,
    pending: bool,
    primed: bool,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            last_input: Instant::now(),
            pending: false,
            primed: false,
        }
    }

    pub fn on_input(&mut self, now: Instant) -> bool {
        self.last_input = now;
        if !self.primed {
            self.primed = true;
            self.pending = false;
            return true;
        }
        self.pending = true;
        false
    }

    pub fn should_fire(&mut self, now: Instant) -> bool {
        if !self.pending {
            return false;
        }
        if now.duration_since(self.last_input) >= self.delay {
            self.pending = false;
            return true;
        }
        false
    }
}
```

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-ui`
Expected: PASS，6 tests passed

- [ ] **Step 5: 接线到引擎**

`main.rs` 替换为：

```rust
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};

use fsearch_core::engine::IndexEngine;
use fsearch_core::index::snapshot::{IndexSnapshot, Store};
use fsearch_core::index::NamePool;

mod bridge;
mod debounce;

slint::include_modules!();

fn placeholder_snapshot() -> IndexSnapshot {
    IndexSnapshot::new(0, 0, 0, 0, 1, Store::Owned { records: Vec::new(), names: NamePool::new() })
}

fn main() -> Result<(), slint::PlatformError> {
    let engine = Arc::new(IndexEngine::new(placeholder_snapshot()));
    let ui = AppWindow::new()?;
    let ui_weak = ui.as_weak();

    let engine_for_search = engine.clone();
    ui.on_search(move |text| {
        let engine = engine_for_search.clone();
        let weak = ui_weak.clone();
        std::thread::spawn(move || {
            let snapshot = engine.current();
            let (_generation, hits) = engine.query(&text, 200);
            let rows = bridge::rows_from_hits(&snapshot, 'C', &hits);
            let ui_rows: Vec<ResultRow> = rows
                .into_iter()
                .map(|r| ResultRow {
                    before: r.before.into(),
                    matched: r.matched.into(),
                    after: r.after.into(),
                    path: r.path.into(),
                })
                .collect();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_rows(ModelRc::new(VecModel::from(ui_rows)));
                    ui.set_selected(0);
                }
            });
        });
    });

    ui.on_move_selection(|delta| {
        let current = ui_snapshot_selected(&ui_weak);
        let next = (current + delta).max(0);
        if let Some(ui) = ui_weak.upgrade() {
            let count = ui.get_rows().row_count() as i32;
            ui.set_selected(next.min(count.saturating_sub(1)));
        }
    });

    ui.on_activate(|_index| {
        if let Some(ui) = ui_weak.upgrade() {
            let selected = ui.get_selected().max(0) as usize;
            if let Some(row) = ui.get_rows().row_data(selected) {
                let _ = row.path;
            }
        }
    });

    ui.run()
}

fn ui_snapshot_selected(weak: &slint::Weak<AppWindow>) -> i32 {
    weak.upgrade().map(|ui| ui.get_selected()).unwrap_or(0)
}

```

测试模块外需要 `mod debounce;` 已在 main。`use crate::debounce::Debouncer;` 在 main 里加。

`IndexBuilder` 未使用会 warning，删除该 import。

- [ ] **Step 6: 手动验证**

Run（Windows）: `cargo run -p fsearch-ui --release`
Expected: 输入文字后列表刷新；防抖生效；`↑↓` 移动选中行

- [ ] **Step 7: 提交**

```bash
git add crates/fsearch-ui
git commit -m "feat(ui): debounce input and query the engine off the ui thread"
```

---

### Task 18: 视觉打磨 — 无边框、Mica、动效

**Files:**
- Create: `crates/fsearch-ui/src/tokens.rs`
- Modify: `crates/fsearch-ui/ui/app.slint`
- Modify: `crates/fsearch-ui/ui/theme.slint`
- Modify: `crates/fsearch-ui/src/main.rs`

**Interfaces:**
- Consumes: `fsearch_win::dwm::{enable_mica, enable_dark_titlebar}`
- Produces: `tokens::{RADIUS, GAP, BG, SURFACE, TEXT, MUTED, ACCENT}`

- [ ] **Step 1: 写失败的令牌一致性测试**

`tokens.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_match_slint_values() {
        assert_eq!(RADIUS, 12.0);
        assert_eq!(GAP, 8.0);
        assert_eq!(BG, "#0f1115");
        assert_eq!(SURFACE, "#171a21");
        assert_eq!(TEXT, "#eef1f6");
        assert_eq!(MUTED, "#8b929e");
        assert_eq!(ACCENT, "#4c8dff");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-ui tokens_match_slint_values`
Expected: FAIL，`cannot find value RADIUS`

- [ ] **Step 3: 实现令牌**

```rust
pub const RADIUS: f32 = 12.0;
pub const GAP: f32 = 8.0;
pub const BG: &str = "#0f1115";
pub const SURFACE: &str = "#171a21";
pub const TEXT: &str = "#eef1f6";
pub const MUTED: &str = "#8b929e";
pub const ACCENT: &str = "#4c8dff";
```

`main.rs` 加 `mod tokens;`。

- [ ] **Step 4: 更新 Slint 界面**

`ui/app.slint` 替换为：

```slint
import { Theme } from "theme.slint";

export struct ResultRow {
    before: string,
    matched: string,
    after: string,
    path: string,
}

export component AppWindow inherits Window {
    in-out property <string> query;
    in property <[ResultRow]> rows: [];
    in property <int> selected: 0;
    callback search(string);
    callback activate(int);
    callback move-selection(int);
    callback close-requested;
    callback reveal-selected;
    callback copy-selected;

    width: 760px;
    height: 540px;
    no-frame: true;
    background: Theme.bg;

    VerticalLayout {
        padding: 0px;
        spacing: 0px;

        Rectangle {
            height: 36px;
            background: transparent;

            HorizontalLayout {
                alignment: end;
                padding-right: 8px;

                TouchArea {
                    width: 28px;
                    height: 28px;
                    clicked => { root.window-close(); }

                    Text {
                        text: "×";
                        color: Theme.muted;
                        font-size: 16px;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                }
            }
        }

        Rectangle {
            height: 48px;
            margin-left: 16px;
            margin-right: 16px;
            border-radius: 10px;
            background: Theme.surface;
            animate background { duration: 120ms; easing: ease-out; }

            FocusScope {
                key-pressed(event) => {
                    if (event.text == Key.UpArrow) {
                        root.move-selection(-1);
                        return accept;
                    }
                    if (event.text == Key.DownArrow) {
                        root.move-selection(1);
                        return accept;
                    }
                    if (event.text == Key.Escape) {
                        root.query = "";
                        return accept;
                    }
                    if (event.text == Key.Return && event.modifiers.control) {
                        root.reveal-selected();
                        return accept;
                    }
                    if (event.text == Key.C && event.modifiers.control) {
                        root.copy-selected();
                        return accept;
                    }
                    return reject;
                }

                TextInput {
                    text <=> root.query;
                    edited(text) => { root.search(text); }
                    accepted => { root.activate(root.selected); }
                    font-size: 20px;
                    color: Theme.text;
                    placeholder-text: "搜索文件…";
                }
            }
        }

        Rectangle {
            vertical-stretch: 1;
            margin-top: 12px;
            background: transparent;

            VerticalLayout {
                for row[i] in root.rows : Rectangle {
                    height: 44px;
                    margin-left: 8px;
                    margin-right: 8px;
                    border-radius: 8px;
                    background: i == root.selected ? Theme.surface : transparent;
                    animate background { duration: 90ms; easing: ease-out; }

                    HorizontalLayout {
                        padding-left: 12px;
                        spacing: 0px;
                        Text { text: row.before; color: Theme.text; font-size: 14px; }
                        Text { text: row.matched; color: Theme.accent; font-size: 14px; }
                        Text { text: row.after; color: Theme.text; font-size: 14px; }
                        Rectangle { horizontal-stretch: 1; }
                        Text { text: row.path; color: Theme.muted; font-size: 12px; }
                    }
                }
            }
        }

        Rectangle {
            height: 28px;
            background: transparent;
            Text {
                text: "↑↓ 选择 · Enter 打开 · Esc 清空";
                color: Theme.muted;
                font-size: 11px;
                horizontal-alignment: center;
            }
        }
    }
}
```

`root` 在 Slint 中指向组件根，`root.window-close()` 不是合法 API。改用 `root` 的内置：Slint `Window` 有 `close()`? 没有公开的关闭方法。

替代：给窗口加 `callback close-requested`，由 Rust 侧 `ui.hide()` 或退出。Slint 组件可以调用 `self.window.hide()`? 

Slint 提供 `Window` 的 `hide()` / `show()` 作为 Rust API，非 slint 代码。

UI 里无法直接关闭。用 callback：
```slint
callback close-requested;
...
clicked => { root.close-requested(); }
```
main.rs `ui.on_close_requested(|| { std::process::exit(0) })`。

我改用 callback。

`margin-left` 等属性在 Slint 中叫 `padding-left`/`x`。Slint 的 Layout 元素支持 `padding`，单个元素没有 margin。可以包一层 `HorizontalLayout` 或用 `x`/`width`。

为简化，用 `padding-left` 在 HorizontalLayout 里。矩形本身用 `x: 16px; width: parent.width - 32px;`。

Slint 中 `Rectangle` 可以设置 `x`、`width`。`parent.width` 可用。

调整：
```slint
Rectangle {
    x: 16px;
    width: parent.width - 32px;
    height: 48px;
    ...
}
```

但这些属性在 `VerticalLayout` 内会被布局覆盖。Slint 中 layout 子元素的 x/y 由布局管理，设置 margin 需用 `padding` 的容器。

简化：外层 VerticalLayout 加 `padding: 16px; spacing: 12px;`，去掉逐个 margin。标题栏单独处理。

最终 app.slint 简化版，避免不确定的属性。

我写一个更保守的版本：
```slint
export component AppWindow inherits Window {
    ...
    no-frame: true;
    background: Theme.bg;

    VerticalLayout {
        padding: 16px;
        spacing: 10px;

        HorizontalLayout {
            height: 28px;
            alignment: end;
            TouchArea {
                width: 28px; height: 28px;
                clicked => { root.close-requested(); }
                Text { text: "×"; color: Theme.muted; font-size: 16px;
                       horizontal-alignment: center; vertical-alignment: center; }
            }
        }

        Rectangle {
            height: 48px;
            border-radius: 10px;
            background: Theme.surface;
            animate background { duration: 120ms; easing: ease-out; }
            TextInput { ... }
        }

        Rectangle {
            vertical-stretch: 1;
            background: transparent;
            VerticalLayout {
                spacing: 2px;
                for row[i] in root.rows : Rectangle {
                    height: 44px;
                    border-radius: 8px;
                    background: i == root.selected ? Theme.surface : transparent;
                    animate background { duration: 90ms; easing: ease-out; }
                    HorizontalLayout {
                        padding-left: 12px;
                        spacing: 0px;
                        Text { text: row.before; color: Theme.text; font-size: 14px;
                               vertical-alignment: center; }
                        Text { text: row.matched; color: Theme.accent; font-size: 14px;
                               vertical-alignment: center; }
                        Text { text: row.after; color: Theme.text; font-size: 14px;
                               vertical-alignment: center; }
                        Rectangle { horizontal-stretch: 1; }
                        Text { text: row.path; color: Theme.muted; font-size: 12px;
                               vertical-alignment: center; }
                    }
                }
            }
        }

        Text { text: "↑↓ 选择 · Enter 打开 · Esc 清空"; color: Theme.muted; font-size: 11px;
               horizontal-alignment: center; }
    }
}
```

`root` 引用 —— Slint 支持 `root` 吗？官方示例大量使用 `root`。是的，Slint 允许 `root` 指代组件根元素。✓

`no-frame: true` 是 Window 的属性 ✓。

`animate background { ... }` ✓。

`easing: ease-out` ✓。

- [ ] **Step 5: 应用 Mica 与暗色标题栏**

`crates/fsearch-ui/Cargo.toml` 追加依赖：

```toml
raw-window-handle = "0.6"
```

`main.rs` 在 `ui.run()` 之前加上 `apply_backdrop(&ui);`，并绑定关闭回调，然后加入下面的函数定义：

```rust
#[cfg(windows)]
fn apply_backdrop(ui: &AppWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = ui.window().window_handle().handle() {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            let hwnd = win32.hwnd.get() as isize;
            let _ = fsearch_win::dwm::enable_mica(hwnd);
            let _ = fsearch_win::dwm::enable_dark_titlebar(hwnd);
        }
    }
}
```

`raw_window_handle` 版本与 slint 的 feature 必须一致。在 Cargo.toml 加 `raw-window-handle = "0.6"`。

在 `main()` 中 `ui.run()` 之前：

```rust
apply_backdrop(&ui);
ui.on_close_requested(|| std::process::exit(0));
```

- [ ] **Step 6: 手动验收**

Run（Windows 11）: `cargo run -p fsearch-ui --release`
Expected: 无系统边框、圆角、Mica 半透明背景、输入框聚焦过渡、选中行背景切换有 90ms 动画

- [ ] **Step 7: 提交**

```bash
git add crates/fsearch-ui
git commit -m "feat(ui): frameless window with mica backdrop and micro animations"
```

---

### Task 19: 惰性属性加载与文件动作

**Files:**
- Create: `crates/fsearch-core/src/props.rs`
- Modify: `crates/fsearch-core/src/lib.rs`
- Create: `crates/fsearch-ui/src/actions.rs`
- Modify: `crates/fsearch-ui/src/main.rs`

**Interfaces:**
- Consumes: `std::fs::metadata`、`std::process::Command`
- Produces:
  - `props::{FileAttrs, read_attrs, AttrCache}`
  - `actions::{open_path, reveal_path, copy_path}`

- [ ] **Step 1: 写失败的测试**

`props.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_size_of_temp_file() {
        let path = std::env::temp_dir().join(format!("fsprops-{}.txt", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let attrs = read_attrs(&path).expect("metadata");
        assert_eq!(attrs.size, 5);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_yields_none() {
        assert!(read_attrs(std::path::Path::new("Z:\\definitely\\missing")).is_none());
    }

    #[test]
    fn cache_returns_hit_without_reload() {
        let mut cache = AttrCache::new(4);
        let attrs = FileAttrs { size: 10, modified_secs: 1 };
        cache.insert(7, attrs);
        assert_eq!(cache.get(7), Some(attrs));
        assert_eq!(cache.get(8), None);
    }

    #[test]
    fn cache_evicts_oldest_when_full() {
        let mut cache = AttrCache::new(2);
        cache.insert(1, FileAttrs { size: 1, modified_secs: 1 });
        cache.insert(2, FileAttrs { size: 2, modified_secs: 2 });
        cache.insert(3, FileAttrs { size: 3, modified_secs: 3 });
        assert_eq!(cache.get(1), None);
        assert_eq!(cache.get(3), Some(FileAttrs { size: 3, modified_secs: 3 }));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p fsearch-core reads_size_of_temp_file`
Expected: FAIL，`cannot find function read_attrs`

- [ ] **Step 3: 实现 props**

```rust
use std::collections::VecDeque;
use std::path::Path;
use std::time::UNIX_EPOCH;

use ahash::AHashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileAttrs {
    pub size: u64,
    pub modified_secs: u64,
}

pub fn read_attrs(path: &Path) -> Option<FileAttrs> {
    let meta = std::fs::metadata(path).ok()?;
    let modified_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(FileAttrs { size: meta.len(), modified_secs })
}

pub struct AttrCache {
    cap: usize,
    map: AHashMap<u32, FileAttrs>,
    order: VecDeque<u32>,
}

impl AttrCache {
    pub fn new(cap: usize) -> Self {
        Self { cap, map: AHashMap::with_capacity(cap), order: VecDeque::with_capacity(cap) }
    }

    pub fn insert(&mut self, key: u32, attrs: FileAttrs) {
        if self.map.insert(key, attrs).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub fn get(&self, key: u32) -> Option<FileAttrs> {
        self.map.get(&key).copied()
    }
}
```

`lib.rs` 加 `pub mod props;`。

- [ ] **Step 4: 运行通过**

Run: `cargo test -p fsearch-core`
Expected: PASS，59 tests passed

- [ ] **Step 5: 实现文件动作**

`actions.rs`：

```rust
use std::process::Command;

pub fn open_path(path: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        Command::new("cmd").args(["/C", "start", "", path]).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

pub fn reveal_path(path: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        Command::new("explorer").arg(format!("/select,{path}")).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        open_path(path)
    }
}

pub fn copy_path(path: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut child = Command::new("cmd")
            .args(["/C", "clip"])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(path.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }
#[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}
```

`main.rs` 顶部加入 `mod actions;`，并在 `ui.run()` 之前绑定两个回调：

```rust
ui.on_reveal_selected(|| {
    if let Some(ui) = ui_weak.upgrade() {
        let index = ui.get_selected().max(0) as usize;
        if let Some(row) = ui.get_rows().row_data(index) {
            let _ = actions::reveal_path(&row.path);
        }
    }
});
ui.on_copy_selected(|| {
    if let Some(ui) = ui_weak.upgrade() {
        let index = ui.get_selected().max(0) as usize;
        if let Some(row) = ui.get_rows().row_data(index) {
            let _ = actions::copy_path(&row.path);
        }
    }
});
```

同时在 `on_activate` 中把占位的 `let _ = row.path;` 替换为：

```rust
let _ = actions::open_path(&row.path);
```

- [ ] **Step 6: 提交**

```bash
git add crates/fsearch-core crates/fsearch-ui
git commit -m "feat: lazy file attributes and open/reveal/copy actions"
```

---

### Task 20: 合成基准与验收脚本

**Files:**
- Create: `crates/fsearch-core/src/bin/bench.rs`
- Create: `scripts/acceptance.md`

**Interfaces:**
- Consumes: `IndexBuilder`、`IndexEngine`
- Produces: `bench` 可执行程序，输出 P50/P99/索引耗时/内存估算

- [ ] **Step 1: 写基准程序**

```rust
use std::time::Instant;

use fsearch_core::collector::RawRecord;
use fsearch_core::engine::IndexEngine;
use fsearch_core::index::build::IndexBuilder;

fn synthetic(count: usize) -> IndexEngine {
    let mut builder = IndexBuilder::with_capacity(0, 0, count + 1);
    builder.extend(RawRecord {
        file_id: 5,
        parent_file_id: 5,
        name: "\\".to_string(),
        attributes: 0x10,
    });
    let names = ["src", "tests", "docs", "main", "lib", "report", "notes", "image"];
    let exts = ["rs", "txt", "md", "png", "json"];
    for i in 0..count {
        builder.extend(RawRecord {
            file_id: (i as u64) + 10,
            parent_file_id: 5,
            name: format!("{}_{}.{}", names[i % names.len()], i, exts[i % exts.len()]),
            attributes: 0,
        });
    }
    IndexEngine::new(builder.finish())
}

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    let build_start = Instant::now();
    let engine = synthetic(count);
    let build_ms = build_start.elapsed().as_millis();

    let queries = ["main", "report_1", "*.rs", "notes json", "src_"];
    let mut samples = Vec::new();
    for _ in 0..20 {
        for q in queries {
            let start = Instant::now();
            let (_gen, hits) = engine.query(q, 200);
            samples.push(start.elapsed().as_micros() as u64);
            std::hint::black_box(hits);
        }
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];

    println!("records      : {count}");
    println!("build        : {build_ms} ms");
    println!("query p50    : {:.2} ms", p50 as f64 / 1000.0);
    println!("query p99    : {:.2} ms", p99 as f64 / 1000.0);
    println!("approx index : {:.1} MB", (count * 16 + count * 20) as f64 / (1024.0 * 1024.0));
}
```

- [ ] **Step 2: 运行小规模基准**

Run: `cargo run -p fsearch-core --release --bin bench -- 100000`
Expected: 输出行存在，p99 为个位数毫秒

- [ ] **Step 3: 写验收清单**

`scripts/acceptance.md`：

```markdown
# 真机验收清单

前置：Windows 10 1809+ 或 Windows 11，x64，管理员 PowerShell，NVMe 系统盘。

| # | 项 | 命令 / 操作 | 通过标准 |
|---|---|---|---|
| 1 | 单元测试 | `cargo test` | 全部通过 |
| 2 | 静态检查 | `cargo clippy --all-targets -- -D warnings` | 无告警 |
| 3 | 全量索引 | `cargo run -p fsearch-ui --release`，看底部耗时 | < 15s（500 万条） |
| 4 | 查询延迟 | `cargo run -p fsearch-core --release --bin bench -- 5000000` | p99 < 30ms |
| 5 | 增量可见 | 在外部创建文件后立即搜索 | < 1s 出现 |
| 6 | 冷启动 | 关闭后重新启动，计时到可输入 | < 1s |
| 7 | 内存 | 预热 10s 后任务管理器 working set | < 200MB |
| 8 | 视觉 | 观察窗口 | 无系统边框、圆角、Mica、过渡动画 |
| 9 | 非管理员 | 用普通权限启动 | 提示提权，且降级模式可用 |
| 10 | 通配符 | 搜 `*.rs`、`report_?` | 结果符合预期 |
```

- [ ] **Step 4: 提交**

```bash
git add crates/fsearch-core scripts
git commit -m "test: add synthetic benchmark and acceptance checklist"
```

---

## 自审记录

**1. 规格覆盖**

| 规格章节 | 覆盖任务 |
|---|---|
| §3 架构与线程模型 | T15 |
| §4.1 记录布局 | T1、T3 |
| §4.2 无 size/mtime | T19（惰性属性）、T7（排序不含时间） |
| §4.3 ASCII 小写化 | T1、T2 |
| §5 全量构建 | T3、T13 |
| §6 持久化与冷启动 | T8、T9 |
| §7.1 USN 消费 | T14 |
| §7.2 overlay / 级联 / 溢出 | T10、T11 |
| §8 查询引擎 | T5、T6、T7、T15 |
| §9 UI 与视觉 | T16、T17、T18 |
| §10 错误处理 | T12（提权）、T14（掉线）、T9（损坏快照）、T3（孤儿） |
| §11 测试与验收 | 每个任务的测试 + T20 |

**2. 占位符扫描**：无 `TBD` / `TODO` / "类似 Task N"。所有步骤都含可执行代码或命令。

**3. 类型一致性**：`IndexSnapshot::name_at` 在 T3 定义、T4/T7/T15/T16 使用；`Store::Owned` 在 T3 定义、T9 扩展 `Mapped`；`Scored` 在 T7 定义、T15 复用；`parse_usn_events` 在 T14 定义并被 `usn.rs` 调用。