# ADR 0001：基础技术选型

- 状态：已接受
- 日期：2026-09-28

## 背景

猿来计划完全开源，要求所有代码经得起公开推敲。需要确定实现语言、许可证、零知识方案和代码分层方式。

## 决策

1. **实现语言：Rust。** 内存安全；类型系统可以表达 `Credit`、`Height` 等领域类型；clippy 可以在编译期强制执行溢出检查和禁用 panic；同一份代码可以编译成 WASM 供浏览器使用。
2. **许可证：MIT OR Apache-2.0 双许可；规范文档另以 CC BY 4.0 授权。** 这是 Rust 生态的惯例，Apache-2.0 提供专利授权。依赖的许可证由 `deny.toml` 中的白名单约束。
3. **零知识方案：halo2，不使用需要可信设置的方案。** 理由见 `spec/rationale.md`。
4. **共识核心层**（`yuanlai-codec`、`yuanlai-crypto`、`yuanlai-types`、`yuanlai-zk`、`yuanlai-consensus`）：
   - 不做 I/O，不使用异步运行时，不读取系统时间，不使用随机数；
   - 禁止使用 `HashMap` / `HashSet`；
   - 由 workspace 统一的 lint 禁止溢出算术、截断转换、浮点运算、`unwrap`、`expect`、`panic!` 和下标越界访问。

   这些约束由 `tools/check-core-deps.sh` 和各 crate 的 `clippy.toml` 在 CI 中强制执行。
5. **共识编码自行实现（`yuanlai-codec`），不使用 serde。** 共识编码必须逐字节可控、有且只有一种合法编码；通用序列化框架的行为会随版本和配置变化。
6. **P2P 不使用 libp2p。** libp2p 功能多、代码量大，大部分功能用不上，却会显著扩大攻击面和审计范围。改用 tokio 加自研的帧协议。

## 后果

- 共识核心层可以单独审计，也可以原样编译成 WASM。
- 严格的 lint 会让代码更啰嗦（例如处处 `checked_add`），这是有意为之的代价。
- 每新增一个外部依赖，都需要新写一份 ADR 说明理由。
