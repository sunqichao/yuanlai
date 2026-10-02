# 猿来协议规范

**协议版本：0（草案）**：本目录中的所有内容都可能变更，直到发布 v0.1.0。

本目录是猿来协议的**规范性文本**。任何实现（包括本仓库的 Rust 实现）都应以此为准。实现与规范不一致时，视为实现缺陷。

## 文档索引

| 文档 | 内容 | 状态 |
|---|---|---|
| [00-overview.md](00-overview.md) | 总体设计、术语 | 草案 |
| [01-serialization.md](01-serialization.md) | 规范化编码、带标签哈希、Merkle 树 | 草案 |
| [02-block.md](02-block.md) | 区块结构、工作量证明、难度调整、分叉选择 | 草案 |
| [03-transactions.md](03-transactions.md) | 交易格式与通用校验规则 | 草案 |
| [04-credit.md](04-credit.md) | 存证额度：产生、有效期、转账、作废 | 草案 |
| [05-course-session.md](05-course-session.md) | 课程写入会话与课程记录 | 草案 |
| [06-zk-identity.md](06-zk-identity.md) | 零知识身份电路 | 草案 |
| [07-p2p.md](07-p2p.md) | 网络协议 | P5 进行中 |
| [08-api.md](08-api.md) | 只读 HTTP 总览页与 JSON 查询接口 | 草案 |
| [parameters.md](parameters.md) | 所有协议参数（唯一数据来源） | 草案 |
| [invariants.md](invariants.md) | 系统不变量 | 草案 |
| [rationale.md](rationale.md) | 设计理由 | 草案 |
| [threat-model.md](threat-model.md) | 威胁模型与已知局限 | 草案 |
| [test-vectors/](test-vectors/README.md) | 官方测试向量 | 框架 |

## 约定

### 规范用语

| 用语 | 含义 |
|---|---|
| **必须** | 绝对要求（对应 RFC 2119 的 MUST） |
| **不得** | 绝对禁止（MUST NOT） |
| **应当** | 推荐做法，有充分理由时可以不遵守（SHOULD） |
| **可以** | 可选（MAY） |

### 规则编号

每条共识规则都有唯一编号，格式为 `<范围>-<主题>-<序号>`，例如 `TX-TRANSFER-04`。

- 编号一经发布**不得**重新使用；规则被废止时标注"已废止"，编号保留。
- 实现中的错误类型、测试用例都应当引用规则编号。

### 参数

规范正文只使用参数名（例如 `EXPIRY_BLOCKS`），具体数值只在 [parameters.md](parameters.md) 中定义。

### 算术

除非另有说明：
- 所有数值都是非负整数，**不存在浮点数**。
- 除法一律**向下取整**，记作 `⌊a / b⌋`；向上取整记作 `⌈a / b⌉`。
- 任何中间结果溢出其类型，都视为校验失败，而不是回绕。

## 许可

本目录内未另行标注的规范文本、说明与测试向量以 CC BY 4.0 授权。适用范围和完整法律文本入口见 [LICENSE-CC-BY-4.0.md](LICENSE-CC-BY-4.0.md)；此授权不扩展到仓库其他目录。
