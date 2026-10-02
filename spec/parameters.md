# 协议参数

本文件是所有协议参数的**唯一数据来源**。实现中的常量应当集中定义，并与本表一一对应。修改其中任何一个值都属于共识变更。

## 共识与区块

| 参数 | 值 | 说明 |
|---|---|---|
| `TARGET_BLOCK_SPACING` | 60 | 秒 |
| `RETARGET_INTERVAL` | 60 | 区块 |
| `MAX_RETARGET_FACTOR` | 4 | |
| `POW_LIMIT` | 2²³⁶ − 1 | 最低难度：区块 ID 至少有 20 个前导零比特 |
| `POW_LIMIT_REGTEST` | 2²⁵³ − 1 | 仅用于回归测试网（平均约 8 次哈希出一个块） |
| `MTP_WINDOW` | 11 | 区块 |
| `TIMEWARP_GRACE` | 600 | 秒 |
| `MAX_FUTURE_DRIFT` | 600 | 秒，非共识接收规则 |
| `MAX_BLOCK_SIZE` | 1 048 576 | 字节 |
| `MAX_BLOCK_TXS` | 8 192 | |
| `MAX_TX_SIZE` | 131 072 | 字节 |
| `MAX_USER_TXS_PER_SENDER_PER_BLOCK` | 16 | |
| `MAX_COINBASE_EXTRA` | 64 | 字节 |
| `MAX_INPUTS` | 64 | |

## 额度

| 参数 | 值 | 说明 |
|---|---|---|
| `BLOCK_REWARD` | 10 | |
| `COINBASE_MATURITY` | 10 | 区块 |
| `EXPIRY_BLOCKS` | 1 440 | 区块，约 24 小时 |
| `BALANCE_CAP` | 500 | |
| `TRANSFER_LOSS_PERCENT` | 10 | |
| `TRANSFER_MIN_LOSS` | 1 | |
| `MAX_HOPS` | 3 | |
| `MAX_RENEWALS` | 2 | 因会话结束（完成、取消或超时）而重新计时的最多次数 |
| `NO_RETURN_WINDOW` | 1 440 | 区块 |
| `INBOUND_WINDOW` | 1 440 | 区块 |
| `INBOUND_CAP` | 200 | |

## 费用

| 参数 | 值 |
|---|---|
| `COST_IDENTITY` | 20 |
| `COST_COURSE_BASE` | 30 |
| `COST_UPDATE_BASE` | 15 |
| `COST_PER_FILE` | 1 |
| `COST_PER_GIB` | 5 |
| `GIB` | 1 073 741 824（2³⁰ 字节） |
| `COST_VERIFY_AUTHOR` | 5 |

## 课程会话

| 参数 | 值 | 说明 |
|---|---|---|
| `LOCK_HEADROOM_PERCENT` | 20 | |
| `SESSION_MAX_BLOCKS` | 10 080 | 区块，约 7 天 |
| `SESSION_IDLE_BLOCKS` | 1 440 | 区块，约 24 小时 |
| `CANCEL_FEE_PERCENT` | 5 | |
| `CANCEL_MIN_FEE` | 2 | |
| `CANCEL_LIMIT` | 1 | |
| `CANCEL_LIMIT_WINDOW` | 1 440 | 区块 |
| `MAX_FILES_PER_COURSE` | 10 000 | |
| `MAX_FILES_PER_APPEND` | 256 | |
| `MAX_PATH_BYTES` | 512 | |
| `MAX_TITLE_BYTES` | 256 | |
| `MAX_VERSION_BYTES` | 32 | |

## 零知识身份

| 参数 | 值 | 说明 |
|---|---|---|
| `MAX_PROOF_SIZE` | 8 192 | 字节 |
| `IDENTITY_CIRCUIT_K` | 9 | Halo2 电路域指数，电路配置固定 |
| `CIRCUIT_FIELD_MODULUS` | `0x40000000000000000000000000000000224698fc094cf91b992d30ed00000001` | Pallas 基域的模数 |

## 创世区块

| 网络 | 时间戳 | nonce | extra | block_id |
|---|---|---|---|---|
| 主网 | 待定 | 待定 | 待定 | 待定 |
| 测试网 | 待定 | 待定 | 待定 | 待定 |
| 开发网 | 1790553600 | 50481 | `YuanLai devnet genesis 2026-09-28` | `000005c5ff70ee48c744e8068595ef7ca64c7ced561d7364256dfd5bf833cc47` |
| 回归测试网 | 1790553600 | 7 | `YuanLai regtest genesis 2026-09-28` | `1ba9d5d6fe9ae65117c90d0eb147885b435098974884d83ca7372ec961737d70` |

完整编码见 [test-vectors/genesis.json](test-vectors/genesis.json)。
