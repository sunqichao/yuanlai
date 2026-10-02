# 01 规范化编码、哈希与 Merkle 树

## 1. 基本类型

| 类型 | 编码 |
|---|---|
| `u8` / `u16` / `u32` / `u64` | 定长，**小端序** |
| `bool` | 1 字节，只能是 `0x00` 或 `0x01` |
| `Bytes32` / `Bytes64` | 定长原始字节 |
| `VarBytes<MAX>` | `u32` 长度 + 原始字节；长度 ≤ `MAX` |
| `Text<MAX>` | 与 `VarBytes<MAX>` 编码相同，另有文本规则（见 §2） |
| `List<T, MAX>` | `u32` 元素个数 + 依次编码的元素；个数 ≤ `MAX` |
| `Option<T>` | `0x00` 表示无；`0x01` 后接 `T` |
| 枚举 | `u8` 标签 + 对应变体的内容 |
| 结构体 | 按定义顺序依次编码各字段，没有分隔符和字段名 |

派生类型：

| 类型 | 底层类型 |
|---|---|
| `Height` | `u64` |
| `Credit` | `u64` |
| `Timestamp` | `u64`，Unix 秒 |
| `PubKey` | `Bytes32`，Ed25519 压缩公钥 |
| `Signature` | `Bytes64`，Ed25519 签名 |
| `Hash` | `Bytes32` |

## 2. 文本规则

`Text` 的内容必须满足：
- **CODEC-TEXT-01**：是合法的 UTF-8。
- **CODEC-TEXT-02**：不包含 C0 控制字符（U+0000 至 U+001F）、U+007F，以及 C1 控制字符（U+0080 至 U+009F）。

共识层**不**要求 Unicode 规范化（例如 NFC），文本按字节比较。客户端**应当**在提交前把文本规范化为 NFC。理由见 [rationale.md](rationale.md#文本不做-unicode-规范化)。

## 3. 严格解码

解码器必须拒绝以下所有输入：
- **CODEC-DEC-01**：解码完成后还有剩余字节。
- **CODEC-DEC-02**：长度或个数超过类型上限。
- **CODEC-DEC-03**：`bool` 不是 `0x00` 或 `0x01`；`Option` 标签不是 `0x00` 或 `0x01`。
- **CODEC-DEC-04**：枚举标签未定义。
- **CODEC-DEC-05**：`Text` 违反 §2 的规则。
- **CODEC-DEC-06**：输入在某个字段中途结束。

由于所有类型都是定长编码或带精确长度前缀，每个值**有且只有一种**合法编码。因此"解码后再编码"必须得到完全相同的字节。

## 4. 带标签哈希

```
TH(tag, m) = SHA256( SHA256(tag) ‖ SHA256(tag) ‖ m )
```

其中 `tag` 是 ASCII 字符串，`‖` 表示字节拼接（与 BIP-340 相同的构造）。

协议中使用的全部标签：

| 标签 | 用途 |
|---|---|
| `YuanLai/chain-id/v1` | 网络标识 |
| `YuanLai/block/v1` | 区块 ID 与工作量证明 |
| `YuanLai/tx/v1` | 交易 ID |
| `YuanLai/tx-sig/v1` | 交易签名消息 |
| `YuanLai/merkle-empty/v1` | 空 Merkle 树 |
| `YuanLai/merkle-leaf/v1` | Merkle 叶子 |
| `YuanLai/merkle-node/v1` | Merkle 内部节点 |

新增标签必须写入本表。任何两个用途**不得**共用同一个标签。

## 5. Merkle 树

采用 RFC 6962 的树形结构，并使用带标签哈希。对有序数据列表 `D = [d0, d1, …, d(n-1)]`：

```
MTH([])      = TH("YuanLai/merkle-empty/v1", "")
MTH([d0])    = TH("YuanLai/merkle-leaf/v1", d0)
MTH(D), n>1  = TH("YuanLai/merkle-node/v1", MTH(D[0:k]) ‖ MTH(D[k:n]))
```

其中 `k` 是**小于 n 的最大 2 的幂**。

这种结构不会复制最后一个元素，因此不存在比特币 Merkle 树的重复元素问题（CVE-2012-2459）。叶子和内部节点使用不同的标签，因此也无法把内部节点伪装成叶子。

## 6. 数值比较

工作量证明中，32 字节的哈希值和难度目标都按**大端序**解释为 256 位无符号整数，然后进行比较。这是整个协议中唯一使用大端序的地方。

电路域元素（身份承诺等）按 32 字节**小端序**编码，与 halo2 / Pasta 库的标准表示一致。
