#!/usr/bin/env python3
"""Independent reference implementation that generates YuanLai test vectors.

Written directly from spec/ using only the Python standard library; it shares no code with the
Rust implementation, so agreement between the two is meaningful evidence of correctness.

    python3 tools/testvectors/generate.py          # write spec/test-vectors/*.json
    python3 tools/testvectors/generate.py --check  # fail if the committed files differ
"""

import hashlib
import json
import pathlib
import struct
import sys

OUT_DIR = pathlib.Path(__file__).resolve().parents[2] / "spec" / "test-vectors"

# spec/parameters.md
POW_LIMIT = 2**236 - 1
POW_LIMIT_REGTEST = 2**253 - 1
RETARGET_INTERVAL = 60
TARGET_BLOCK_SPACING = 60
MAX_RETARGET_FACTOR = 4
EXPECTED = RETARGET_INTERVAL * TARGET_BLOCK_SPACING

NETWORKS = {
    "mainnet": ("yuanlai-mainnet", POW_LIMIT),
    "testnet": ("yuanlai-testnet", POW_LIMIT),
    "devnet": ("yuanlai-devnet", POW_LIMIT),
    "regtest": ("yuanlai-regtest", POW_LIMIT_REGTEST),
}

TAGS = [
    "YuanLai/chain-id/v1",
    "YuanLai/block/v1",
    "YuanLai/tx/v1",
    "YuanLai/tx-sig/v1",
    "YuanLai/merkle-empty/v1",
    "YuanLai/merkle-leaf/v1",
    "YuanLai/merkle-node/v1",
]


# --- spec/01-serialization.md -------------------------------------------------------------------

def sha256(data):
    return hashlib.sha256(data).digest()


def tagged_hash(tag, msg):
    t = sha256(tag.encode("ascii"))
    return sha256(t + t + msg)


def merkle_root(leaves):
    n = len(leaves)
    if n == 0:
        return tagged_hash("YuanLai/merkle-empty/v1", b"")
    if n == 1:
        return tagged_hash("YuanLai/merkle-leaf/v1", leaves[0])
    k = 1
    while k * 2 < n:
        k *= 2
    return tagged_hash("YuanLai/merkle-node/v1", merkle_root(leaves[:k]) + merkle_root(leaves[k:]))


def u32(v):
    return struct.pack("<I", v)


def u64(v):
    return struct.pack("<Q", v)


def chain_id(network):
    return tagged_hash("YuanLai/chain-id/v1", NETWORKS[network][0].encode("ascii"))


# --- spec/02-block.md ---------------------------------------------------------------------------

def encode_header(h):
    return (
        u32(h["version"])
        + h["chain_id"]
        + u64(h["height"])
        + h["prev_block"]
        + h["tx_root"]
        + u64(h["timestamp"])
        + h["target"].to_bytes(32, "big")
        + u64(h["nonce"])
    )


def block_id(h):
    return tagged_hash("YuanLai/block/v1", encode_header(h))


# --- spec/03-transactions.md, spec/02-block.md §6 --------------------------------------------------

def encode_coinbase(height, beneficiary, extra):
    return b"\x00" + u64(height) + beneficiary + u32(len(extra)) + extra


def txid(tx_bytes):
    return tagged_hash("YuanLai/tx/v1", tx_bytes)


# --- RFC 8032 Ed25519, written from the RFC's reference description --------------------------------

