# 05 课程写入会话

## 1. 文件条目

```
FileEntry = { path: Text<MAX_PATH_BYTES>, content_hash: Hash, size: u64 }
```

`content_hash` 是文件内容的**普通 SHA-256**（不带标签）。这样任何人都可以用系统自带工具（例如 `shasum -a 256`）独立核对。

路径规则：
- **CRS-PATH-01**：以 `/` 分隔成若干段；不以 `/` 开头或结尾；没有空段。
- **CRS-PATH-02**：没有等于 `.` 或 `..` 的段。
- **CRS-PATH-03**：不包含 `\`。

## 2. 会话状态

```
Session = {
  id:                   Hash               // BeginCourse 的 txid
  owner:                PubKey
  parent_root:          Option<Hash>       // 有值表示登记新版本
  title:                Text<MAX_TITLE_BYTES>
  version:              Text<MAX_VERSION_BYTES>
  declared_files:       u32
  declared_bytes:       u64
  locked:               Credit
  max_hops:             u8
  max_renewals:         u8
  files:                按 path 排序的 FileEntry 集合
  started_height:       Height
  last_activity_height: Height
}
```

- 截止高度：`deadline = min(started_height + SESSION_MAX_BLOCKS, last_activity_height + SESSION_IDLE_BLOCKS)`。
- 会话以 `FinalizeCourse`、`CancelCourse` 或 `AutoCancel` 结束。结束后从状态中移除，只保留历史记录。

## 3. 计费

```
gib_units(bytes) = ⌈bytes / GIB⌉

