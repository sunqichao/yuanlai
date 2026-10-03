# 猿来 YuanLai

[English](README.en.md)

> **当前方向：本机优先的文件指纹登记。** 新版产品仍处于原型阶段：可在本机计算文件 SHA-256、签署声明并保存/核验记录；新版区块链、联网同步和公开浏览器尚未实现。

## 当前产品

猿来用于生成可携带的文件指纹与声明签名记录。文件原文留在用户设备；当前网页原型通过本机 Ed25519 密钥签署登记声明，记录保存在回环服务管理的本机 JSON 文件中。登记、导出和核验可离线完成；同一台电脑上使用相同本机地址的浏览器可查看共享记录。

本机签名记录不等于链上记录。设备时间不是可信时间戳；签名只表明对应密钥签署了记录，SHA-256 只比较文件字节，均不证明身份、原创性或法律权利。原型未接入互联网同步，也不会自动公开记录。

## 运行本机原型

需要 Python 3。在仓库根目录运行：

```sh
python3 web/v2-prototype/run_local.py
```

打开 <http://127.0.0.1:18765/>。完成登记后，顶部“本机浏览器”可搜索作品/声明/记录编号/指纹，按 V1/V2 筛选，查看签名详情、导出 JSON，或选择原文件核验。原型文件上限为 500 MiB；当前整块读入内存计算，大文件可能带来内存压力。服务只绑定 `127.0.0.1`。密钥管理和限制见[原型说明](web/v2-prototype/README.md)。

## 路线图与设计

- [新版产品方案](docs/new-product-proposal.md)
- [新版威胁模型](docs/new-threat-model.md)
- [项目跟踪计划](docs/project-tracking.md)
- [V2 协议草案](spec/v2/README.md)，尚未冻结或实现
- [新版区块链浏览器设计](docs/blockchain-explorer-design.md)，仅为未来共享网络方案

<details>
<summary>旧版 PoW/Credit 操作指南（Legacy，不代表当前产品方向）</summary>

**旧版实现**是一条为课程登记设计的 PoW/Credit 区块链，包含 Halo2 身份证明。该实现保留在仓库中供历史参考和独立开发；它不是新版文件指纹登记服务或新版区块链。

> **状态：P5 开发中（pre-alpha）。** P4 身份证明已完成；P5 已有原子重组、限长 P2P 帧、TCP 同步、最多 8 peer 并行服务及实验性交易接纳/转发。网络 relay 与挖矿仍需分开运行；待处理交易会写入 `mempool.dat` 并在启动时重新验证。尚未达到公开网络运行要求，不可用于实际存证。

完整的节点启动、P2P 同步、交易 relay、CLI 存证和故障排查步骤见[使用与操作指南](docs/node-operations.md)。

启动只读项目总览服务，并在浏览器打开 `http://127.0.0.1:18445`：

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-data --api-listen 127.0.0.1:18445
```

页面直接读取并验证该 datadir 的活动链；API-only 模式会持续运行。HTTP API 无认证或 TLS，只应绑定本机地址；与 `--listen` 或持续挖矿并用时，可在节点运行期间查看实时状态。接口字段见 [spec/08-api.md](spec/08-api.md)。

```sh
cargo run --release -p yuanlai-node -- --network devnet --mine --beneficiary <32 字节公钥的十六进制>
```

regtest 下启动一个 peer 服务端，并从另一个 peer 一次性同步活动链：

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-a --listen 127.0.0.1:18444
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-b --connect 127.0.0.1:18444
```

`--listen` 会并行服务最多 8 个入站 peer。可重复指定 `--connect`，节点按顺序尝试静态 peer；至少一个 peer 成功同步后退出，`--tx-file` 会提交给每个成功连接的 peer。加上 `--stay-connected` 后，CLI 地址会合并保存到 datadir 的 `peers.txt`，重启时可只传 `--stay-connected` 恢复配置。每个 peer 独立记录失败退避与最近成功时间，失败时按 1–30 秒指数退避，成功后 30 秒再同步。peer 仍按顺序短连接，不保持 TCP socket 常开，也不做并行同步或自动发现。网络模式不能与 `--mine` 同时使用；协议和节点尚未做公网压力/安全审计，不要暴露到不可信网络。

可选交易转发模式：listener 通过 `--beneficiary` 指定 mempool 校验所用的候选区块受益人；client 同步后用 `--tx-file` 提交交易并等待 ACK。listener 会向其他已连接 peer 转发新接纳的交易，并将待处理交易原子写入 `mempool.dat`：

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./relay-node \
	--listen 127.0.0.1:18444 --beneficiary <32 字节公钥的十六进制>
cargo run --release -p yuanlai-node -- --network regtest --datadir ./sender-node \
	--connect 127.0.0.1:18444 --tx-file ./signed-tx.bin
```

重复交易会得到已接纳 ACK，但不会再次转发。relay 与挖矿目前不能同时运行；停止 relay 后，可使用相同 datadir 和对应受益人启动 `--mine`，节点会重新验证并加载待处理交易。网络功能仍属实验性，不要暴露到不可信网络。

扫描目录并生成本地清单（不会提交到区块链）：

```sh
cargo run --release -p yuanlai-cli -- scan ./my-course
```

首次登记前，先为 Ed25519 公钥登记身份承诺。可传入已计算的承诺：

```sh
cargo run --release -p yuanlai-cli -- identity-register \
	--network regtest --datadir ./node-data \
	--commitment <64 位小端十六进制 Pallas 基域承诺> \
	--key-file ./owner.key --out-dir ./identity-transactions
