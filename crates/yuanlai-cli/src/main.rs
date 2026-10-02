//! `yuanlai`: command-line wallet, mining control and course registration tool.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rand::{rand_core::UnwrapErr, rngs::SysRng};
use yuanlai_codec::{BoundedBytes, BoundedVec, Encode};
use yuanlai_consensus::{Chain, check_path, course_cost, course_root, is_canonical_field_element};
use yuanlai_crypto::{Hash, public_key, sha256, sign};
use yuanlai_mempool::Mempool;
use yuanlai_storage::read_blocks;
use yuanlai_types::params::{
    COST_IDENTITY, COST_VERIFY_AUTHOR, IDENTITY_CIRCUIT_K, MAX_FILES_PER_APPEND,
    MAX_FILES_PER_COURSE, MAX_INPUTS, MAX_RENEWALS, MAX_USER_TXS_PER_SENDER_PER_BLOCK,
};
use yuanlai_types::{
    AppendFiles, BeginCourse, CoursePath, CourseTitle, CourseVersion, Credit, FileEntry, Network,
    OutPoint, PubKey, RegisterIdentity, SessionRef, Signature, Tx, UserTx, VerifyAuthor,
};
use yuanlai_zk::{identity_commitment, prove_author, setup};
use zeroize::Zeroizing;

const USAGE: &str = "usage: yuanlai scan <directory>
    yuanlai identity-register (--commitment HASH | --identity-secret-file FILE --salt-file FILE)
            --key-file FILE --out-dir PATH
                  [--network devnet|regtest] [--datadir PATH]
       yuanlai author-verify --course-root HASH --challenge HASH
                  --identity-secret-file FILE --salt-file FILE --key-file FILE
                  --out-dir PATH [--network devnet|regtest] [--datadir PATH]
       yuanlai register --course-dir DIR --title TEXT --version TEXT --key-file FILE
              [--parent-root HASH] [--network devnet|regtest]
              [--datadir PATH] --out-dir PATH
       yuanlai --version";

struct Manifest {
    entries: Vec<FileEntry>,
    root: Hash,
    total_size: u64,
    estimated_cost: Credit,
}

fn visit_directory(
    directory: &Path,
    prefix: &str,
    entries: &mut Vec<FileEntry>,
) -> Result<(), String> {
    let mut children = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    children.sort_by_key(std::fs::DirEntry::file_name);

    for child in children {
        let file_name = child.file_name();
        let file_name = file_name
            .to_str()
            .ok_or_else(|| format!("{}: filename is not UTF-8", child.path().display()))?;
        let path = if prefix.is_empty() {
            file_name.to_owned()
        } else {
            format!("{prefix}/{file_name}")
        };
        let path = CoursePath::new(path).map_err(|error| format!("{file_name}: {error}"))?;
        check_path(path.as_str()).map_err(|error| format!("{}: {error:?}", path.as_str()))?;

        let file_type = child
            .file_type()
            .map_err(|error| format!("{}: {error}", child.path().display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "{}: symbolic links are not supported",
                child.path().display()
            ));
        }
        if file_type.is_dir() {
            visit_directory(&child.path(), path.as_str(), entries)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(format!("{}: not a regular file", child.path().display()));
        }
        if entries.len() >= usize::try_from(MAX_FILES_PER_COURSE).unwrap_or(usize::MAX) {
            return Err(format!("course exceeds {MAX_FILES_PER_COURSE} files"));
        }

        let contents = std::fs::read(child.path())
            .map_err(|error| format!("{}: {error}", child.path().display()))?;
        let size = u64::try_from(contents.len()).map_err(|_| "file size exceeds u64")?;
        entries.push(FileEntry {
            path,
            content_hash: sha256(&contents),
            size,
        });
    }
    Ok(())
}