新课程：cost = COST_COURSE_BASE + files × COST_PER_FILE + gib_units(bytes) × COST_PER_GIB
新版本：cost = COST_UPDATE_BASE + files × COST_PER_FILE + gib_units(bytes) × COST_PER_GIB
```

- 新课程：`files`、`bytes` 统计全部文件。
- 新版本：`files`、`bytes` 只统计**变更文件**，即 `content_hash` 不在父课程文件集合中的条目。

两种用法：
- **预估费用** `estimate`：代入 `declared_files`、`declared_bytes` 计算。
- **实际费用** `actual`：代入会话中的实际文件计算。

锁定区间：

```
estimate ≤ locked ≤ estimate + ⌊estimate × LOCK_HEADROOM_PERCENT / 100⌋
```

## 4. BeginCourse

```
BeginCourse = {
  sender, nonce, inputs,
  parent_root: Option<Hash>,
  title, version,
  declared_files: u32, declared_bytes: u64,
  lock: Credit,
  signature
}
```

`R = lock`。

- **CRS-BEGIN-01**：`sender` 已登记身份。
- **CRS-BEGIN-02**：`sender` 没有未结束的会话。
- **CRS-BEGIN-03**：`1 ≤ declared_files ≤ MAX_FILES_PER_COURSE`。
- **CRS-BEGIN-04**：`lock` 在锁定区间内（§3）。
- **CRS-BEGIN-05**：如果 `parent_root` 有值，它必须是 `sender` 拥有的某个课程记录，并且是其版本链的**最新版本**（即 `sender` 没有任何以它为 `parent_root` 的课程记录）。
- **CRS-BEGIN-06**：每个输入的 `renewals < MAX_RENEWALS`。
- 效果：创建会话，`locked = lock`，`max_hops = consumed_hops`，`max_renewals = consumed_renewals`，`started_height = last_activity_height = h`；找零按 03 §4 处理。

## 5. AppendFiles / AddLock

```
AppendFiles = { sender, nonce, session_id: Hash, entries: List<FileEntry, MAX_FILES_PER_APPEND>, signature }
```

- **CRS-APPEND-01**：`session_id` 是 `sender` 的未结束会话。
- **CRS-APPEND-02**：每个条目都满足路径规则。
- **CRS-APPEND-03**：本次条目之间、本次条目与会话已有条目之间，`path` 都不重复。
- **CRS-APPEND-04**：同样地，`content_hash` 都不重复。
- **CRS-APPEND-05**：追加后会话的文件总数 ≤ `MAX_FILES_PER_COURSE`。
- **CRS-APPEND-06**：`entries` 非空（空追加只会延长截止高度，没有任何实际作用）。
- **CRS-APPEND-07**：追加后会话内所有文件的 `size` 之和不超过 `u64` 的范围。
- 效果：条目加入会话；`last_activity_height = h`。不消耗额度。

```
AddLock = { sender, nonce, inputs, session_id: Hash, declared_files: u32, declared_bytes: u64, amount: Credit, signature }
```

`R = amount`。

- **CRS-ADDLOCK-01**：`session_id` 是 `sender` 的未结束会话。
- **CRS-ADDLOCK-02**：`1 ≤ declared_files ≤ MAX_FILES_PER_COURSE`。
- **CRS-ADDLOCK-03**：按新的声明重新计算锁定区间，`locked + amount` 在区间内。
- **CRS-ADDLOCK-04**：每个输入的 `renewals < MAX_RENEWALS`。
- **CRS-ADDLOCK-05**：`amount > 0`。
- 效果：更新声明；`locked += amount`；`max_hops = max(max_hops, consumed_hops)`；`max_renewals = max(max_renewals, consumed_renewals)`；`last_activity_height = h`。

## 6. FinalizeCourse / CancelCourse

```
FinalizeCourse = { sender, nonce, session_id: Hash, signature }
```

- **CRS-FIN-01**：`session_id` 是 `sender` 的未结束会话。
- **CRS-FIN-02**：会话中至少有 1 个文件。
- **CRS-FIN-03**：如果是新版本，`parent_root` 此时仍然是其版本链的最新版本，并且至少有 1 个变更文件。
- **CRS-FIN-04**：`actual ≤ locked`。
- **CRS-FIN-05**：`sender` 没有其他课程记录的课程根等于本次计算出的 `course_root`（同一作者内课程根唯一，保证版本链没有歧义）。

效果：
1. `course_root = MTH([FileEntry 编码，按 path 字节序升序])`。
2. 创建课程记录：

```
Course = {
  id:               Hash       // 等于 session_id
  root, owner, parent_root, title, version,
  file_count:       u32
  total_size:       u64
  finalized_height: Height     // h
  position:         u32        // FinalizeCourse 在区块 txs 中的下标
}
```

课程的文件条目随课程记录一起保存。

3. `actual` 被销毁。
4. `locked - actual` 作为 UTXO 释放给 `owner`：`source = Released`，从 `h` 起算 `EXPIRY_BLOCKS`，`hops = max_hops`，`renewals = max_renewals + 1`。
5. 会话结束。

同一个 `course_root` 或 `content_hash` **可以**被不同的人重复登记。所有登记都会保留，并按 `(finalized_height, position)` 全序排列；**最早的一条即为时间在先的证据**。理由见 [rationale.md](rationale.md#不强制内容哈希全局唯一)。

```
CancelCourse = { sender, nonce, session_id: Hash, signature }
```

- **CRS-CANCEL-01**：`session_id` 是 `sender` 的未结束会话。
- **CRS-CANCEL-02**：`sender` 在区间 `(h - CANCEL_LIMIT_WINDOW, h]` 内手动取消的次数 < `CANCEL_LIMIT`。

效果：

```
fee      = min(locked, max(CANCEL_MIN_FEE, ⌊locked × CANCEL_FEE_PERCENT / 100⌋))
restored = locked - fee
```

`fee` 被销毁；`restored` 作为 UTXO 恢复给 `owner`：`source = Cancelled`，从 `h` 起算 `EXPIRY_BLOCKS`，`hops = max_hops`，`renewals = max_renewals + 1`。会话结束。

## 7. AutoCancel

在高度 `h` 的区块中，作废处理（04 §6）之后、Coinbase 之前：

1. 找出所有 `deadline = h` 的会话，按 `session_id` 升序排列。
2. 对每个会话按 CancelCourse 的方式计算 `fee` 和 `restored`，并执行同样的效果。自动取消**不**计入 CRS-CANCEL-02 的次数。
3. 生成记录：

```
AutoCancelRecord = { session_id: Hash, owner: PubKey, locked: Credit, fee: Credit, restored: Credit }
```

4. `AutoCancel.count` = 记录条数，`AutoCancel.records_root = MTH([AutoCancelRecord 编码, …])`。
5. 第 `i` 个会话恢复的 UTXO 编号为 `(AutoCancel 的 txid, i)`。

## 8. 区块内处理顺序

```
1. Expire        （移除 expire_height = h 的 UTXO）
2. AutoCancel    （结束 deadline = h 的会话）
3. Coinbase      （按处理后的余额计算发放量）
4. 用户交易，按区块中的顺序
```

因此，在高度 `h` 的区块中，用户交易无法花费恰好在 `h` 作废的额度，也无法操作恰好在 `h` 超时的会话。
