//! Tests for course write sessions (`spec/05-course-session.md`).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "test code: any failure should abort the test"
)]

#[macro_use]
mod common;

use common::{
    A, B, C, add_lock, append, begin, begin_with, cancel, entry, finalize, inputs_for, mine,
    mine_blocks, mine_until_next, next_height, nonce, pk, world,
};
use yuanlai_consensus::{Chain, Session, course_root};
use yuanlai_types::params::{
    CANCEL_LIMIT_WINDOW, EXPIRY_BLOCKS, GIB, MAX_FILES_PER_APPEND, MAX_FILES_PER_COURSE,
    SESSION_IDLE_BLOCKS, SESSION_MAX_BLOCKS,
};
use yuanlai_types::{
    CourseTitle, CourseVersion, Credit, FileEntry, Hash, Height, OutPoint, PubKey, Source, Tx, Utxo,
};

/// Opens a session for `seed` and returns its ID (= txid of `BeginCourse`).
fn open(
    chain: &mut Chain,
    seed: u8,
    parent: Option<Hash>,
    declared: (u32, u64),
    lock: u64,
) -> Hash {
    let tx = begin(chain, seed, parent, declared, lock);
    let id = tx.txid();
    accept!(*chain, vec![tx]);
    id
}

/// Registers `files` as a complete new course (or update) and returns its root. `declared` counts
/// changed files for an update.
fn register_course(
    chain: &mut Chain,
    seed: u8,
    parent: Option<Hash>,
    files: Vec<FileEntry>,
    declared: (u32, u64),
    lock: u64,
) -> Hash {
    let session = open(chain, seed, parent, declared, lock);
    let k = nonce(chain, seed);
    accept!(
        *chain,
        vec![
            append(chain, seed, k, session, files),
            finalize(chain, seed, k + 1, session)
        ]
    );
    chain.state().courses().course(&session).unwrap().root
}