fn scan_directory(directory: &Path) -> Result<Manifest, String> {
    let root = std::fs::canonicalize(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    if !root.is_dir() {
        return Err(format!("{}: not a directory", root.display()));
    }

    let mut entries = Vec::new();
    visit_directory(&root, "", &mut entries)?;
    if entries.is_empty() {
        return Err("course directory contains no files".into());
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    let mut contents = BTreeSet::new();
    let mut total_size = 0_u64;
    for entry in &entries {
        if !contents.insert(entry.content_hash) {
            return Err(format!("duplicate file content: {}", entry.path.as_str()));
        }
        total_size = total_size
            .checked_add(entry.size)
            .ok_or("total file size exceeds u64")?;
    }

    let file_count = u64::try_from(entries.len()).map_err(|_| "file count exceeds u64")?;
    let estimated_cost = course_cost(false, file_count, total_size)
        .ok_or("course cost exceeds the supported range")?;
    let root = course_root(&entries);
    Ok(Manifest {
        entries,
        root,
        total_size,
        estimated_cost,
    })
}

fn parse_network(value: &str) -> Result<Network, String> {
    match value {
        "devnet" => Ok(Network::Devnet),
        "regtest" => Ok(Network::Regtest),
        other => Err(format!("unsupported network {other}")),
    }
}

fn parse_hash(value: &str) -> Result<Hash, String> {
    if value.len() != 64 {
        return Err("hash must be 64 hexadecimal characters".into());
    }
    let mut bytes = [0_u8; 32];
    let (pairs, _) = value.as_bytes().as_chunks::<2>();
    for (byte, pair) in bytes.iter_mut().zip(pairs) {
        let pair = std::str::from_utf8(pair).map_err(|_| "hash is not valid hex")?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| "hash is not valid hex")?;
    }
    Ok(Hash(bytes))
}

fn default_datadir(network: Network) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set; use --datadir")?;
    Ok(PathBuf::from(home).join(".yuanlai").join(network.name()))
}

fn load_chain(network: Network, datadir: &Path) -> Result<Chain, String> {
    let genesis = yuanlai_consensus::genesis::genesis_block(network)
        .ok_or("genesis block not defined for this network")?;
    let mut chain = Chain::new(network.params(), &genesis).map_err(|error| error.to_string())?;
    let blocks = read_blocks(&datadir.join("blocks.dat")).map_err(|error| error.to_string())?;
    for block in blocks {
        chain.connect(&block).map_err(|error| {
            format!(
                "stored block at height {} is invalid: {error}",
                block.header.height
            )
        })?;
    }
    Ok(chain)
}

fn read_secret(path: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{}: key path must be a regular file",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "{}: key file must be accessible only by its owner (chmod 600)",
                path.display()
            ));
        }
    }
    let bytes = Zeroizing::new(
        std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
    );
    let secret = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
        format!(
            "{}: key file must contain exactly 32 raw bytes",
            path.display()
        )
    })?;
    Ok(Zeroizing::new(secret))
}

fn sign_transaction(tx: &mut Tx, secret: &[u8; 32], chain_id: &Hash) -> Result<(), String> {
    let sender = tx.sender().ok_or("system transactions cannot be signed")?;
    if sender != PubKey(public_key(secret)) {
        return Err("transaction sender does not match the key file".into());
    }
    let message = tx
        .sig_msg(chain_id)
        .ok_or("transaction has no signing message")?;
    if !tx.set_signature(Signature(sign(secret, message.as_bytes()))) {
        return Err("transaction signature could not be set".into());
    }
    Ok(())
}

fn build_identity_transaction(
    chain: &Chain,
    network: Network,
    secret: &[u8; 32],
    commitment: Hash,
) -> Result<Tx, String> {
    let owner = PubKey(public_key(secret));
    if chain.state().identity(&owner).is_some() {
        return Err("key already has a registered identity on this chain".into());
    }
    if chain.state().identity_owner(&commitment).is_some() {
        return Err("identity commitment is already registered".into());
    }
    if !is_canonical_field_element(commitment.as_bytes()) {
        return Err("identity commitment is outside the Pallas base field".into());
    }
    let height = chain
        .height()
        .checked_add(1)
        .ok_or("chain height overflow")?;
    let inputs = chain
        .state()
        .select_inputs(&owner, COST_IDENTITY, height, |_| true)
        .ok_or_else(|| format!("not enough spendable credits to pay {}", COST_IDENTITY.0))?;
    let inputs = BoundedVec::<OutPoint, MAX_INPUTS>::new(inputs)
        .map_err(|error| format!("selected credits need too many inputs: {error}"))?;
    let mut transaction = Tx::RegisterIdentity(UserTx {
        sender: owner,
        nonce: chain.state().nonce(&owner),
        body: RegisterIdentity { inputs, commitment },
        signature: Signature::EMPTY,
    });
    sign_transaction(&mut transaction, secret, &network.chain_id())?;
    let mut mempool = Mempool::default();
    mempool
        .submit(chain, owner, transaction.clone())
        .map_err(|error| format!("identity registration failed preflight: {error}"))?;
    Ok(transaction)
}

