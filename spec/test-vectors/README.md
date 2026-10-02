# 测试向量

测试向量是本协议的**可执行规范**。任何实现都应当通过全部测试向量，以此证明与规范一致。

## 来源

所有测试向量都由 [tools/testvectors/generate.py](../../tools/testvectors/generate.py) 生成。这是一份**独立的参考实现**：只使用 Python 标准库，直接按规范编写，不与 Rust 实现共享任何代码。

```sh
python3 tools/testvectors/generate.py          # 重新生成
python3 tools/testvectors/generate.py --check  # 校验仓库中的文件是否最新（CI 使用）
```

Rust 实现由 `crates/yuanlai-conformance` 逐条校验：`cargo test -p yuanlai-conformance`。

## 文件

| 文件 | 内容 | 状态 |
|---|---|---|
| `codec.json` | 各类型的合法编码与非法编码，非法时给出规则编号 | P0 ✓ |
| `tagged-hash.json` | 全部标签的带标签哈希；各网络的 chain_id | P0 ✓ |
| `merkle.json` | Merkle 树（0 至 17 个叶子） | P0 ✓ |
| `block-header.json` | 区块头编码、block_id、PoW 校验、`work(target)` | P0 ✓ |
| `difficulty.json` | 难度调整（含边界与钳制）、中位时间 | P0 ✓ |
| `genesis.json` | 开发网与回归测试网的创世区块（完整编码与 ID） | P1 ✓ |
| `tx.json` | 用户交易编码（含 proof 明确标注为 wire-only 的 VerifyAuthor）、`sig_msg`、RFC 8032 确定性签名、txid；转账损耗；域校验；官方 Poseidon 承诺向量 | P4 ✓ |
| `state-transition/` | 状态转换场景：初始状态 + 区块 → 结果状态或错误的规则编号 | P1 至 P3 |

## 格式约定

- 所有字节数据都用小写十六进制字符串表示，不带 `0x` 前缀；256 位整数用 64 位十六进制表示（大端序）。
- 预期失败的向量有 `error` 字段，值为规则编号（例如 `"CODEC-DEC-01"`）；合法输入的 `error` 为 `null`。
- 测试向量一旦发布，只能新增，不能修改。修改生成器导致已有向量变化时，必须经过 YIP。
