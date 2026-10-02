# 06 零知识身份

本章定义身份承诺和本人验证证明。所有域元素使用 Pallas 基域 `Fp`，模数见
[parameters.md](parameters.md)。域元素编码为 32 字节小端序，解码必须拒绝大于或等于模数的值。

## 1. 身份承诺

作者在本地生成两个独立、均匀随机的 `Fp` 元素 `secret` 与 `salt`。二者都是私密身份材料，必须保密并备份；不得从公开身份信息或 Ed25519 密钥直接导出。

使用 Poseidon 参数 `P128Pow5T3`（宽度 3、速率 2、S-box 指数 5、128 位安全级别）和固定长度输入模式计算：

```
identity_commitment = Poseidon< P128Pow5T3, ConstantLength<2> >([secret, salt])
```

输出是一个 `Fp` 元素，以规范的 32 字节小端序编码为 `RegisterIdentity.commitment`。固定输入长度属于哈希域分离的一部分；实现不得把输入改为可变长度模式。

## 2. 哈希公开输入编码

`challenge` 和 `course_root` 各是 32 字节 `Hash`，其字节串不是按域元素解码。为将其无损映射到电路域，每个哈希按字节下标 `0..16` 与 `16..32` 拆成两个 16 字节小端无符号整数，分别零扩展至 32 字节后解码为 `Fp`：

```
challenge_lo = Fp::from_bytes_le(zero_extend(challenge[0..16]))
challenge_hi = Fp::from_bytes_le(zero_extend(challenge[16..32]))
course_lo    = Fp::from_bytes_le(zero_extend(course_root[0..16]))
course_hi    = Fp::from_bytes_le(zero_extend(course_root[16..32]))
```

每个分量最多 128 位，因此必为规范域元素。不得调换分量次序或改变字节序。

## 3. 本人验证电路

作者以 `secret`、`salt` 为私有见证。证明的公开实例按下列顺序排列：

```
[identity_commitment, challenge_lo, challenge_hi, course_lo, course_hi, nullifier]
```

电路必须约束：

1. `identity_commitment` 等于 §1 中计算的固定长度 Poseidon 输出。
2. `nullifier` 等于以下固定长度 Poseidon 输出：

```
nullifier = Poseidon< P128Pow5T3, ConstantLength<5> >(
    [secret, challenge_lo, challenge_hi, course_lo, course_hi]
)
```

3. `secret`、`salt` 是 `Fp` 域元素。零值是有效输入；实现不得添加未在本章规定的秘密值限制。

交易中的 `challenge`、`course_root` 按 §2 分解后必须与公开实例一致。公开实例中的承诺和 nullifier 使用规范 32 字节小端域元素编码。一个 proof 只对其公开实例中指定的承诺、challenge、课程根和 nullifier 有效。

## 4. 证明系统与验证

证明系统为 Halo2 PLONK，使用 Pasta/Pallas 原生域与内积论证多项式承诺，不需要可信设置。电路域指数固定为 `IDENTITY_CIRCUIT_K`；证明参数由 Halo2 的确定性透明参数生成方法产生，证明键由本章固定电路唯一导出。Transcript 固定为 Halo2 `Blake2bWrite`/`Blake2bRead` 与 `Challenge255`。Proof 字段内容是 transcript 产生的原始字节串，不得附加字段内长度前缀；交易中的 `VarBytes` 编码仍按 03 §2 具有唯一的 `u32` 长度前缀。字段字节长度不得超过 `MAX_PROOF_SIZE`。

验证失败、非规范域元素、错误的公开实例数量或超长 proof 均视为无效证明。共识验证器必须使用与证明者相同的电路和公开实例顺序；不得依赖本机时钟、随机数或非确定性参数。

## 5. 隐私与重放

`secret`、`salt` 及其身份信息不得作为公开实例或 proof 附加数据。承诺隐藏身份材料的前提是 `secret` 和 `salt` 具有足够熵且未泄露。

nullifier 将身份秘密、验证方 challenge 和课程根绑定在一起。挑战由验证方生成；链上拒绝重复 nullifier。验证方应检查链上本人验证记录中的 challenge 与自己发出的值完全一致。