fn build_author_transaction(
    chain: &Chain,
    network: Network,
    signing_secret: &[u8; 32],
    identity_secret: &[u8; 32],
    salt: &[u8; 32],
    challenge: Hash,
    course_root: Hash,
) -> Result<Tx, String> {
    let owner = PubKey(public_key(signing_secret));
    let commitment = identity_commitment(identity_secret, salt)
        .map_err(|error| format!("identity witness is invalid: {error}"))?;
    if chain.state().identity(&owner) != Some(commitment) {
        return Err("identity witness does not match the key's registered commitment".into());
    }
    if chain
        .state()
        .courses()
        .course_by_owner_root(&owner, &course_root)
        .is_none()
    {
        return Err("course root is not owned by this key on this chain".into());
    }

    let height = chain
        .height()
        .checked_add(1)
        .ok_or("chain height overflow")?;
    let inputs = chain
        .state()
        .select_inputs(&owner, COST_VERIFY_AUTHOR, height, |_| true)
        .ok_or_else(|| {
            format!(
                "not enough spendable credits to pay {}",
                COST_VERIFY_AUTHOR.0
            )
        })?;
    let inputs = BoundedVec::<OutPoint, MAX_INPUTS>::new(inputs)
        .map_err(|error| format!("selected credits need too many inputs: {error}"))?;
    let parameters = setup(IDENTITY_CIRCUIT_K).map_err(|error| error.to_string())?;
    let proof = prove_author(
        &parameters,
        identity_secret,
        salt,
        &challenge,
        &course_root,
        UnwrapErr(SysRng),
    )
    .map_err(|error| error.to_string())?;
    let proof_bytes = BoundedBytes::new(proof.proof)
        .map_err(|error| format!("author proof exceeds protocol limit: {error}"))?;
    let mut transaction = Tx::VerifyAuthor(UserTx {
        sender: owner,
        nonce: chain.state().nonce(&owner),
        body: VerifyAuthor {
            inputs,
            course_root,
            challenge,
            nullifier: proof.nullifier,
            proof: proof_bytes,
        },
        signature: Signature::EMPTY,
    });
    sign_transaction(&mut transaction, signing_secret, &network.chain_id())?;
    let mut mempool = Mempool::default();
    mempool
        .submit(chain, owner, transaction.clone())
        .map_err(|error| format!("author verification failed preflight: {error}"))?;
    Ok(transaction)
}

fn registration_cost(
    chain: &Chain,
    owner: PubKey,
    manifest: &Manifest,
    parent_root: Option<Hash>,
) -> Result<(u32, u64, Credit), String> {
    if let Some(parent_root) = parent_root {
        let parent = chain
            .state()
            .courses()
            .course_by_owner_root(&owner, &parent_root)
            .ok_or("parent root is not owned by this key")?;
        if chain
            .state()
            .courses()
            .successor(&owner, &parent_root)
            .is_some()
        {
            return Err("parent root is not the latest version".into());
        }
        let parent_id = parent.id;
        let parent_contents: BTreeSet<Hash> = chain
            .state()
            .courses()
            .course_files(&parent_id)
            .iter()
            .map(|entry| entry.content_hash)
            .collect();
        let changed: Vec<&FileEntry> = manifest
            .entries
            .iter()
            .filter(|entry| !parent_contents.contains(&entry.content_hash))
            .collect();
        if changed.is_empty() {
            return Err("new version must contain at least one changed file".into());
        }
        let changed_bytes = changed
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.size))
            .ok_or("changed file size sum exceeds u64")?;
        let changed_files = u32::try_from(changed.len())
            .map_err(|_| "changed file count exceeds the protocol limit")?;
        let locked = course_cost(true, u64::from(changed_files), changed_bytes)
            .ok_or("course update cost exceeds the supported range")?;
        Ok((changed_files, changed_bytes, locked))
    } else {
        let file_count = u32::try_from(manifest.entries.len())
            .map_err(|_| "file count exceeds the protocol limit")?;
        Ok((file_count, manifest.total_size, manifest.estimated_cost))
    }
}