ED_P = 2**255 - 19
ED_Q = 2**252 + 27742317777372353535851937790883648493
ED_D = -121665 * pow(121666, ED_P - 2, ED_P) % ED_P
ED_SQRT_M1 = pow(2, (ED_P - 1) // 4, ED_P)


def _ed_add(a, b):
    x1, y1, z1, t1 = a
    x2, y2, z2, t2 = b
    ea = (y1 - x1) * (y2 - x2) % ED_P
    eb = (y1 + x1) * (y2 + x2) % ED_P
    ec = 2 * t1 * t2 * ED_D % ED_P
    ed = 2 * z1 * z2 % ED_P
    e, f, g, h = eb - ea, ed - ec, ed + ec, eb + ea
    return (e * f % ED_P, g * h % ED_P, f * g % ED_P, e * h % ED_P)


def _ed_mul(s, point):
    acc = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            acc = _ed_add(acc, point)
        point = _ed_add(point, point)
        s >>= 1
    return acc


def _ed_recover_x(y, sign):
    x2 = (y * y - 1) * pow(ED_D * y * y + 1, ED_P - 2, ED_P) % ED_P
    x = pow(x2, (ED_P + 3) // 8, ED_P)
    if (x * x - x2) % ED_P != 0:
        x = x * ED_SQRT_M1 % ED_P
    if (x & 1) != sign:
        x = ED_P - x
    return x


_GY = 4 * pow(5, ED_P - 2, ED_P) % ED_P
_GX = _ed_recover_x(_GY, 0)
ED_G = (_GX, _GY, 1, _GX * _GY % ED_P)


def _ed_compress(point):
    zinv = pow(point[2], ED_P - 2, ED_P)
    x, y = point[0] * zinv % ED_P, point[1] * zinv % ED_P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _ed_expand(secret):
    h = hashlib.sha512(secret).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def ed25519_public(secret):
    return _ed_compress(_ed_mul(_ed_expand(secret)[0], ED_G))


def ed25519_sign(secret, msg):
    a, prefix = _ed_expand(secret)
    pub = _ed_compress(_ed_mul(a, ED_G))
    r = int.from_bytes(hashlib.sha512(prefix + msg).digest(), "little") % ED_Q
    big_r = _ed_compress(_ed_mul(r, ED_G))
    k = int.from_bytes(hashlib.sha512(big_r + pub + msg).digest(), "little") % ED_Q
    return big_r + ((r + k * a) % ED_Q).to_bytes(32, "little")


# RFC 8032 §7.1 test 1; the generator refuses to run if its Ed25519 is wrong.
assert ed25519_public(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")) == bytes.fromhex(
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
)
assert ed25519_sign(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"), b"") == bytes.fromhex(
    "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
)


# --- spec/03-transactions.md user transactions -------------------------------------------------------

CIRCUIT_FIELD_MODULUS = 0x40000000000000000000000000000000224698fc094cf91b992d30ed00000001


def encode_outpoints(outpoints):
    return u32(len(outpoints)) + b"".join(t + u32(i) for t, i in outpoints)


def signed_user_tx(tag, secret, nonce, body, network):
    sender = ed25519_public(secret)
    unsigned = bytes([tag]) + sender + u64(nonce) + body
    msg = tagged_hash("YuanLai/tx-sig/v1", chain_id(network) + unsigned)
    sig = ed25519_sign(secret, msg)
    full = unsigned + sig
    return {"sender": hx(sender), "sig_msg": hx(msg), "signature": hx(sig), "encoding": hx(full), "txid": hx(txid(full))}


# Only the genesis parameters that must be chosen; everything else follows from the spec.
GENESIS = {
    "devnet": {"timestamp": 1_790_553_600, "extra": b"YuanLai devnet genesis 2026-09-28"},
    "regtest": {"timestamp": 1_790_553_600, "extra": b"YuanLai regtest genesis 2026-09-28"},
}


def genesis_block(network):
    g = GENESIS[network]
    coinbase = encode_coinbase(0, bytes(32), g["extra"])
    header = {
        "version": 1,
        "chain_id": chain_id(network),
        "height": 0,
        "prev_block": bytes(32),
        "tx_root": merkle_root([txid(coinbase)]),
        "timestamp": g["timestamp"],
        "target": NETWORKS[network][1],
        "nonce": 0,
    }
    header = first_nonce(header, True)
    block = encode_header(header) + u32(1) + coinbase
    return header, block


def meets_target(h):
    return int.from_bytes(block_id(h), "big") <= h["target"]


def work(target):
    return 2**256 // (target + 1)


def median_time_past(timestamps):
    s = sorted(timestamps)
    return s[(len(s) - 1) // 2]


def next_target(next_height, parent_target, pow_limit, ts_first, ts_last):
    """ts_first = ts(h - I - 1), ts_last = ts(h - 1); unused off-boundary."""
    if next_height % RETARGET_INTERVAL != 0 or next_height < 2 * RETARGET_INTERVAL:
        return parent_target
    actual = ts_last - ts_first
    actual = max(EXPECTED // MAX_RETARGET_FACTOR, min(EXPECTED * MAX_RETARGET_FACTOR, actual))
    # Python integers are unbounded, so the direct formula is an independent check of the
    # split formula used by implementations with fixed-width integers.
    nxt = parent_target * actual // EXPECTED
    return max(1, min(pow_limit, nxt))


# --- vector sets --------------------------------------------------------------------------------

def hx(b):
    return b.hex()


def vectors_tagged_hash():
    messages = [b"", b"abc", bytes(64), bytes(range(256))]
    cases = []
    for tag in TAGS:
        for m in messages:
            cases.append({"tag": tag, "message": hx(m), "hash": hx(tagged_hash(tag, m))})
    chain_ids = [{"network": n, "name": NETWORKS[n][0], "chain_id": hx(chain_id(n))} for n in NETWORKS]
    return {"description": "TH(tag, m) and chain_id (spec/01 §4, spec/00 §5)", "cases": cases, "chain_ids": chain_ids}


def vectors_merkle():
    cases = []
    for n in range(0, 18):
        leaves = [bytes([i]) * (i % 5) for i in range(n)]
        cases.append({"leaves": [hx(x) for x in leaves], "root": hx(merkle_root(leaves))})
    return {"description": "MTH over byte-string leaves; leaf i is byte i repeated (i mod 5) times (spec/01 §5)", "cases": cases}


def sample_header(network, nonce=0, target=None):
    return {
        "version": 1,
        "chain_id": chain_id(network),
        "height": 42,
        "prev_block": bytes(range(32)),
        "tx_root": bytes(range(32, 64)),
        "timestamp": 1_790_000_000,
        "target": NETWORKS[network][1] if target is None else target,
        "nonce": nonce,
    }


def first_nonce(header, want_valid):
    h = dict(header)
    while meets_target(h) != want_valid:
        h["nonce"] += 1
    return h


def header_json(h, description):
    return {
        "description": description,
        "version": h["version"],
        "chain_id": hx(h["chain_id"]),
        "height": h["height"],
        "prev_block": hx(h["prev_block"]),
        "tx_root": hx(h["tx_root"]),
        "timestamp": h["timestamp"],
        "target": "%064x" % h["target"],
        "nonce": h["nonce"],
        "encoding": hx(encode_header(h)),
        "id": hx(block_id(h)),
        "meets_target": meets_target(h),
        "work": "%064x" % work(h["target"]),
    }


def vectors_block_header():
    cases = [
        header_json(first_nonce(sample_header("devnet"), True), "devnet header, first nonce meeting POW_LIMIT"),
        header_json(first_nonce(sample_header("devnet"), False), "devnet header, first nonce failing POW_LIMIT"),
        header_json(first_nonce(sample_header("regtest"), True), "regtest header, first nonce meeting POW_LIMIT_REGTEST"),
        header_json(sample_header("mainnet", nonce=2**64 - 1, target=1), "maximum nonce, minimum target"),
    ]
    work_cases = [
        {"target": "%064x" % t, "work": "%064x" % work(t)}
        for t in [1, 2, 3, EXPECTED, 2**128, POW_LIMIT, POW_LIMIT_REGTEST, 2**255, 2**256 - 2]
    ]
    return {"description": "Header encoding, block_id, BLK-06 and work(target) (spec/02 §1, §5)", "cases": cases, "work": work_cases}


def vectors_difficulty():
    I = RETARGET_INTERVAL
    L = POW_LIMIT
    mid = 0x00000123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789AB
    scenarios = [
        ("off-boundary height keeps parent target", 61, mid, L, None, None),
        ("first boundary h = I keeps parent target", I, mid, L, 0, 10),
        ("on schedule", 2 * I, mid, L, 1000, 1000 + EXPECTED),
        ("twice as fast halves the target", 2 * I, mid, L, 1000, 1000 + EXPECTED // 2),
        ("clamped at 4x faster", 3 * I, mid, L, 1000, 1001),
        ("zero timespan clamps to minimum", 3 * I, mid, L, 5000, 5000),
        ("negative timespan clamps to minimum", 3 * I, mid, L, 5000, 4000),
        ("clamped at 4x slower", 4 * I, mid, L, 1000, 1000 + 100 * EXPECTED),
        ("slower but capped at POW_LIMIT", 2 * I, L, L, 0, 2 * EXPECTED),
        ("target 1 getting harder is floored at 1", 2 * I, 1, L, 0, 1),
        ("target 3 on schedule stays 3", 2 * I, 3, L, 0, EXPECTED),
        ("regtest limit, 4x slower, capped", 2 * I, POW_LIMIT_REGTEST, POW_LIMIT_REGTEST, 0, 4 * EXPECTED),
        ("regtest limit, 3x slower from a quarter", 2 * I, POW_LIMIT_REGTEST // 4, POW_LIMIT_REGTEST, 0, 3 * EXPECTED),
        ("remainder term matters", 2 * I, EXPECTED * 1000 + EXPECTED - 1, L, 0, EXPECTED + 1),
    ]
    cases = []
    for desc, h, parent, limit, first, last in scenarios:
        cases.append({
            "description": desc,
            "next_height": h,
            "parent_target": "%064x" % parent,
            "pow_limit": "%064x" % limit,
            "ts_first": first,
            "ts_last": last,
            "expected": "%064x" % next_target(h, parent, limit, first, last),
        })
    mtp_cases = [
        {"timestamps": ts, "median": median_time_past(ts)}
        for ts in [[7], [3, 1, 2], [4, 1, 3, 2], [10, 10, 10], list(range(11, 0, -1)), [5, 100, 1, 50, 50, 2]]
    ]
    return {
        "description": "NextTarget (spec/02 §4). ts_first = ts(h-I-1), ts_last = ts(h-1); null when unused.",
        "cases": cases,
        "median_time_past": mtp_cases,
    }


def vectors_genesis():
    cases = []
    for network in GENESIS:
        header, block = genesis_block(network)
        cases.append({
            "network": network,
            "timestamp": header["timestamp"],
            "extra": hx(GENESIS[network]["extra"]),
            "nonce": header["nonce"],
            "block": hx(block),
            "id": hx(block_id(header)),
        })
    return {"description": "Genesis blocks (spec/02 §6); mainnet and testnet are fixed at launch", "cases": cases}


# --- spec/05-course-session.md ------------------------------------------------------------------------

GIB = 2**30


def text(s):
    b = s.encode("utf-8")
    return u32(len(b)) + b


def encode_file_entry(path, content, size):
    return text(path) + content + u64(size)


def course_root(entries):
    # Sorted by the UTF-8 bytes of the path, which is what Python's bytes ordering gives.
    ordered = sorted(entries, key=lambda e: e[0].encode("utf-8"))
    return merkle_root([encode_file_entry(*e) for e in ordered])


def course_cost(is_update, files, size):
    base = 15 if is_update else 30
    return base + files * 1 + -(-size // GIB) * 5


def lock_bounds(estimate):
    return estimate, estimate + estimate * 20 // 100


def cancel_fee(locked):
    return min(locked, max(2, locked * 5 // 100))


def path_rule(path):
    if "\\" in path:
        return "CRS-PATH-03"
    for segment in path.split("/"):
        if segment == "":
            return "CRS-PATH-01"
        if segment in (".", ".."):
            return "CRS-PATH-02"
    return None


def vectors_course():
    h = lambda n: bytes([n]) * 32
    file_sets = [
        [("a", h(1), 1)],
        [("第2章/b.mp4", h(2), 7 << 20), ("第1章/a.mp4", h(1), 3 << 20), ("讲义.pdf", h(3), 1)],
        [("Z", h(4), 0), ("a", h(5), GIB), ("é", h(6), GIB + 1), ("e\u0301", h(7), 2)],
    ]
    roots = [
        {"entries": [{"path": p, "content_hash": hx(c), "size": s} for p, c, s in fs], "root": hx(course_root(fs))}
        for fs in file_sets
    ]
    pricing = [
        {"update": u, "files": f, "bytes": b, "cost": course_cost(u, f, b), "lock_bounds": list(lock_bounds(course_cost(u, f, b)))}
        for u, f, b in [(False, 1, 0), (False, 1, 1), (False, 10, 2 * GIB), (False, 40, 12 * GIB), (False, 3, GIB), (False, 3, GIB + 1), (True, 1, 1), (True, 5, 0)]
    ]
    fees = [{"locked": l, "fee": cancel_fee(l)} for l in [1, 2, 3, 30, 39, 40, 41, 60, 400]]
    paths = [
        {"path": p, "error": path_rule(p)}
        for p in ["a", "a/b", "第1章/视频.mp4", ".hidden", "a/..b", "...", "", "/a", "a/", "a//b", ".", "..", "a/./b", "a/../b", "a\\b"]
    ]
    record = h(0xAA) + ed25519_public(bytes([1]) * 32) + u64(40) + u64(2) + u64(38)
    return {
        "description": "Course roots, pricing, lock bounds, cancel fees, path rules, AutoCancelRecord encoding (spec/05)",
        "course_roots": roots,
        "pricing": pricing,
        "cancel_fees": fees,
        "paths": paths,
        "auto_cancel_record": {
            "session_id": hx(h(0xAA)),
            "owner": hx(ed25519_public(bytes([1]) * 32)),
            "locked": 40,
            "fee": 2,
            "restored": 38,
            "encoding": hx(record),
        },
    }


def vectors_course_tx():
    secret = bytes([1]) * 32
    session = bytes([0x5E]) * 32
    outpoints = [(bytes([0xA1]) * 32, 0)]
    entries = [("第1章/a.mp4", bytes([1]) * 32, 3 << 20), ("讲义.pdf", bytes([3]) * 32, 1)]
    bodies = {
        "begin_course": (0x20, encode_outpoints(outpoints) + b"\x01" + bytes([0x77]) * 32 + text("课程") + text("1.0") + u32(2) + u64(3 << 20) + u64(40)),
        "begin_course_new": (0x20, encode_outpoints(outpoints) + b"\x00" + text("课程") + text("1.0") + u32(2) + u64(3 << 20) + u64(40)),
        "append_files": (0x21, session + u32(len(entries)) + b"".join(encode_file_entry(*e) for e in entries)),
        "add_lock": (0x22, encode_outpoints(outpoints) + session + u32(3) + u64(1) + u64(2)),
        "finalize_course": (0x23, session),
        "cancel_course": (0x24, session),
    }
    cases = []
    for name, (tag, body) in bodies.items():
        c = signed_user_tx(tag, secret, 7, body, "regtest")
        c.update({"type": name, "network": "regtest", "secret": hx(secret)})
        cases.append(c)
    return {"description": "Course transaction encodings, sig_msg, signatures and txids (spec/05)", "cases": cases}


def vectors_tx():
    outpoints = [(bytes([0xA1]) * 32, 0), (bytes([0xA2]) * 32, 1), (bytes([0xA3]) * 32, 256)]
    recipient = ed25519_public(bytes([2]) * 32)
    commitment = (CIRCUIT_FIELD_MODULUS - 1).to_bytes(32, "little")
    cases = []
    for network in ["regtest", "mainnet"]:
        transfer_body = encode_outpoints(outpoints) + recipient + u64(25)
        t = signed_user_tx(0x10, bytes([1]) * 32, 3, transfer_body, network)
        t.update({"type": "transfer", "network": network, "secret": hx(bytes([1]) * 32)})
        cases.append(t)
        identity_body = encode_outpoints(outpoints[:1]) + commitment
        r = signed_user_tx(0x11, bytes([4]) * 32, 0, identity_body, network)
        r.update({"type": "register_identity", "network": network, "secret": hx(bytes([4]) * 32)})
        cases.append(r)
        proof = b"p4-proof-vector"
        author_body = (
            encode_outpoints(outpoints[:1])
            + bytes([0x55]) * 32
            + bytes([0x66]) * 32
            + bytes([1]) + bytes(31)
            + u32(len(proof)) + proof
        )
        author = signed_user_tx(0x30, bytes([4]) * 32, 1, author_body, network)
        author.update({
            "type": "verify_author",
            "network": network,
            "secret": hx(bytes([4]) * 32),
            "proof_status": "synthetic wire-format payload; not a valid Halo2 proof",
        })
        cases.append(author)

    loss = [{"amount": a, "loss": max(1, a * 10 // 100)} for a in [2, 9, 10, 11, 19, 20, 25, 99, 100, 199, 500]]
    field = [
        {"value": hx(v.to_bytes(32, "little")), "canonical": v < CIRCUIT_FIELD_MODULUS}
        for v in [0, 1, CIRCUIT_FIELD_MODULUS - 1, CIRCUIT_FIELD_MODULUS, CIRCUIT_FIELD_MODULUS + 1, 2**256 - 1]
    ]
    poseidon_commitments = [{
        "source": "Halo2 P128Pow5T3 official Pallas fixed-length hash vector (input [0, 1])",
        "secret": hx(bytes(32)),
        "salt": hx((1).to_bytes(32, "little")),
        "commitment": "8358d711a0329d38becd54fba7c283ed3e089a39c91b6a9d10efb02bc3f12f06",
    }]
    return {
        "description": "User transaction encoding, sig_msg, deterministic RFC 8032 signatures, txid; transfer loss; commitment field and official Poseidon check",
        "cases": cases,
        "transfer_loss": loss,
        "field_elements": field,
        "poseidon_commitments": poseidon_commitments,
    }


def vectors_codec():
    def case(ty, data, error=None):
        c = {"type": ty, "hex": hx(data)}
        c["error"] = error
        return c

    return {
        "description": "Strict decoding (spec/01 §1-§3). error is null for valid input, else the rule ID.",
        "cases": [
            case("u16", b"\x02\x01"),
            case("u64", u64(2**64 - 1)),
            case("u64", b"\x00" * 7, "CODEC-DEC-06"),
            case("u64", b"\x00" * 9, "CODEC-DEC-01"),
            case("bool", b"\x00"),
            case("bool", b"\x01"),
            case("bool", b"\x02", "CODEC-DEC-03"),
            case("option_u8", b"\x00"),
            case("option_u8", b"\x01\x07"),
            case("option_u8", b"\x02\x07", "CODEC-DEC-03"),
            case("option_u8", b"\x01", "CODEC-DEC-06"),
            case("varbytes_4", u32(0)),
            case("varbytes_4", u32(4) + b"abcd"),
            case("varbytes_4", u32(5) + b"abcde", "CODEC-DEC-02"),
            case("varbytes_4", u32(3) + b"ab", "CODEC-DEC-06"),
            case("text_8", u32(6) + "猿来".encode("utf-8")),
            case("text_8", u32(2) + b"\xc3\x28", "CODEC-DEC-05"),
            case("text_8", u32(1) + b"\x1f", "CODEC-DEC-05"),
            case("text_8", u32(1) + b"\x7f", "CODEC-DEC-05"),
            case("text_8", u32(2) + "\u0085".encode("utf-8"), "CODEC-DEC-05"),
            case("text_8", u32(2) + "\u00a0".encode("utf-8")),
            case("list_u16_3", u32(2) + b"\x01\x00\x03\x02"),
            case("list_u16_3", u32(4), "CODEC-DEC-02"),
            case("list_u16_3", u32(0xFFFFFFFF), "CODEC-DEC-02"),
            case("tx", b"\xff", "CODEC-DEC-04"),
        ],
    }


SETS = {
    "codec.json": vectors_codec,
    "tagged-hash.json": vectors_tagged_hash,
    "merkle.json": vectors_merkle,
    "block-header.json": vectors_block_header,
    "difficulty.json": vectors_difficulty,
    "genesis.json": vectors_genesis,
    "tx.json": vectors_tx,
    "course.json": vectors_course,
    "course-tx.json": vectors_course_tx,
}


def render(obj):
    return json.dumps(obj, indent=2, ensure_ascii=False, sort_keys=True) + "\n"


def main():
    check = "--check" in sys.argv[1:]
    stale = []
    for name, build in SETS.items():
        path = OUT_DIR / name
        text = render(build())
        if check:
            if not path.exists() or path.read_text(encoding="utf-8") != text:
                stale.append(name)
        else:
            path.write_text(text, encoding="utf-8")
    if stale:
        print("stale test vectors: " + ", ".join(stale), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
