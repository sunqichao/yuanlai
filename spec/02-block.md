# 02 区块

## 1. 区块头

| 字段 | 类型 | 说明 |
|---|---|---|
| `version` | `u32` | 当前为 `1` |
| `chain_id` | `Hash` | 见 00 §5 |
| `height` | `Height` | 创世区块为 0 |
| `prev_block` | `Hash` | 父区块 ID；创世区块为全零 |
| `tx_root` | `Hash` | `MTH([txid₀, txid₁, …])` |
| `timestamp` | `Timestamp` | |
| `target` | `Bytes32` | 难度目标（完整 256 位，大端序） |
| `nonce` | `u64` | |

区块头编码后固定为 156 字节。

```
block_id = TH("YuanLai/block/v1", 区块头编码)
```

## 2. 区块

```
Block = { header: 区块头, txs: List<Tx, MAX_BLOCK_TXS> }
```

`Tx` 的定义见 03。

## 3. 区块有效性规则

对高度为 `h`、父区块为 `P` 的区块 `B`：

- **BLK-01**：`B` 能被严格解码，编码长度 ≤ `MAX_BLOCK_SIZE`。
- **BLK-02**：`version = 1`。
- **BLK-03**：`chain_id` 等于本网络的 chain_id。
- **BLK-04**：`P` 是已知且有效的区块，并且 `h = P.height + 1`。
- **BLK-05**：`target = NextTarget(P)`（见 §4）。
- **BLK-06**：将 `block_id` 解释为大端序整数后，≤ `target`。
- **BLK-07**：`timestamp` > `MTP(P)`。`MTP(P)` 是以 `P` 结尾、最多 `MTP_WINDOW` 个区块的时间戳中位数；数量为偶数时取两者中较小的那个。
- **BLK-08**：如果 `h` 是 `RETARGET_INTERVAL` 的倍数，那么 `timestamp` ≥ `P.timestamp - TIMEWARP_GRACE`（防止时间扭曲攻击）。
- **BLK-09**：`txs` 的前三笔依次是 `Expire`、`AutoCancel`、`Coinbase`，其余全部是用户交易。系统交易**不得**出现在其他位置。
- **BLK-10**：`tx_root = MTH([txid of each tx in txs])`。
- **BLK-11**：`txs` 中没有重复的 txid。
- **BLK-12**：同一个 `sender` 的用户交易数量 ≤ `MAX_USER_TXS_PER_SENDER_PER_BLOCK`。
- **BLK-13**：以 `P` 之后的状态为起点，按顺序应用 `txs` 中的每笔交易，都满足 03 至 05 中的全部规则。

### 接收规则（非共识）

- **BLK-ACCEPT-01**：节点应当暂不接受 `timestamp` > 本地时间 + `MAX_FUTURE_DRIFT` 的区块，但**不得**将其永久标记为无效；本地时间追上后应当重新评估。

## 4. 难度调整

设 `h = P.height + 1`，`I = RETARGET_INTERVAL`，`E = I × TARGET_BLOCK_SPACING`：

```
NextTarget(P):
  if h mod I ≠ 0 or h < 2I:
      return P.target
  actual  = ts(h-1) - ts(h-I-1)        // 有符号运算，恰好覆盖 I 个出块间隔
  actual  = clamp(actual, ⌊E / MAX_RETARGET_FACTOR⌋, E × MAX_RETARGET_FACTOR)
  q, r    = ⌊P.target / E⌋, P.target mod E
  next    = q × actual + ⌊r × actual / E⌋   // 等于 ⌊P.target × actual / E⌋
  return clamp(next, 1, POW_LIMIT)       // 回归测试网为 POW_LIMIT_REGTEST，见 00 §5
```

`ts(x)` 表示当前链上高度为 `x` 的区块的时间戳。

溢出分析：由于 `P.target = qE + r`，有 `⌊P.target × actual / E⌋ = q × actual + ⌊r × actual / E⌋`。其中 `q × actual ≤ 4 × P.target < 4 × 2²⁵³ = 2²⁵⁵`，`r × actual < E × 4E < 2³²`，所以所有中间值都小于 2²⁵⁶，对所有网络都不会溢出。

## 5. 链工作量与分叉选择

```
work(target) = ⌊(2²⁵⁶ - 1 - target) / (target + 1)⌋ + 1     // 等于 ⌊2²⁵⁶ / (target + 1)⌋
chain_work(B) = Σ work(target)，对从创世区块到 B 的所有区块求和
```

- **FORK-01**：节点必须把 `chain_work` 最大的有效链作为主链。
- **FORK-02**：`chain_work` 相同时，保留先收到的链（本地策略，非共识）。
- **FORK-03**：切换主链时，必须用 undo 记录回滚到分叉点，再依次应用新链的区块。

## 6. 创世区块

- `height = 0`，`prev_block` 为全零，`target` 等于该网络的 `POW_LIMIT`。
- `txs` 只包含一笔 `Coinbase`，它**不产生任何额度**（不预挖），`beneficiary` 为 32 个零字节。它的 `extra` 字段写入一段公开可查的近期信息（例如当日新闻标题），用来证明创世区块不早于该日期生成。
- 创世区块豁免 BLK-07、BLK-09 和 TX-CB-02。
- 各网络的创世区块在启动时确定，并写入 [parameters.md](parameters.md)。