fn build_registration_transactions(
    chain: &Chain,
    network: Network,
    secret: &[u8; 32],
    manifest: &Manifest,
    title: &str,
    version: &str,
    parent_root: Option<Hash>,
) -> Result<(Vec<Tx>, Credit), String> {
    let owner = PubKey(public_key(secret));
    if chain.state().identity(&owner).is_none() {
        return Err("key has no registered identity on this chain".into());
    }
    if chain.state().courses().open_session(&owner).is_some() {
        return Err("key already has an open course session".into());
    }
    let (declared_files, declared_bytes, locked) =
        registration_cost(chain, owner, manifest, parent_root)?;
    let height = chain
        .height()
        .checked_add(1)
        .ok_or("chain height overflow")?;
    let inputs = chain
        .state()
        .select_inputs(&owner, locked, height, |utxo| utxo.renewals < MAX_RENEWALS)
        .ok_or_else(|| format!("not enough spendable credits to lock {}", locked.0))?;
    let inputs = BoundedVec::<OutPoint, MAX_INPUTS>::new(inputs)
        .map_err(|error| format!("selected credits need too many inputs: {error}"))?;
    let mut nonce = chain.state().nonce(&owner);
    let mut begin = Tx::BeginCourse(UserTx {
        sender: owner,
        nonce,
        body: BeginCourse {
            inputs,
            parent_root,
            title: CourseTitle::new(title.to_owned()).map_err(|error| error.to_string())?,
            version: CourseVersion::new(version.to_owned()).map_err(|error| error.to_string())?,
            declared_files,
            declared_bytes,
            lock: locked,
        },
        signature: Signature::EMPTY,
    });
    sign_transaction(&mut begin, secret, &network.chain_id())?;
    let session_id = begin.txid();
    let mut transactions = vec![begin];

    let append_limit = usize::try_from(MAX_FILES_PER_APPEND)
        .map_err(|_| "append file limit does not fit this platform")?;
    for entries in manifest.entries.chunks(append_limit) {
        nonce = nonce.checked_add(1).ok_or("account nonce overflow")?;
        let mut append = Tx::AppendFiles(UserTx {
            sender: owner,
            nonce,
            body: AppendFiles {
                session_id,
                entries: BoundedVec::new(entries.to_vec())
                    .map_err(|error| format!("append exceeds protocol limit: {error}"))?,
            },
            signature: Signature::EMPTY,
        });
        sign_transaction(&mut append, secret, &network.chain_id())?;
        transactions.push(append);
    }

    nonce = nonce.checked_add(1).ok_or("account nonce overflow")?;
    let mut finalize = Tx::FinalizeCourse(UserTx {
        sender: owner,
        nonce,
        body: SessionRef { session_id },
        signature: Signature::EMPTY,
    });
    sign_transaction(&mut finalize, secret, &network.chain_id())?;
    transactions.push(finalize);

    let per_block = usize::try_from(MAX_USER_TXS_PER_SENDER_PER_BLOCK)
        .map_err(|_| "per-block transaction limit does not fit this platform")?;
    let first_group = transactions
        .chunks(per_block)
        .next()
        .ok_or("registration produced no transactions")?;
    let mut mempool = Mempool::default();
    for transaction in first_group {
        mempool
            .submit(chain, owner, transaction.clone())
            .map_err(|error| format!("registration transactions failed preflight: {error}"))?;
    }
    Ok((transactions, locked))
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn write_submission_groups(
    out_dir: &Path,
    label: &str,
    network: Network,
    datadir: &Path,
    owner: PubKey,
    groups: &[Vec<Tx>],
) -> Result<(), String> {
    let mut beneficiary = String::with_capacity(64);
    for byte in owner.0 {
        write!(&mut beneficiary, "{byte:02x}")
            .map_err(|_| "could not format beneficiary public key")?;
    }
    let output_dir = out_dir.join(label);
    println!("submission groups\t{}", groups.len());
    for (group_index, group) in groups.iter().enumerate() {
        let group_number = group_index
            .checked_add(1)
            .ok_or("submission group number overflow")?;
        let group_dir = output_dir.join(format!("block-{group_number:03}"));
        std::fs::create_dir_all(&group_dir)
            .map_err(|error| format!("{}: {error}", group_dir.display()))?;
        let mut command = format!(
            "cargo run --release -p yuanlai-node -- --network {} --datadir {} --mine --beneficiary {} --blocks 1",
            network.name(),
            shell_quote(&datadir.display().to_string()),
            beneficiary
        );
        for (transaction_index, transaction) in group.iter().enumerate() {
            let transaction_number = transaction_index
                .checked_add(1)
                .ok_or("transaction file number overflow")?;
            let path = group_dir.join(format!("tx-{transaction_number:03}.bin"));
            std::fs::write(&path, transaction.encode())
                .map_err(|error| format!("{}: {error}", path.display()))?;
            command.push_str(" --tx-file ");
            command.push_str(&shell_quote(&path.display().to_string()));
        }
        println!("group {group_number} files\t{}", group.len());
        println!("submit\t{command}");
    }
    Ok(())
}

fn register(args: &[String]) -> Result<(), String> {
    let mut network = Network::Devnet;
    let mut datadir = None;
    let mut course_dir = None;
    let mut title = None;
    let mut version = None;
    let mut parent_root = None;
    let mut key_file = None;
    let mut out_dir = None;
    let mut args = args.iter();
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{option} needs a value"))?;
        match option.as_str() {
            "--network" => network = parse_network(value)?,
            "--datadir" => datadir = Some(PathBuf::from(value)),
            "--course-dir" => course_dir = Some(PathBuf::from(value)),
            "--title" => title = Some(value.as_str()),
            "--version" => version = Some(value.as_str()),
            "--parent-root" => parent_root = Some(parse_hash(value)?),
            "--key-file" => key_file = Some(PathBuf::from(value)),
            "--out-dir" => out_dir = Some(PathBuf::from(value)),
            other => return Err(format!("unknown register option {other}\n{USAGE}")),
        }
    }
    let course_dir = course_dir.ok_or("register requires --course-dir")?;
    let title = title.ok_or("register requires --title")?;
    let version = version.ok_or("register requires --version")?;
    let key_file = key_file.ok_or("register requires --key-file")?;
    let out_dir = out_dir.ok_or("register requires --out-dir")?;
    let datadir = match datadir {
        Some(path) => path,
        None => default_datadir(network)?,
    };

    let manifest = scan_directory(&course_dir)?;
    let secret = read_secret(&key_file)?;
    let chain = load_chain(network, &datadir)?;
    let owner = PubKey(public_key(&secret));
    let (transactions, locked) = build_registration_transactions(
        &chain,
        network,
        &secret,
        &manifest,
        title,
        version,
        parent_root,
    )?;
    let per_block = usize::try_from(MAX_USER_TXS_PER_SENDER_PER_BLOCK)
        .map_err(|_| "per-block transaction limit does not fit this platform")?;
    let groups: Vec<Vec<Tx>> = transactions.chunks(per_block).map(<[Tx]>::to_vec).collect();
    println!("course root\t{}", manifest.root);
    println!("locked credit\t{}", locked.0);
    write_submission_groups(
        &out_dir,
        &manifest.root.to_string(),
        network,
        &datadir,
        owner,
        &groups,
    )
}

