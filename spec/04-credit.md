# 04 存证额度（Credit）

## 1. UTXO

```
Utxo = {
  owner:           PubKey
  amount:          Credit          // > 0
  source:          enum { Mined = 0, Received = 1, Released = 2, Cancelled = 3 }
  acquired_height: Height          // 本轮有效期的起算高度
  spendable_from:  Height          // 成熟高度；非 Coinbase 产生的等于 acquired_height
  expire_height:   Height
  hops:            u8              // 已被转账的次数
  renewals:        u8              // 因会话结束而重新计时的次数
}
```

锁定在会话中的额度**不**以 UTXO 形式存在，而是计入会话的 `locked` 字段（见 05）。因此锁定期间不会过期。

## 2. 有效性

在高度 `h` 的区块中，UTXO `u` **有效**，当且仅当：

```
u.spendable_from ≤ h  并且  h < u.expire_height
```

`u.expire_height` 就是它的**作废高度**：在该高度的区块中，`u` 被 `Expire` 移除。

## 3. 产生：Coinbase

设区块高度为 `h`，受益人为 `b`：

```
amount = min(BLOCK_REWARD, BALANCE_CAP - balance(b))    // balance 取 Expire 与 AutoCancel 处理之后的值
```

- 如果 `amount > 0`，创建 UTXO：`source = Mined`，`acquired_height = h`，`spendable_from = h + COINBASE_MATURITY`，`expire_height = h + COINBASE_MATURITY + EXPIRY_BLOCKS`，`hops = 0`，`renewals = 0`。
- 如果 `amount = 0`，不创建任何 UTXO。**超出上限的部分直接不发放，也不会累计。**
- 高度 0 的 Coinbase 不产生额度。

## 4. 转账

对 `Transfer { sender, inputs, recipient, amount }`，设当前高度为 `h`：

- **TX-TRANSFER-01**：`amount ≥ 2`。
- **TX-TRANSFER-02**：`recipient ≠ sender`。
- **TX-TRANSFER-03**：每个输入的 `hops < MAX_HOPS`。
- **TX-TRANSFER-04**：`recipient` 已登记身份。
- **TX-TRANSFER-05**（防回转）：在高度区间 `(h - NO_RETURN_WINDOW, h]` 内，不存在 `recipient` 转给 `sender` 的 `Transfer`，包括本区块中排在前面的交易。
- **TX-TRANSFER-06**：`balance(recipient) + received ≤ BALANCE_CAP`。
- **TX-TRANSFER-07**：`recipient` 在区间 `(h - INBOUND_WINDOW, h]` 内通过转账收到的 `received` 之和，加上本次的 `received`，≤ `INBOUND_CAP`。

计算：

```
loss     = max(TRANSFER_MIN_LOSS, ⌊amount × TRANSFER_LOSS_PERCENT / 100⌋)
received = amount - loss
```

效果：
- `loss` 被销毁。
- 为 `recipient` 创建 UTXO：`source = Received`，`acquired_height = spendable_from = h`，`expire_height = h + EXPIRY_BLOCKS`，`hops = consumed_hops + 1`，`renewals = consumed_renewals`。
- 找零按 03 §4 处理。

`hops` 达到 `MAX_HOPS` 的额度不能再转出（TX-TRANSFER-03），但仍然可以用于存证。

## 5. 余额

```
balance(a) = Σ 属于 a 的所有 UTXO 的 amount（含未成熟的）
           + Σ a 名下未结束会话的 locked
```

`BALANCE_CAP` 在两个地方生效：Coinbase 发放（§3）和转账接收（TX-TRANSFER-06）。其他交易只会减少或保持余额，不需要检查。

## 6. 作废

在高度 `h` 的区块中，状态处理的第一步是：

1. 找出所有 `expire_height = h` 的 UTXO，按 OutPoint 顺序（03 §2）升序排列。
2. 为每个 UTXO 生成一条作废记录：

```
ExpireRecord = {
  outpoint:        OutPoint
  owner:           PubKey
  amount:          Credit
  source:          u8        // Utxo.source 的标签
  acquired_height: Height
  expire_height:   Height
  hops:            u8
  renewals:        u8
}
```

3. 从 UTXO 集合中移除这些 UTXO。
4. `Expire.count` = 记录条数，`Expire.records_root = MTH([ExpireRecord 编码, …])`。

节点必须保存全部作废记录并建立索引（按地址、按高度），供查询接口使用。任何人都可以根据 `records_root` 校验节点返回的记录是否完整、是否被篡改。

## 7. 额度流转汇总

| 事件 | 新 UTXO 的 `source` | 有效期 | `hops` | `renewals` |
|---|---|---|---|---|
| 挖矿 | `Mined` | 成熟后 `EXPIRY_BLOCKS` | 0 | 0 |
| 接收转账 | `Received` | 从当前高度起 `EXPIRY_BLOCKS` | 被消耗部分的最大值 + 1 | 被消耗部分的最大值 |
| 找零 | 继承原输入 | 继承原输入 | 继承原输入 | 继承原输入 |
| 会话完成后释放 | `Released` | 从当前高度起 `EXPIRY_BLOCKS` | 会话的 `max_hops` | 会话的 `max_renewals` + 1 |
| 会话取消或超时后恢复 | `Cancelled` | 从当前高度起 `EXPIRY_BLOCKS` | 会话的 `max_hops` | 会话的 `max_renewals` + 1 |

`hops` 和 `renewals` 在锁定、恢复和转账的过程中**都不会被重置**，因此无法通过"开始写入再取消"来清空转账次数，也无法通过转账来清空续期次数。

`renewals` 达到 `MAX_RENEWALS` 的额度不能再被锁定到会话中（CRS-BEGIN-06、CRS-ADDLOCK-04），但仍然可以用于登记身份、本人验证，或在 `hops` 允许时转出。