fn outputs_of(chain: &Chain, tx_id: Hash) -> Vec<(OutPoint, Utxo)> {
    chain
        .state()
        .utxos()
        .filter(|(o, _)| o.txid == tx_id)
        .map(|(o, u)| (*o, *u))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Full flows

#[test]
fn new_course_end_to_end() {
    let mut chain = world();
    let balance = chain.state().balance(&pk(A));
    let burned = chain.state().totals().burned;

    // Estimate 30 + 3 + 5 = 38, bounds [38, 45].
    let files = vec![
        entry("第2章/b.mp4", 2, 7 << 20),
        entry("第1章/a.mp4", 1, 3 << 20),
        entry("讲义.pdf", 3, 1),
    ];
    let session = open(&mut chain, A, None, (3, 10 << 20), 40);
    let s = chain.state().courses().session(&session).unwrap().clone();
    assert_eq!(s.locked, Credit(40));
    assert_eq!(
        chain.state().balance(&pk(A)),
        balance,
        "locked credit still counts"
    );

    let k = nonce(&chain, A);
    accept!(chain, vec![append(&chain, A, k, session, files.clone())]);
    let fin = finalize(&chain, A, k + 1, session);
    let h = next_height(&chain);
    accept!(chain, vec![fin.clone()]);

    let courses = chain.state().courses();
    let course = courses.course(&session).unwrap();
    let mut sorted = files.clone();
    sorted.sort_by(|a, b| a.path.as_str().as_bytes().cmp(b.path.as_str().as_bytes()));
    assert_eq!(course.root, course_root(&sorted));
    assert_eq!(
        courses.course_files(&session),
        sorted.iter().collect::<Vec<_>>()
    );
    assert_eq!(
        (course.owner, course.finalized_height, course.position),
        (pk(A), h, 3)
    );
    assert_eq!((course.file_count, course.total_size), (3, (10 << 20) + 1));
    assert!(courses.session(&session).is_none());
    assert!(courses.open_session(&pk(A)).is_none());
    assert_eq!(
        courses.registrations_of_content(&Hash([2; 32]))[0].course_id,
        session
    );
    assert_eq!(
        courses
            .course_by_owner_root(&pk(A), &course.root)
            .unwrap()
            .id,
        session
    );

    let released = outputs_of(&chain, fin.txid());
    assert_eq!(released.len(), 1);
    let (_, utxo) = released[0];
    assert_eq!(
        (utxo.amount, utxo.source, utxo.renewals),
        (Credit(2), Source::Released, 1)
    );
    assert_eq!(utxo.expire_height, Height(h.0 + EXPIRY_BLOCKS));
    assert_eq!(chain.state().totals().burned, Credit(burned.0 + 38));
    assert_eq!(chain.state().balance(&pk(A)), Credit(balance.0 - 38));
}

#[test]
fn locked_credit_does_not_expire() {
    let mut chain = world();
    let inputs = inputs_for(&chain, A, 40, |_| true);
    let earliest_expiry = chain.state().utxo(&inputs[0]).unwrap().expire_height;
    let session = open(&mut chain, A, None, (3, 1), 40);

    // Keep the session active until well past the inputs' original expiry.
    mine_until_next(&mut chain, earliest_expiry.0 - 100);
    let k = nonce(&chain, A);
    accept!(
        chain,
        vec![append(
            &chain,
            A,
            k,
            session,
            vec![entry("a", 1, 1), entry("b", 2, 1), entry("c", 3, 1)]
        )]
    );
    mine_until_next(&mut chain, earliest_expiry.0 + 10);

    assert_eq!(
        chain.state().courses().session(&session).unwrap().locked,
        Credit(40)
    );
    accept!(chain, vec![finalize(&chain, A, k + 1, session)]);
}

#[test]
fn versions_pay_only_for_changed_files() {
    let mut chain = world();
    let v1_files = vec![entry("a", 1, GIB), entry("b", 2, GIB)];
    let v1 = register_course(&mut chain, A, None, v1_files, (2, 2 * GIB), 42);
    let burned = chain.state().totals().burned;

    // One unchanged file, one changed file of 1 byte: 15 + 1 + 5 = 21.
    let v2_files = vec![entry("a", 1, GIB), entry("b", 9, 1)];
    let v2 = register_course(&mut chain, A, Some(v1), v2_files, (1, 1), 21);
    assert_eq!(chain.state().totals().burned, Credit(burned.0 + 21));

    let courses = chain.state().courses();
    assert_eq!(courses.successor(&pk(A), &v1).unwrap().root, v2);
    assert_eq!(
        courses
            .course_by_owner_root(&pk(A), &v2)
            .unwrap()
            .parent_root,
        Some(v1)
    );
}

#[test]
fn same_content_by_others_is_allowed_and_ordered() {
    let mut chain = world();
    let files = vec![entry("x", 7, 1)];
    let root_a = register_course(&mut chain, A, None, files.clone(), (1, 1), 36);
    let root_b = register_course(&mut chain, B, None, files, (1, 1), 36);
    assert_eq!(root_a, root_b);

    let regs = chain.state().courses().registrations_of_root(&root_a);
    assert_eq!(regs.len(), 2);
    assert!((regs[0].height, regs[0].position) < (regs[1].height, regs[1].position));
    assert_eq!(
        chain
            .state()
            .courses()
            .course(&regs[0].course_id)
            .unwrap()
            .owner,
        pk(A)
    );
    assert_eq!(
        chain
            .state()
            .courses()
            .registrations_of_content(&Hash([7; 32]))
            .len(),
        2
    );
}

// ---------------------------------------------------------------------------------------------
// BeginCourse

#[test]
fn begin_rules() {
    let mut chain = world();
    // C has 40 mature credit but no identity.
    mine_blocks(&mut chain, C, 4);
    mine_blocks(&mut chain, common::FILLER, 10);
    reject!(
        chain,
        vec![begin(&chain, C, None, (1, 1), 36)],
        "CRS-BEGIN-01"
    );

    reject!(
        chain,
        vec![begin(&chain, A, None, (0, 0), 30)],
        "CRS-BEGIN-03"
    );
    reject!(
        chain,
        vec![begin(&chain, A, None, (MAX_FILES_PER_COURSE + 1, 1), 50)],
        "CRS-BEGIN-03"
    );
    // Estimate 36, bounds [36, 43].
    reject!(
        chain,
        vec![begin(&chain, A, None, (1, 1), 35)],
        "CRS-BEGIN-04"
    );
    reject!(
        chain,
        vec![begin(&chain, A, None, (1, 1), 44)],
        "CRS-BEGIN-04"
    );
    reject!(
        chain,
        vec![begin(&chain, A, Some(Hash([4; 32])), (1, 1), 21)],
        "CRS-BEGIN-05"
    );

    open(&mut chain, A, None, (1, 1), 43);
    reject!(
        chain,
        vec![begin(&chain, A, None, (1, 1), 36)],
        "CRS-BEGIN-02"
    );
}

#[test]
fn begin_05_parent_must_be_own_latest_version() {
    let mut chain = world();
    let v1 = register_course(&mut chain, A, None, vec![entry("a", 1, 1)], (1, 1), 36);
    reject!(
        chain,
        vec![begin(&chain, B, Some(v1), (1, 1), 21)],
        "CRS-BEGIN-05"
    );
    register_course(&mut chain, A, Some(v1), vec![entry("a", 2, 1)], (1, 1), 21);
    reject!(
        chain,
        vec![begin(&chain, A, Some(v1), (1, 1), 21)],
        "CRS-BEGIN-05"
    );
}

/// Produces a UTXO of `seed` with `renewals = 2` by finalizing twice with surplus lock.
fn twice_released(chain: &mut Chain, seed: u8) -> OutPoint {
    let mut released = None;
    for round in 0..2_u8 {
        let k = nonce(chain, seed);
        // Estimate 36, lock 43: 7 is released. The second round spends the first release last.
        let inputs = match released {
            None => inputs_for(chain, seed, 43, |u| u.renewals == 0),
            Some(r) => {
                let mut v = inputs_for(chain, seed, 36, |u| u.renewals == 0);
                v.push(r);
                v
            }
        };
        let files = vec![entry("f", 50 + round, 1)];
        let begin_tx = begin_with(chain, seed, k, inputs, None, (1, 1), 43);
        let session = begin_tx.txid();
        let fin = finalize(chain, seed, k + 2, session);
        accept!(
            *chain,
            vec![
                begin_tx,
                append(chain, seed, k + 1, session, files),
                fin.clone()
            ]
        );
        released = Some(outputs_of(chain, fin.txid())[0].0);
    }
    let outpoint = released.unwrap();
    assert_eq!(chain.state().utxo(&outpoint).unwrap().renewals, 2);
    outpoint
}

#[test]
fn begin_06_and_addlock_04_renewal_limit() {
    let mut chain = world();
    let exhausted = twice_released(&mut chain, A);
    let k = nonce(&chain, A);
    let mut inputs = inputs_for(&chain, A, 36, |u| u.renewals == 0);
    inputs.push(exhausted);
    reject!(
        chain,
        vec![begin_with(&chain, A, k, inputs, None, (1, 1), 43)],
        "CRS-BEGIN-06"
    );

    let session = open(&mut chain, A, None, (1, 1), 36);
    let k = nonce(&chain, A);
    let body = yuanlai_types::AddLock {
        inputs: yuanlai_codec::BoundedVec::new(vec![exhausted]).unwrap(),
        session_id: session,
        declared_files: 1,
        declared_bytes: 1,
        amount: Credit(7),
    };
    let tx = common::signed(&chain, A, Tx::AddLock(common::user(A, k, body)));
    reject!(chain, vec![tx], "CRS-ADDLOCK-04");
}

// ---------------------------------------------------------------------------------------------
// AppendFiles

#[test]
fn append_rules() {
    let mut chain = world();
    let session = open(&mut chain, A, None, (2, 2), 37);
    let other = open(&mut chain, B, None, (1, 1), 36);
    accept!(
        chain,
        vec![append(
            &chain,
            A,
            nonce(&chain, A),
            session,
            vec![entry("a", 1, 1)]
        )]
    );
    let k = nonce(&chain, A);

    let cases: Vec<(Hash, Vec<FileEntry>, &str)> = vec![
        (other, vec![entry("b", 2, 1)], "CRS-APPEND-01"),
        (Hash([1; 32]), vec![entry("b", 2, 1)], "CRS-APPEND-01"),
        (session, vec![entry("/b", 2, 1)], "CRS-PATH-01"),
        (session, vec![entry("b/", 2, 1)], "CRS-PATH-01"),
        (session, vec![entry("x//b", 2, 1)], "CRS-PATH-01"),
        (session, vec![entry("x/../b", 2, 1)], "CRS-PATH-02"),
        (session, vec![entry("x\\b", 2, 1)], "CRS-PATH-03"),
        (session, vec![entry("a", 2, 1)], "CRS-APPEND-03"),
        (
            session,
            vec![entry("b", 2, 1), entry("b", 3, 1)],
            "CRS-APPEND-03",
        ),
        (session, vec![entry("b", 1, 1)], "CRS-APPEND-04"),
        (
            session,
            vec![entry("b", 2, 1), entry("c", 2, 1)],
            "CRS-APPEND-04",
        ),
        (session, vec![], "CRS-APPEND-06"),
        (session, vec![entry("b", 2, u64::MAX)], "CRS-APPEND-07"),
    ];
    for (id, entries, rule) in cases {
        reject!(chain, vec![append(&chain, A, k, id, entries)], rule);
    }
    accept!(
        chain,
        vec![append(&chain, A, k, session, vec![entry("b", 2, 1)])]
    );
}

#[test]
fn append_05_file_limit() {
    let mut chain = world();
    // Appending is free, so an open session can reach the limit even though finalizing that many
    // files would cost more than BALANCE_CAP allows.
    let session = open(&mut chain, A, None, (1, 0), 31);
    let per_tx = MAX_FILES_PER_APPEND;
    let mut next = 0_u32;
    while next < MAX_FILES_PER_COURSE {
        let k = nonce(&chain, A);
        let mut txs = Vec::new();
        for i in 0..16 {
            if next >= MAX_FILES_PER_COURSE {
                break;
            }
            let count = per_tx.min(MAX_FILES_PER_COURSE - next);
            let entries = (next..next + count)
                .map(|n| FileEntry {
                    path: common::text(&format!("f{n:05}")),
                    content_hash: Hash(u256_bytes(n)),
                    size: 0,
                })
                .collect();
            txs.push(append(&chain, A, k + i, session, entries));
            next += count;
        }
        accept!(chain, txs);
    }
    assert_eq!(
        chain
            .state()
            .courses()
            .session(&session)
            .unwrap()
            .file_count,
        MAX_FILES_PER_COURSE
    );
    let extra = vec![FileEntry {
        path: common::text("extra"),
        content_hash: Hash([0xEE; 32]),
        size: 0,
    }];
    reject!(
        chain,
        vec![append(&chain, A, nonce(&chain, A), session, extra)],
        "CRS-APPEND-05"
    );
}

fn u256_bytes(n: u32) -> [u8; 32] {
    let mut out = [0xAB; 32];
    out[..4].copy_from_slice(&n.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------------------------
// AddLock and FinalizeCourse

#[test]
fn addlock_rules_and_fin_04() {
    let mut chain = world();
    // Declared 1 file: estimate 36, lock 36. Three files make the actual cost 38.
    let session = open(&mut chain, A, None, (1, 1), 36);
    let other = open(&mut chain, B, None, (1, 1), 36);
    let k = nonce(&chain, A);
    accept!(
        chain,
        vec![append(
            &chain,
            A,
            k,
            session,
            vec![entry("a", 1, 1), entry("b", 2, 1), entry("c", 3, 1)]
        )]
    );
    reject!(
        chain,
        vec![finalize(&chain, A, k + 1, session)],
        "CRS-FIN-04"
    );

    let k = k + 1;
    reject!(
        chain,
        vec![add_lock(&chain, A, k, other, (3, 3), 2)],
        "CRS-ADDLOCK-01"
    );
    reject!(
        chain,
        vec![add_lock(&chain, A, k, session, (0, 3), 2)],
        "CRS-ADDLOCK-02"
    );
    // New estimate 38, bounds [38, 45]: 36 + 1 is too little, 36 + 10 too much.
    reject!(
        chain,
        vec![add_lock(&chain, A, k, session, (3, 3), 1)],
        "CRS-ADDLOCK-03"
    );
    reject!(
        chain,
        vec![add_lock(&chain, A, k, session, (3, 3), 10)],
        "CRS-ADDLOCK-03"
    );
    reject!(
        chain,
        vec![add_lock(&chain, A, k, session, (3, 3), 0)],
        "CRS-ADDLOCK-05"
    );

    accept!(
        chain,
        vec![
            add_lock(&chain, A, k, session, (3, 3), 2),
            finalize(&chain, A, k + 1, session)
        ]
    );
}

#[test]
fn finalize_rules() {
    let mut chain = world();
    let session = open(&mut chain, A, None, (1, 1), 36);
    let other = open(&mut chain, B, None, (1, 1), 36);
    let k = nonce(&chain, A);
    reject!(chain, vec![finalize(&chain, A, k, other)], "CRS-FIN-01");
    reject!(chain, vec![finalize(&chain, A, k, session)], "CRS-FIN-02");
    accept!(
        chain,
        vec![
            append(&chain, A, k, session, vec![entry("a", 1, 1)]),
            finalize(&chain, A, k + 1, session)
        ]
    );

    // The same file set again by the same owner.
    let again = open(&mut chain, A, None, (1, 1), 36);
    let k = nonce(&chain, A);
    accept!(
        chain,
        vec![append(&chain, A, k, again, vec![entry("a", 1, 1)])]
    );
    reject!(chain, vec![finalize(&chain, A, k + 1, again)], "CRS-FIN-05");
}

#[test]
fn fin_03_update_needs_a_changed_file() {
    let mut chain = world();
    let v1 = register_course(&mut chain, A, None, vec![entry("a", 1, 1)], (1, 1), 36);
    let session = open(&mut chain, A, Some(v1), (1, 1), 21);
    let k = nonce(&chain, A);
    // Renamed, same content: nothing changed.
    accept!(
        chain,
        vec![append(&chain, A, k, session, vec![entry("renamed", 1, 1)])]
    );
    reject!(
        chain,
        vec![finalize(&chain, A, k + 1, session)],
        "CRS-FIN-03"
    );
}

// ---------------------------------------------------------------------------------------------
// Cancellation

#[test]
fn cancel_effects_and_limit() {
    let mut chain = world();
    let session = open(&mut chain, A, None, (1, 1), 40);
    let burned = chain.state().totals().burned;
    let tx = cancel(&chain, A, nonce(&chain, A), session);
    let h = next_height(&chain);
    accept!(chain, vec![tx.clone()]);

    let restored = outputs_of(&chain, tx.txid());
    let (_, utxo) = restored[0];
    // Fee max(2, 5% of 40) = 2.
    assert_eq!(
        (utxo.amount, utxo.source, utxo.renewals),
        (Credit(38), Source::Cancelled, 1)
    );
    assert_eq!(utxo.expire_height, Height(h.0 + EXPIRY_BLOCKS));
    assert_eq!(chain.state().totals().burned, Credit(burned.0 + 2));
    assert!(chain.state().courses().session(&session).is_none());

    let other = open(&mut chain, B, None, (1, 1), 36);
    reject!(
        chain,
        vec![cancel(&chain, A, nonce(&chain, A), other)],
        "CRS-CANCEL-01"
    );

    let second = open(&mut chain, A, None, (1, 1), 36);
    reject!(
        chain,
        vec![cancel(&chain, A, nonce(&chain, A), second)],
        "CRS-CANCEL-02"
    );
    mine_until_next(&mut chain, h.0 + CANCEL_LIMIT_WINDOW - 1);
    // Keep the session alive; blocked through h + W − 1 because the window is (h' − W, h'].
    reject!(
        chain,
        vec![cancel(&chain, A, nonce(&chain, A), second)],
        "CRS-CANCEL-02"
    );
    mine_blocks(&mut chain, common::FILLER, 1);
    accept!(chain, vec![cancel(&chain, A, nonce(&chain, A), second)]);
}

#[test]
fn auto_cancel_after_idle_period() {
    let mut chain = world();
    let started = next_height(&chain);
    let session = open(&mut chain, A, None, (1, 1), 40);
    let deadline = started.0 + SESSION_IDLE_BLOCKS;
    assert_eq!(
        chain
            .state()
            .courses()
            .session(&session)
            .unwrap()
            .deadline(),
        Height(deadline)
    );
    let burned = chain.state().totals().burned;

    mine_until_next(&mut chain, deadline);
    let block = mine(&mut chain, vec![]).unwrap();
    let Tx::AutoCancel(commitment) = &block.txs.as_slice()[1] else {
        panic!()
    };
    assert_eq!(commitment.count, 1);
    let delta = chain.delta_at(Height(deadline)).unwrap();
    assert_eq!(delta.courses.auto_cancelled.len(), 1);
    let record = &delta.courses.auto_cancelled[0];
    assert_eq!(
        (
            record.session_id,
            record.locked,
            record.fee,
            record.restored
        ),
        (session, Credit(40), Credit(2), Credit(38))
    );
    assert_eq!(chain.state().totals().burned, Credit(burned.0 + 2));

    let restored = outputs_of(&chain, block.txs.as_slice()[1].txid());
    assert_eq!(restored[0].0.index, 0);
    assert_eq!(
        (restored[0].1.source, restored[0].1.renewals),
        (Source::Cancelled, 1)
    );

    // Automatic cancellation does not count towards CRS-CANCEL-02.
    let next = open(&mut chain, A, None, (1, 1), 36);
    accept!(chain, vec![cancel(&chain, A, nonce(&chain, A), next)]);
}

#[test]
fn session_at_its_deadline_is_gone_before_user_transactions() {
    let mut chain = world();
    let started = next_height(&chain);
    let session = open(&mut chain, A, None, (1, 1), 36);
    mine_until_next(&mut chain, started.0 + SESSION_IDLE_BLOCKS);
    reject!(
        chain,
        vec![append(
            &chain,
            A,
            nonce(&chain, A),
            session,
            vec![entry("a", 1, 1)]
        )],
        "CRS-APPEND-01"
    );
}

#[test]
fn deadline_is_capped_by_maximum_lifetime() {
    let session = |started: u64, last: u64| Session {
        id: Hash::ZERO,
        owner: PubKey([0; 32]),
        parent_root: None,
        title: CourseTitle::empty(),
        version: CourseVersion::empty(),
        declared_files: 1,
        declared_bytes: 0,
        locked: Credit(31),
        max_hops: 0,
        max_renewals: 0,
        file_count: 0,
        total_size: 0,
        started_height: Height(started),
        last_activity_height: Height(last),
    };
    assert_eq!(
        session(100, 100).deadline(),
        Height(100 + SESSION_IDLE_BLOCKS)
    );
    assert_eq!(
        session(100, 5_000).deadline(),
        Height(5_000 + SESSION_IDLE_BLOCKS)
    );
    assert_eq!(
        session(100, 10_000).deadline(),
        Height(100 + SESSION_MAX_BLOCKS)
    );
}