fn identity_register(args: &[String]) -> Result<(), String> {
    let mut network = Network::Devnet;
    let mut datadir = None;
    let mut commitment = None;
    let mut identity_secret_file = None;
    let mut salt_file = None;
    let mut key_file = None;
    let mut out_dir = None;
    let mut args = args.iter();
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{option} needs a value"))?;
        match option.as_str() {
            "--network" => network = parse_network(value)?,
            "--datadir" => datadir = Some(PathBuf::from(value)),
            "--commitment" => commitment = Some(parse_hash(value)?),
            "--identity-secret-file" => identity_secret_file = Some(PathBuf::from(value)),
            "--salt-file" => salt_file = Some(PathBuf::from(value)),
            "--key-file" => key_file = Some(PathBuf::from(value)),
            "--out-dir" => out_dir = Some(PathBuf::from(value)),
            other => return Err(format!("unknown identity-register option {other}\n{USAGE}")),
        }
    }
    let commitment = match (commitment, identity_secret_file, salt_file) {
        (Some(commitment), None, None) => commitment,
        (None, Some(identity_secret_file), Some(salt_file)) => {
            let identity_secret = read_secret(&identity_secret_file)?;
            let salt = read_secret(&salt_file)?;
            identity_commitment(&identity_secret, &salt)
                .map_err(|error| format!("identity witness is invalid: {error}"))?
        }
        _ => {
            return Err(
                "identity-register requires --commitment or both --identity-secret-file and --salt-file"
                    .into(),
            );
        }
    };
    let key_file = key_file.ok_or("identity-register requires --key-file")?;
    let out_dir = out_dir.ok_or("identity-register requires --out-dir")?;
    let datadir = match datadir {
        Some(path) => path,
        None => default_datadir(network)?,
    };

    let secret = read_secret(&key_file)?;
    let chain = load_chain(network, &datadir)?;
    let owner = PubKey(public_key(&secret));
    let transaction = build_identity_transaction(&chain, network, &secret, commitment)?;
    println!("identity commitment\t{commitment}");
    println!("registration cost\t{}", COST_IDENTITY.0);
    write_submission_groups(
        &out_dir,
        &format!("identity-{commitment}"),
        network,
        &datadir,
        owner,
        &[vec![transaction]],
    )
}

