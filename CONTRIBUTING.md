# 参与贡献

感谢你对猿来的关注。本项目以"每一行代码都经得起推敲"为目标，因此对贡献有较严格的要求。

## 基本原则

1. **规范先于代码**：凡是涉及共识的改动，必须先修改 `spec/` 中的对应规则，再修改代码。代码和规范不一致时，以规范为准，并视为缺陷。
2. **规则可追溯**：每条共识规则都有编号（例如 `TX-TRANSFER-04`）。实现、错误枚举和测试都要引用该编号。
3. **不增加信任假设**：不引入需要可信设置的密码学方案，不引入中心化服务。

## 共识变更

改变区块或交易有效性的任何修改，都属于共识变更，必须走 [YIP 流程](GOVERNANCE.md#猿来改进提案yip)，不接受直接提交 PR。

## 提交 PR 的要求

- [ ] `cargo fmt --all --check` 通过
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过
- [ ] `cargo test --workspace --locked` 通过
- [ ] `./tools/check-core-deps.sh` 通过
- [ ] `python3 tools/testvectors/generate.py --check` 通过
- [ ] 新增或修改的规则都有"通过"和"拒绝"两类测试用例
- [ ] 涉及编码、哈希或状态转换的修改，已在 `tools/testvectors/generate.py`（独立参考实现）中同步实现并重新生成测试向量
- [ ] 新增依赖已在 `docs/adr/` 中说明理由，许可证在 `deny.toml` 的白名单内
- [ ] 每个提交都有 DCO 签名（`git commit -s`）

## 共识核心层的额外规则

以下 crate 属于共识核心层：`yuanlai-codec`、`yuanlai-crypto`、`yuanlai-types`、`yuanlai-zk`、`yuanlai-consensus`。

- 禁止 I/O、异步运行时、读取系统时间和使用随机数（证明生成所需的随机源由调用方传入）
- 禁止使用 `HashMap` / `HashSet`，改用 `BTreeMap` / `BTreeSet`
- 所有算术运算都使用 `checked_*`，禁止 `as` 截断转换和浮点运算
- 禁止 `unwrap`、`expect`、`panic!` 和下标越界访问。测试代码可以在文件开头用带 `reason` 的 `#![allow(...)]` 放宽，生产代码不得使用
- 理论上可能 panic 的标准库函数（如 `clamp`、整数除法）只能在 panic 条件被类型或编译期断言排除时使用，例如除数用 `NonZero*` 类型
- 这些 crate 下的 `clippy.toml` 必须与 `tools/core-clippy.toml` 完全一致（修改时同步复制，CI 会检查）

## 提交信息

遵循 [Conventional Commits](https://www.conventionalcommits.org/)，例如：

```
feat(consensus): implement TX-TRANSFER-04 recipient identity check
fix(codec): reject non-minimal length prefix
docs(spec): clarify expiry height semantics
```

## 开发者原创声明（DCO）

提交即表示你同意 [Developer Certificate of Origin 1.1](https://developercertificate.org/)：你有权以本项目的许可证提交这些代码。请使用 `git commit -s` 为每个提交添加 `Signed-off-by` 行。

## 行为准则

参与本项目即表示你同意遵守 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。
