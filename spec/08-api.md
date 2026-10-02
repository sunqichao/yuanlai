# 08 只读 HTTP 查询接口

**状态：草案**。本接口是节点管理与查看入口，不属于共识协议；接口字段可以独立于区块格式演进。

## 1. 监听与安全

节点使用 `--api-listen ADDR` 启动 HTTP 服务。只指定该参数时，节点以前台 API 服务运行；与持续运行的 P2P listener 或挖矿模式组合时，API 在独立线程中服务。

API 不提供认证、TLS、写入操作或速率限制。节点操作者应将其绑定到 loopback 地址，例如 `127.0.0.1:18445`。不得直接暴露到不可信网络。服务按顺序处理连接；请求行最多 2048 字节、headers 总计最多 8192 字节且最多 32 项，读写超时为 5 秒。

API 从 datadir 的 `blocks.dat` 读取活动链记录，并以网络的创世块为起点重放和验证记录。文件长度或修改时间变化时重新生成快照；文件未变化时复用已验证的 JSON 响应。存储文件无效或无法读取时，查询返回 `503 Service Unavailable`，不会返回未经验证的链状态。

## 2. 路由

### `GET /`

返回内嵌的 HTML 项目总览页面。页面从同源 `/api/overview` 加载数据并定时刷新。

### `GET /api/overview`

返回 `application/json` 的当前活动链快照。所有可能超过 JavaScript 安全整数范围的数值都编码为十进制字符串，消费方不得将这些字段先转换为二进制浮点数。

响应对象包含：

| 字段 | 类型 | 含义 |
|---|---|---|
| `network` | string | 网络名称 |
| `chain_id` | string | 网络 chain ID 的十六进制形式 |
| `height` | decimal string | 活动链 tip 高度；创世高度为 0 |
| `tip` | string | tip 区块 ID 的十六进制形式 |
| `tip_timestamp` | decimal string | tip 时间戳，Unix 秒 |
| `target` | hex string | tip 的无符号目标值，固定宽度十六进制表示 |
| `block_count` | decimal string | 活动链区块总数，包含创世块 |
| `transaction_count` | decimal string | 活动链记录中的交易总数 |
| `credit` | object | Credit 汇总，数量单位与协议 `Credit` 一致 |
| `recent_blocks` | array | 最新优先，最多 16 个区块，包含创世块 |

`credit` 包含十进制字符串字段 `minted`、`burned`、`expired` 和 `unspent`。`unspent` 仅统计当前 UTXO 中的 Credit，不包含课程会话锁定额度。

`recent_blocks` 的每个对象包含 `height`、`timestamp`、`transactions` 三个十进制字符串字段，以及十六进制 `id` 和固定宽度十六进制 `target` 字符串。`transaction_count` 和区块内交易计数均包含创世块。

只接受无请求体的 GET 请求。其他方法或带请求体的请求返回 `405 Method Not Allowed`；未知路径返回 `404 Not Found`；格式错误或超过限制的请求返回 `400 Bad Request`。