fn author_verify(args: &[String]) -> Result<(), String> {
    let mut network = Network::Devnet;
    let mut datadir = None;
    let mut course_root = None;
    let mut challenge = None;
    let mut identity_secret_file = None;
    let mut salt_file = None;
    let mut key_file = None;
    let mut out_dir = None;
    let mut args = args.iter();
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{option} needs a value"))?;
        match option.as_str() {
            "--network" => network = parse_network(value)?,
            "--datadir" => datadir = Some(PathBuf::from(value)),
            "--course-root" => course_root = Some(parse_hash(value)?),
            "--challenge" => challenge = Some(parse_hash(value)?),
            "--identity-secret-file" => identity_secret_file = Some(PathBuf::from(value)),
            "--salt-file" => salt_file = Some(PathBuf::from(value)),
            "--key-file" => key_file = Some(PathBuf::from(value)),
            "--out-dir" => out_dir = Some(PathBuf::from(value)),
            other => return Err(format!("unknown author-verify option {other}\n{USAGE}")),
        }
    }
    let course_root = course_root.ok_or("author-verify requires --course-root")?;
    let challenge = challenge.ok_or("author-verify requires --challenge")?;
    let identity_secret_file =
        identity_secret_file.ok_or("author-verify requires --identity-secret-file")?;
    let salt_file = salt_file.ok_or("author-verify requires --salt-file")?;
    let key_file = key_file.ok_or("author-verify requires --key-file")?;
    let out_dir = out_dir.ok_or("author-verify requires --out-dir")?;
    let datadir = match datadir {
        Some(path) => path,
        None => default_datadir(network)?,
    };

    let identity_secret = read_secret(&identity_secret_file)?;
    let salt = read_secret(&salt_file)?;
    let signing_secret = read_secret(&key_file)?;
    let chain = load_chain(network, &datadir)?;
    let owner = PubKey(public_key(&signing_secret));
    let transaction = build_author_transaction(
        &chain,
        network,
        &signing_secret,
        &identity_secret,
        &salt,
        challenge,
        course_root,
    )?;
    let Tx::VerifyAuthor(verification) = &transaction else {
        return Err("author verification builder returned an unexpected transaction".into());
    };
    println!("course root\t{course_root}");
    println!("challenge\t{challenge}");
    println!("nullifier\t{}", verification.body.nullifier);
    println!("verification cost\t{}", COST_VERIFY_AUTHOR.0);
    write_submission_groups(
        &out_dir,
        &format!("author-{}", verification.body.nullifier),
        network,
        &datadir,
        owner,
        &[vec![transaction]],
    )
}

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [command, directory] if command == "scan" => {
            let manifest = scan_directory(Path::new(directory))?;
            println!("root\t{}", manifest.root);
            println!("files\t{}", manifest.entries.len());
            println!("bytes\t{}", manifest.total_size);
            println!("estimated_cost\t{}", manifest.estimated_cost.0);
            for entry in manifest.entries {
                println!(
                    "file\t{}\t{}\t{}",
                    entry.path.as_str(),
                    entry.size,
                    entry.content_hash
                );
            }
            Ok(())
        }
        [command, rest @ ..] if command == "register" => register(rest),
        [command, rest @ ..] if command == "identity-register" => identity_register(rest),
        [command, rest @ ..] if command == "author-verify" => author_verify(rest),
        [option] if option == "--version" => {
            println!("yuanlai {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [] => {
            println!("yuanlai {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [option] if option == "--help" || option == "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use yuanlai_codec::BoundedBytes;
    use yuanlai_consensus::{Chain, course_cost, course_root, genesis::genesis_block};
    use yuanlai_crypto::{Hash, public_key, sha256};
    use yuanlai_types::{CoursePath, Credit, FileEntry, Network, PubKey, Signature, Tx};

    use super::{
        Manifest, build_author_transaction, build_identity_transaction,
        build_registration_transactions, identity_commitment, parse_hash, read_secret,
        scan_directory,
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("yuanlai-cli-{}-{id}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(path)
    }

    fn connect_regtest_block(
        chain: &mut Chain,
        beneficiary: PubKey,
        transactions: Vec<Tx>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut block = chain
            .template_with(
                beneficiary,
                BoundedBytes::empty(),
                chain.min_next_timestamp(),
                transactions,
            )
            .ok_or_else(|| std::io::Error::other("could not build regtest template"))?;
        block.header = yuanlai_miner::solve(&block.header, 0..=10_000)
            .ok_or_else(|| std::io::Error::other("could not solve regtest block"))?;
        chain.connect(&block)?;
        Ok(())
    }

    struct IdentityFixture {
        chain: Chain,
        signing_secret: [u8; 32],
        identity_secret: [u8; 32],
        salt: [u8; 32],
    }

    fn chain_with_registered_identity() -> Result<IdentityFixture, Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let genesis = genesis_block(network)
            .ok_or_else(|| std::io::Error::other("regtest genesis is missing"))?;
        let mut chain = Chain::new(network.params(), &genesis)?;
        let secret = [
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ];
        let identity_secret = [0x11; 32];
        let salt = [0x1d; 32];
        let commitment = identity_commitment(&identity_secret, &salt)?;
        let owner = PubKey(public_key(&secret));
        for _ in 0..12 {
            connect_regtest_block(&mut chain, owner, Vec::new())?;
        }

        let identity = build_identity_transaction(&chain, network, &secret, commitment)
            .map_err(std::io::Error::other)?;
        connect_regtest_block(&mut chain, owner, vec![identity])?;
        for _ in 0..3 {
            connect_regtest_block(&mut chain, owner, Vec::new())?;
        }
        Ok(IdentityFixture {
            chain,
            signing_secret: secret,
            identity_secret,
            salt,
        })
    }

    fn connect_author_verification(
        chain: &mut Chain,
        signing_secret: &[u8; 32],
        identity_secret: &[u8; 32],
        salt: &[u8; 32],
        course_root: Hash,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let challenge = Hash([0x77; 32]);
        let wrong_secret = [0x12; 32];
        let error = build_author_transaction(
            chain,
            network,
            signing_secret,
            &wrong_secret,
            salt,
            challenge,
            course_root,
        )
        .err()
        .ok_or("mismatched identity witness was accepted")?;
        assert!(error.contains("does not match the key's registered commitment"));

        let transaction = build_author_transaction(
            chain,
            network,
            signing_secret,
            identity_secret,
            salt,
            challenge,
            course_root,
        )
        .map_err(std::io::Error::other)?;
        let Tx::VerifyAuthor(verification) = &transaction else {
            return Err(std::io::Error::other("unexpected author transaction type").into());
        };
        assert_eq!(verification.body.course_root, course_root);
        assert_eq!(verification.body.challenge, challenge);
        assert!(!verification.body.proof.is_empty());
        assert_ne!(verification.signature, Signature::EMPTY);
        connect_regtest_block(chain, PubKey(public_key(signing_secret)), vec![transaction])?;
        Ok(())
    }

    fn update_registered_course(
        chain: &mut Chain,
        signing_secret: &[u8; 32],
        manifest: &Manifest,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut update_entries = manifest.entries.clone();
        let changed_entry = update_entries
            .first_mut()
            .ok_or_else(|| std::io::Error::other("course has no entries"))?;
        changed_entry.content_hash = sha256(b"revised lesson");
        changed_entry.size = 15;
        let update_manifest = Manifest {
            root: course_root(&update_entries),
            estimated_cost: course_cost(false, 257, 271)
                .ok_or_else(|| std::io::Error::other("updated manifest cost overflow"))?,
            entries: update_entries,
            total_size: 271,
        };
        let (transactions, locked) = build_registration_transactions(
            chain,
            Network::Regtest,
            signing_secret,
            &update_manifest,
            "Course",
            "2.0",
            Some(manifest.root),
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(locked, Credit(21));
        connect_regtest_block(chain, PubKey(public_key(signing_secret)), transactions)?;
        let successor = chain
            .state()
            .courses()
            .successor(&PubKey(public_key(signing_secret)), &manifest.root)
            .ok_or_else(|| std::io::Error::other("updated course is not linked"))?;
        assert_eq!(successor.root, update_manifest.root);
        assert_eq!(successor.parent_root, Some(manifest.root));
        Ok(())
    }

    #[test]
    fn identity_registration_preflights_and_rejects_noncanonical_commitments()
    -> Result<(), Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let genesis = genesis_block(network)
            .ok_or_else(|| std::io::Error::other("regtest genesis is missing"))?;
        let mut chain = Chain::new(network.params(), &genesis)?;
        let secret = [0x42; 32];
        let owner = PubKey(public_key(&secret));
        for _ in 0..12 {
            connect_regtest_block(&mut chain, owner, Vec::new())?;
        }

        let error = build_identity_transaction(&chain, network, &secret, Hash([0xff; 32]))
            .err()
            .ok_or("noncanonical identity commitment was accepted")?;
        assert!(error.contains("outside the Pallas base field"));

        let commitment = Hash([1; 32]);
        let transaction = build_identity_transaction(&chain, network, &secret, commitment)
            .map_err(std::io::Error::other)?;
        let Tx::RegisterIdentity(identity) = &transaction else {
            return Err(std::io::Error::other("unexpected identity transaction type").into());
        };
        assert_eq!(identity.sender, owner);
        assert_eq!(identity.body.commitment, commitment);
        assert_ne!(identity.signature, Signature::EMPTY);
        connect_regtest_block(&mut chain, owner, vec![transaction])?;
        assert!(chain.state().identity(&owner).is_some());
        assert!(
            build_identity_transaction(&chain, network, &secret, Hash([2; 32]))
                .err()
                .is_some_and(|error| error.contains("already has a registered identity"))
        );
        Ok(())
    }

    #[test]
    fn scan_builds_sorted_manifest_and_estimate() -> Result<(), Box<dyn std::error::Error>> {
        let directory = temp_dir()?;
        fs::create_dir(directory.join("nested"))?;
        fs::write(directory.join("z.txt"), b"last")?;
        fs::write(directory.join("nested/a.txt"), b"first")?;

        let manifest = scan_directory(&directory).map_err(std::io::Error::other)?;
        let expected = vec![
            FileEntry {
                path: CoursePath::new("nested/a.txt".to_owned())?,
                content_hash: sha256(b"first"),
                size: 5,
            },
            FileEntry {
                path: CoursePath::new("z.txt".to_owned())?,
                content_hash: sha256(b"last"),
                size: 4,
            },
        ];
        assert_eq!(manifest.entries, expected);
        assert_eq!(manifest.root, course_root(&expected));
        assert_eq!(manifest.total_size, 9);

        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn scan_rejects_duplicate_content() -> Result<(), Box<dyn std::error::Error>> {
        let directory = temp_dir()?;
        fs::write(directory.join("one.txt"), b"same")?;
        fs::write(directory.join("two.txt"), b"same")?;

        let error = scan_directory(&directory)
            .err()
            .ok_or("duplicate content was accepted")?;
        assert!(error.contains("duplicate file content"));

        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn register_builds_signed_course_transactions_that_finalize()
    -> Result<(), Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let IdentityFixture {
            mut chain,
            signing_secret: secret,
            identity_secret,
            salt,
        } = chain_with_registered_identity()?;
        for _ in 0..26 {
            connect_regtest_block(&mut chain, PubKey(public_key(&secret)), Vec::new())?;
        }
        let mut entries = Vec::new();
        for index in 0..257 {
            let path = format!("lesson-{index:03}.md");
            entries.push(FileEntry {
                path: CoursePath::new(path.clone())?,
                content_hash: sha256(path.as_bytes()),
                size: 1,
            });
        }
        let manifest = Manifest {
            root: course_root(&entries),
            estimated_cost: course_cost(false, 257, 257)
                .ok_or_else(|| std::io::Error::other("course cost overflow"))?,
            entries,
            total_size: 257,
        };

        let (transactions, locked) = build_registration_transactions(
            &chain, network, &secret, &manifest, "Course", "1.0", None,
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(locked, Credit(292));
        assert_eq!(transactions.len(), 4);
        let session_id = transactions
            .first()
            .ok_or_else(|| std::io::Error::other("begin transaction is missing"))?
            .txid();
        let [
            Tx::BeginCourse(begin),
            Tx::AppendFiles(first_append),
            Tx::AppendFiles(second_append),
            Tx::FinalizeCourse(finalize),
        ] = transactions.as_slice()
        else {
            return Err(
                std::io::Error::other("unexpected registration transaction sequence").into(),
            );
        };
        assert_eq!(begin.nonce, chain.state().nonce(&begin.sender));
        assert_eq!(first_append.body.entries.len(), 256);
        assert_eq!(second_append.body.entries.len(), 1);
        assert_eq!(first_append.body.session_id, session_id);
        assert_eq!(second_append.body.session_id, session_id);
        assert_eq!(finalize.body.session_id, session_id);

        connect_regtest_block(&mut chain, begin.sender, transactions)?;
        assert_eq!(
            chain
                .state()
                .courses()
                .registrations_of_root(&manifest.root)
                .len(),
            1
        );

        connect_author_verification(&mut chain, &secret, &identity_secret, &salt, manifest.root)?;

        update_registered_course(&mut chain, &secret, &manifest)?;
        Ok(())
    }

    #[test]
    fn secret_file_requires_exact_bytes_and_private_permissions()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = temp_dir()?;
        let path = directory.join("owner.key");
        fs::write(&path, [7_u8; 32])?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        let secret = read_secret(&path).map_err(std::io::Error::other)?;
        assert_eq!(*secret, [7_u8; 32]);

        fs::write(&path, [7_u8; 31])?;
        let error = read_secret(&path)
            .err()
            .ok_or("short key file was accepted")?;
        assert!(error.contains("exactly 32 raw bytes"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(&path, [7_u8; 32])?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
            let error = read_secret(&path)
                .err()
                .ok_or("group-readable key file was accepted")?;
            assert!(error.contains("chmod 600"));
        }
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn parent_root_parser_accepts_only_32_byte_hex_hashes() -> Result<(), Box<dyn std::error::Error>>
    {
        let expected = Hash([0xab; 32]);
        assert_eq!(
            parse_hash(&expected.to_string()).map_err(std::io::Error::other)?,
            expected
        );
        assert!(parse_hash("abcd").is_err());
        assert!(parse_hash(&"gg".repeat(32)).is_err());
        Ok(())
    }
}
