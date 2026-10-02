# 03 交易

## 1. 交易类型

```
Tx = enum {
  0x00 Coinbase         系统交易
  0x01 Expire           系统交易
  0x02 AutoCancel       系统交易
  0x10 Transfer
  0x11 RegisterIdentity
  0x20 BeginCourse
  0x21 AppendFiles
  0x22 AddLock
  0x23 FinalizeCourse
  0x24 CancelCourse
  0x30 VerifyAuthor
  0x31 RotateKey        保留，尚未启用
}
```

未启用的标签视为未定义（CODEC-DEC-04）。

```
txid = TH("YuanLai/tx/v1", Tx 的完整编码（含签名）)
```

## 2. 公共结构

```
OutPoint = { txid: Hash, index: u32 }
```

**OutPoint 顺序**：先按 `txid` 的字节序比较，相同时再按 `index` 的**数值**比较。注意这不同于按编码字节比较，因为 `index` 是小端序编码。

每笔**用户交易**都以如下字段开头，以签名结尾：

```
sender:    PubKey
nonce:     u64
…类型特有字段…
signature: Signature
```

签名消息：

```
sig_msg = TH("YuanLai/tx-sig/v1", chain_id ‖ 除 signature 外的完整编码（含类型标签）)
```

## 3. 用户交易的通用规则

- **TX-SIG-01**：`sender` 是有效公钥（定义同 TX-CB-02），并且 `signature` 是 `sender` 对 32 字节消息 `sig_msg` 的有效 Ed25519 签名（RFC 8032），按严格模式验证：拒绝非规范编码的 `S`、非规范编码的点，以及小阶的 `R`（与 `ed25519-dalek` 的 `verify_strict` 一致）。
- **TX-NONCE-01**：`nonce` 等于 `sender` 当前的账户序号。交易生效后序号加 1。账户序号初始为 0。
- **TX-SIZE-01**：交易编码长度 ≤ `MAX_TX_SIZE`。

## 4. 输入与找零（适用于带 `inputs` 字段的交易）

交易需要从 `sender` 名下取出 `R` 个额度（`R` 由各交易类型定义）时，使用 `inputs: List<OutPoint, MAX_INPUTS>` 字段：

- **TX-IN-01**：`inputs` 中没有重复项。
- **TX-IN-02**：每个输入都存在、属于 `sender`，并且在当前高度**有效**（定义见 04 §2）。
- **TX-IN-03**：输入按 `(expire_height, OutPoint)` 升序排列（OutPoint 顺序见 §2）。这个顺序即为**消耗顺序**（先到期的先用）。
- **TX-IN-04**：设 `Sₖ` 为前 `k` 个输入的金额之和，`n` 为输入个数，那么必须满足 `S(n-1) < R ≤ Sₙ`。也就是说，每个输入都是必需的：不能少，也不能多。
- **TX-IN-05**：`R > 0` 时 `inputs` 不能为空；`R = 0` 的交易类型没有 `inputs` 字段。

**找零**：如果 `Sₙ > R`，那么最后一个输入剩余的 `Sₙ - R` 成为一个新的 UTXO，完整继承该输入的 `source`、`acquired_height`、`expire_height`、`hops` 和 `renewals`。找零不会重新计时。

**被消耗部分的 hops 与 renewals**：`consumed_hops = max(被消耗的各输入的 hops)`，`consumed_renewals = max(被消耗的各输入的 renewals)`，最后一个输入即使只被部分消耗，也计入其中。

## 5. 输出编号

交易新建的 UTXO 按下表编号（`OutPoint.index`）。金额为 0 的输出不会创建，但**编号保留**，以保证编号固定。

| 交易 | index 0 | index 1 |
|---|---|---|
| `Coinbase` | 奖励 | — |
| `Transfer` | 接收方所得 | 找零 |
| `RegisterIdentity` / `VerifyAuthor` / `BeginCourse` / `AddLock` | 找零 | — |
| `FinalizeCourse` | 释放的剩余额度 | — |
| `CancelCourse` | 恢复的额度 | — |
| `AutoCancel` | 第 i 个被取消会话恢复的额度，位于 index i | |

## 6. 各交易类型

### 6.1 Coinbase（系统交易）

```
Coinbase = { height: Height, beneficiary: PubKey, extra: VarBytes<MAX_COINBASE_EXTRA> }
```

- **TX-CB-01**：`height` 等于所在区块的高度（这保证不同区块的 Coinbase 有不同的 txid）。
- **TX-CB-02**：`beneficiary` 是**有效公钥**：能解压为 Ed25519 曲线上的点；解压后重新压缩得到完全相同的 32 字节（规范编码）；并且不是小阶点。
- 产生的额度及其属性见 04 §3。

### 6.2 Expire（系统交易）

```
Expire = { height: Height, count: u32, records_root: Hash }
```

- **TX-EXP-01**：`height` 等于所在区块的高度。
- **TX-EXP-02**：`count` 与 `records_root` 等于 04 §6 中确定性计算的结果。

### 6.3 AutoCancel（系统交易）

```
AutoCancel = { height: Height, count: u32, records_root: Hash }
```

- **TX-AC-01**：`height` 等于所在区块的高度。
- **TX-AC-02**：`count` 与 `records_root` 等于 05 §7 中确定性计算的结果。

### 6.4 Transfer

```
Transfer = { sender, nonce, inputs, recipient: PubKey, amount: Credit, signature }
```

`R = amount`。规则见 04 §4。

### 6.5 RegisterIdentity

```
RegisterIdentity = { sender, nonce, inputs, commitment: Hash, signature }
```

`R = COST_IDENTITY`。

- **TX-ID-01**：`sender` 尚未登记身份。
- **TX-ID-02**：`commitment` 没有被任何地址登记过。
- **TX-ID-03**：`commitment` 是电路域元素的规范编码：按 32 字节**小端序**解释为整数后，小于 `CIRCUIT_FIELD_MODULUS`（Pallas 曲线的基域，即 halo2 电路的原生域，见 parameters.md）。
- 效果：`R` 被销毁；`identity[sender] = commitment`。

### 6.6 课程会话交易

`BeginCourse`、`AppendFiles`、`AddLock`、`FinalizeCourse`、`CancelCourse` 见 05。

### 6.7 VerifyAuthor

```
VerifyAuthor = {
  sender, nonce, inputs,
  course_root: Hash, challenge: Hash, nullifier: Hash,
  proof: VarBytes<MAX_PROOF_SIZE>,
  signature
}
```

`R = COST_VERIFY_AUTHOR`。

- **TX-VA-01**：存在以 `course_root` 为根、所有者为 `sender` 的课程记录。
- **TX-VA-02**：`sender` 已登记身份。
- **TX-VA-03**：`nullifier` 从未出现过。
- **TX-VA-04**：以 `(identity[sender], challenge, course_root, nullifier)` 为公开输入，`proof` 能通过 06 中定义的验证。
- 效果：`R` 被销毁；`nullifier` 加入已用集合；记录一条本人验证记录。

`challenge` 由验证方在链下生成并交给作者。验证方确认链上存在包含自己 `challenge` 的 `VerifyAuthor` 记录，即完成验证。