```

也可由 CLI 从独立的身份秘密和 salt 文件计算承诺并登记：

```sh
cargo run --release -p yuanlai-cli -- identity-register \
	--network regtest --datadir ./node-data \
	--identity-secret-file ./identity.secret --salt-file ./identity.salt \
	--key-file ./owner.key --out-dir ./identity-transactions
```

`identity.secret` 与 `identity.salt` 必须是独立、均匀随机的规范 Pallas 基域元素，各为 32 字节原始小端编码；两者和 `owner.key` 都必须仅允许文件所有者访问（`chmod 600`）。身份秘密与 Ed25519 密钥必须独立，并须安全备份。命令会选择当前可花费 Credit，生成签名交易文件并打印提交命令。先执行该命令打印的提交命令，再构造课程登记交易：

```sh
cargo run --release -p yuanlai-cli -- register \
	--network regtest --datadir ./node-data \
	--course-dir ./my-course --title '课程标题' --version 1.0 \
	--key-file ./owner.key --out-dir ./course-transactions
```

`owner.key` 必须是 32 字节 Ed25519 私钥原始字节；macOS/Linux 上须先执行 `chmod 600 ./owner.key`。课程登记命令会检查链上身份和可花费额度、生成签名交易文件，并打印逐组提交命令；按顺序执行输出的命令即可挖入登记交易。

登记新版本时，添加 `--parent-root <最新版本的 64 位十六进制根哈希>`。新版目录仍须包含完整文件快照，费用只按相对父版本新增或变化的内容计算。

课程登记上链后，可用验证方提供的 challenge 生成并提交本人验证交易。验证方会按 challenge 与课程根在链上的 nullifier 记录确认结果；该交易消耗 5 Credit：

```sh
cargo run --release -p yuanlai-cli -- author-verify \
	--network regtest --datadir ./node-data \
	--course-root <课程根的 64 位十六进制哈希> \
	--challenge <验证方提供的 64 位十六进制 challenge> \
	--identity-secret-file ./identity.secret --salt-file ./identity.salt \
	--key-file ./owner.key --out-dir ./author-verifications
```

CLI 会确认身份承诺与课程根均属于该密钥，生成 Halo2 proof 和签名交易文件，并打印提交命令。challenge 必须来自验证方，不应重复使用。

已签名的规范编码用户交易可随挖矿请求一起提交；节点会先按当前链状态验证交易，再将其放入下一个区块：

```sh
cargo run --release -p yuanlai-node -- --network regtest --datadir ./node-data \
	--tx-file ./signed-tx.bin --mine --beneficiary <32 字节公钥的十六进制> --blocks 1
```

## 非金融声明

- 猿来**不发行代币**，也不提供任何交易、兑换或定价服务。
- 存证额度（Credit）只能用于存证，**获得后 24 小时内不使用即作废**，不具备保值和流通属性。
- 协议中不存在任何价格字段。禁止将额度用于有偿交易。
- 猿来**不收集个人信息**。身份信息只在用户本地参与零知识证明计算。

## 能做什么，不能做什么

| 能 | 不能 |
|---|---|
| 证明某份内容在某个区块高度之前已被登记 | 证明内容是**原创**（只能证明时间在先） |
| 证明登记人持有登记时的身份秘密（零知识，不泄露信息） | 识别转码、剪辑后的相似内容 |
| 追溯课程的版本演进 | 替代国家版权登记或公证 |

请务必在**课程公开之前**完成登记。完整的局限说明见 [spec/threat-model.md](spec/threat-model.md)。

## 核心机制

- **共识**：SHA-256 工作量证明，目标出块 60 秒，每 60 个区块调整一次难度，设有难度下限。
- **存证额度**：挖矿获得，登记课程时销毁；可以转给他人，但每次转账损耗 10%、最多转 3 次，并且都有 24 小时有效期。
- **课程写入会话**：写入期间额度锁定、暂停计时；取消后恢复。
- **零知识身份**：基于 halo2，**不需要可信设置**。
- **本人验证**：证明绑定身份承诺、验证方 challenge 和课程根；nullifier 防止重复使用。

详见 [spec/](spec/README.md)。

## 仓库结构

```
spec/      协议规范（规范性文本，代码以此为准）
crates/    Rust 实现
tools/     开发与校验脚本
docs/      架构决策记录（ADR）与使用文档
yips/      猿来改进提案（YIP）
```

</details>

## 构建与检查

需要 Rust（版本由 `rust-toolchain.toml` 锁定，rustup 会自动安装）。

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
./tools/check-core-deps.sh
python3 tools/testvectors/generate.py --check
```

## 参与贡献

请先阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。安全问题请按 [SECURITY.md](SECURITY.md) **私下报告**，不要公开提交 issue。

## 许可证

Rust 代码及其他未另行标注的源文件采用以下任一许可证授权，由你选择：

- Apache License 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT License（[LICENSE-MIT](LICENSE-MIT)）

仅 `spec/` 下协议文本、说明和测试向量另外以 CC BY 4.0 授权。范围、例外和完整法律文本入口见 [spec/LICENSE-CC-BY-4.0.md](spec/LICENSE-CC-BY-4.0.md)；其他文档、代码、网页素材和商标不因此适用 CC BY 4.0。

除非你明确声明，否则你有意提交并被纳入本项目的任何贡献（按 Apache-2.0 许可证的定义），都将按上述双许可证授权，不附加任何额外条款或条件。
