//! PAR2 verification and repair

use crate::engine::archive_pipeline::{Par2Outcome, Par2Report};
use crate::engine::archive_safety::{validate_member_path, ArchiveLimits};
use md5::{Digest, Md5};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const PAR2_MAGIC: &[u8; 8] = b"PAR2\0PKT";
const PAR2_HEADER_BYTES: u64 = 64;
const PAR2_TYPE_MAIN: &[u8; 16] = b"PAR 2.0\0Main\0\0\0\0";
const PAR2_TYPE_FILE_DESC: &[u8; 16] = b"PAR 2.0\0FileDesc";
const PAR2_TYPE_IFSC: &[u8; 16] = b"PAR 2.0\0IFSC\0\0\0\0";
const PAR2_TYPE_RECOVERY: &[u8; 16] = b"PAR 2.0\0RecvSlic";
const MAX_PAR2_PACKET_COUNT: u64 = 100_000;
const MAX_PAR2_INPUT_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_PAR2_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_PAR2_SLICE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PAR2_METADATA_PACKET_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PAR2_SOURCE_BLOCKS: u64 = 65_535;
const MAX_PAR2_SOURCE_FILES: u64 = 65_535;
const MAX_PAR2_RECOVERY_BLOCKS: u32 = 4_096;
const MAX_PAR2_REPAIR_BLOCKS: u32 = 1_024;
const PAR2_REPAIR_MATRIX_BYTES_PER_CELL: u64 = 8;
const PAR2_REPAIR_BATCH_BLOCKS: u64 = 24;
const SPARSE_COPY_BUFFER_BYTES: usize = 128 * 1024;
const VERIFY_HASH_BUFFER_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Par2InputFile {
    pub manifest_name: String,
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub expected_size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Par2RepairRequest {
    pub destination: PathBuf,
    pub data_files: Vec<Par2InputFile>,
    pub parity_files: Vec<PathBuf>,
    pub required_incomplete_names: BTreeSet<String>,
    pub limits: ArchiveLimits,
    pub active_started_at: Option<Instant>,
    pub active_elapsed_before_repair: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Par2RepairResult {
    pub report: Par2Report,
    pub promoted_outputs: Vec<PathBuf>,
}

#[derive(Debug)]
pub enum Par2Error {
    MissingParity,
    Malformed(String),
    UnsafePath(String),
    Limits(String),
    InsufficientRecovery { needed: u32, available: u32 },
    Cancelled,
    Repair(String),
    Io(io::Error),
}

impl std::fmt::Display for Par2Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingParity => {
                formatter.write_str("PAR2 repair requires a complete index and recovery volume")
            }
            Self::Malformed(message) => write!(formatter, "malformed PAR2: {message}"),
            Self::UnsafePath(path) => write!(formatter, "unsafe PAR2 filename: {path}"),
            Self::Limits(message) => write!(formatter, "PAR2 safety limit: {message}"),
            Self::InsufficientRecovery { needed, available } => write!(
                formatter,
                "PAR2 recovery is insufficient: need {needed} blocks, have {available}"
            ),
            Self::Cancelled => formatter.write_str("Download cancelled"),
            Self::Repair(message) => write!(formatter, "PAR2 repair failed: {message}"),
            Self::Io(error) => write!(formatter, "PAR2 I/O: {error}"),
        }
    }
}

impl std::error::Error for Par2Error {}

impl From<io::Error> for Par2Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Verify a PAR2 set and repair it in a private stage when necessary
pub fn verify_or_repair(request: &Par2RepairRequest) -> Result<Par2RepairResult, Par2Error> {
    verify_or_repair_with_cancel(request, None)
}

pub fn verify_or_repair_with_cancel(
    request: &Par2RepairRequest,
    cancel: Option<&CancellationToken>,
) -> Result<Par2RepairResult, Par2Error> {
    check_cancel(cancel)?;
    check_active_time(request)?;
    if request.parity_files.is_empty() {
        return Err(Par2Error::MissingParity);
    }
    fs::create_dir_all(&request.destination)?;
    let limits = request.limits;
    let data_by_name = validate_input_files(&request.data_files, limits)?;
    let parity = validate_parity_files(&request.parity_files)?;
    let (file_set, source) = parse_index(
        &parity.files,
        &data_by_name,
        &request.required_incomplete_names,
        limits,
    )?;
    validate_task_budget(request, source, parity.total)?;
    validate_verification_resources(&file_set)?;
    ensure_staging_space(request, source.bytes, parity.total.bytes)?;
    let stage = tempfile::Builder::new()
        .prefix(".risuko-par2-")
        .tempdir_in(&request.destination)
        .map_err(Par2Error::Io)?;
    let probe = stage.path().join("probe");
    fs::create_dir(&probe)?;
    stage_parity_files(&probe, &request.parity_files, cancel)?;
    stage_data_files(&probe, &file_set, &data_by_name, None, cancel)?;

    check_cancel(cancel)?;
    check_active_time(request)?;
    let verification = rust_par2::verify(&file_set, &probe);
    check_cancel(cancel)?;
    check_active_time(request)?;
    if verification.all_correct() {
        let names = file_set_names(&file_set)?;
        let promoted_outputs = promote_outputs(&probe, &names, &data_by_name, cancel)?;
        return Ok(Par2RepairResult {
            report: Par2Report {
                outcome: Par2Outcome::Verified,
                recovered_bytes: 0,
            },
            promoted_outputs,
        });
    }
    validate_repair_resources(&verification, &file_set)?;
    if !verification.repair_possible {
        return Err(Par2Error::InsufficientRecovery {
            needed: verification.blocks_needed(),
            available: verification.recovery_blocks_available,
        });
    }

    let intact: HashSet<&str> = verification
        .intact
        .iter()
        .map(|file| file.filename.as_str())
        .collect();
    fs::remove_dir_all(&probe)?;
    let repair_dir = stage.path().join("repair");
    fs::create_dir(&repair_dir)?;
    stage_parity_files(&repair_dir, &request.parity_files, cancel)?;
    stage_data_files(&repair_dir, &file_set, &data_by_name, Some(&intact), cancel)?;
    check_cancel(cancel)?;
    check_active_time(request)?;
    let repair = rust_par2::repair_from_verify(&file_set, &repair_dir, &verification)
        .map_err(|error| Par2Error::Repair(error.to_string()))?;
    check_cancel(cancel)?;
    check_active_time(request)?;
    let post_repair = rust_par2::verify(&file_set, &repair_dir);
    check_active_time(request)?;
    if !post_repair.all_correct() {
        return Err(Par2Error::Repair(format!(
            "post-repair verification failed: {post_repair}"
        )));
    }

    let changed_names: BTreeSet<String> = verification
        .damaged
        .iter()
        .map(|file| file.filename.clone())
        .chain(
            verification
                .missing
                .iter()
                .map(|file| file.filename.clone()),
        )
        .collect();
    let promoted_outputs = promote_outputs(&repair_dir, &changed_names, &data_by_name, cancel)?;
    Ok(Par2RepairResult {
        report: Par2Report {
            outcome: Par2Outcome::Repaired,
            recovered_bytes: (repair.blocks_repaired as u64).saturating_mul(file_set.slice_size),
        },
        promoted_outputs,
    })
}

fn check_cancel(cancel: Option<&CancellationToken>) -> Result<(), Par2Error> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        Err(Par2Error::Cancelled)
    } else {
        Ok(())
    }
}

fn check_active_time(request: &Par2RepairRequest) -> Result<(), Par2Error> {
    let repair_elapsed = request
        .active_started_at
        .map(|started| started.elapsed())
        .unwrap_or_default();
    let active_elapsed = request
        .active_elapsed_before_repair
        .unwrap_or_default()
        .checked_add(repair_elapsed)
        .unwrap_or(Duration::MAX);
    if active_elapsed.as_secs() > request.limits.max_active_seconds {
        return Err(Par2Error::Limits(
            "PAR2 repair exceeded the active-time limit".into(),
        ));
    }
    Ok(())
}

pub fn platform_limits() -> ArchiveLimits {
    if cfg!(target_os = "android") {
        ArchiveLimits::android_defaults()
    } else {
        ArchiveLimits::desktop_defaults()
    }
}

fn repair_memory_limit() -> u64 {
    if cfg!(target_os = "android") {
        128 * 1024 * 1024
    } else {
        512 * 1024 * 1024
    }
}

fn metadata_memory_limit() -> u64 {
    if cfg!(target_os = "android") {
        16 * 1024 * 1024
    } else {
        64 * 1024 * 1024
    }
}

#[derive(Debug, Clone, Copy)]
struct ParityStats {
    bytes: u64,
    packets: u64,
    recovery_blocks: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct ParityFileStats {
    packets: u64,
    recovery_blocks: u64,
    metadata_bytes: u64,
    source_file_descriptors: u64,
    source_blocks: u64,
}

#[derive(Debug, Clone)]
struct ValidatedParityFile {
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct ValidatedParityFiles {
    total: ParityStats,
    files: Vec<ValidatedParityFile>,
}

#[derive(Debug, Clone, Copy)]
struct SourceStats {
    bytes: u64,
}

fn validate_task_budget(
    request: &Par2RepairRequest,
    source: SourceStats,
    parity: ParityStats,
) -> Result<(), Par2Error> {
    let total_entries = (request.data_files.len() as u64)
        .checked_add(request.parity_files.len() as u64)
        .ok_or_else(|| Par2Error::Limits("PAR2 entry count overflowed".into()))?;
    if total_entries > request.limits.max_entries {
        return Err(Par2Error::Limits("too many data and PAR2 files".into()));
    }
    if parity.packets > MAX_PAR2_PACKET_COUNT {
        return Err(Par2Error::Limits("too many PAR2 packets".into()));
    }
    if parity.recovery_blocks > u64::from(MAX_PAR2_RECOVERY_BLOCKS) {
        return Err(Par2Error::Limits(
            "PAR2 recovery set has too many blocks for in-process repair".into(),
        ));
    }
    if parity.bytes > MAX_PAR2_INPUT_BYTES {
        return Err(Par2Error::Limits("parity input is too large".into()));
    }
    let total_bytes = source
        .bytes
        .checked_add(parity.bytes)
        .ok_or_else(|| Par2Error::Limits("PAR2 task size overflowed".into()))?;
    if total_bytes > request.limits.max_expanded_bytes {
        return Err(Par2Error::Limits(
            "data and PAR2 files exceed the task-wide limit".into(),
        ));
    }
    Ok(())
}

fn ensure_staging_space(
    request: &Par2RepairRequest,
    source_bytes: u64,
    parity_bytes: u64,
) -> Result<(), Par2Error> {
    let staging_bytes = source_bytes
        .checked_add(parity_bytes)
        .ok_or_else(|| Par2Error::Limits("PAR2 staging size overflowed".into()))?;
    let required = staging_bytes
        .checked_add(request.limits.free_space_reserve_bytes)
        .ok_or_else(|| Par2Error::Limits("PAR2 staging reserve overflowed".into()))?;
    let available = fs4::available_space(&request.destination)?;
    if available < required {
        return Err(Par2Error::Limits(
            "insufficient free space for PAR2 staging and reserve".into(),
        ));
    }
    Ok(())
}

fn validate_verification_resources(file_set: &rust_par2::Par2FileSet) -> Result<(), Par2Error> {
    let workers = std::thread::available_parallelism()
        .map(|count| count.get() as u64)
        .unwrap_or(1);
    let per_worker = file_set
        .slice_size
        .checked_add(VERIFY_HASH_BUFFER_BYTES.saturating_mul(2))
        .ok_or_else(|| Par2Error::Limits("PAR2 verification memory overflowed".into()))?;
    let working_set = workers
        .checked_mul(per_worker)
        .ok_or_else(|| Par2Error::Limits("PAR2 verification memory overflowed".into()))?;
    if working_set > repair_memory_limit() {
        return Err(Par2Error::Limits(
            "PAR2 verification would exceed the in-process memory limit".into(),
        ));
    }
    Ok(())
}

fn validate_repair_resources(
    verification: &rust_par2::VerifyResult,
    file_set: &rust_par2::Par2FileSet,
) -> Result<(), Par2Error> {
    // This estimate matches rust-par2 0.1.3: its recovery blocks, repair
    // matrix, paired repair buffers, and verification worker buffers are all
    // live during repair.
    let needed = verification.blocks_needed();
    let available = verification.recovery_blocks_available;
    if needed > MAX_PAR2_REPAIR_BLOCKS || available > MAX_PAR2_RECOVERY_BLOCKS {
        return Err(Par2Error::Limits(
            "PAR2 recovery set has too many blocks for in-process repair".into(),
        ));
    }
    let needed = u64::from(needed);
    let available = u64::from(available);
    let slice_size = file_set.slice_size;
    let matrix_cells = needed
        .checked_mul(needed)
        .ok_or_else(|| Par2Error::Limits("PAR2 repair matrix size overflowed".into()))?;
    let matrix_bytes = matrix_cells
        .checked_mul(PAR2_REPAIR_MATRIX_BYTES_PER_CELL)
        .ok_or_else(|| Par2Error::Limits("PAR2 repair matrix memory overflowed".into()))?;
    let recovery_bytes = available
        .checked_mul(slice_size)
        .ok_or_else(|| Par2Error::Limits("PAR2 recovery memory calculation overflowed".into()))?;
    let repair_buffers = needed
        .checked_mul(slice_size)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| Par2Error::Limits("PAR2 repair buffer calculation overflowed".into()))?;
    let parallelism = std::thread::available_parallelism()
        .map(|count| count.get() as u64)
        .unwrap_or(1);
    let verification_buffers = parallelism
        .checked_mul(2)
        .and_then(|workers| workers.checked_add(PAR2_REPAIR_BATCH_BLOCKS))
        .and_then(|blocks| blocks.checked_mul(slice_size))
        .ok_or_else(|| {
            Par2Error::Limits("PAR2 verification buffer calculation overflowed".into())
        })?;
    let working_set = recovery_bytes
        .checked_add(repair_buffers)
        .and_then(|bytes| bytes.checked_add(matrix_bytes))
        .and_then(|bytes| bytes.checked_add(verification_buffers))
        .ok_or_else(|| Par2Error::Limits("PAR2 repair memory calculation overflowed".into()))?;
    if working_set > repair_memory_limit() {
        return Err(Par2Error::Limits(
            "PAR2 repair would exceed the in-process memory limit".into(),
        ));
    }
    Ok(())
}

fn validate_input_files(
    inputs: &[Par2InputFile],
    limits: ArchiveLimits,
) -> Result<HashMap<String, &Par2InputFile>, Par2Error> {
    let mut by_name = HashMap::new();
    let mut portable_names = HashSet::new();
    for input in inputs {
        validate_simple_filename(&input.manifest_name)?;
        if !portable_names.insert(portable_filename_key(&input.manifest_name)) {
            return Err(Par2Error::Malformed(format!(
                "NZB output name {} collides on case-insensitive filesystems",
                input.manifest_name
            )));
        }
        if input
            .expected_size
            .is_some_and(|size| size > limits.max_entry_bytes)
        {
            return Err(Par2Error::Limits(format!(
                "{} exceeds the per-file limit",
                input.manifest_name
            )));
        }
        ensure_regular_file(&input.source_path)?;
        if fs::metadata(&input.source_path)?.len() > limits.max_entry_bytes {
            return Err(Par2Error::Limits(format!(
                "{} exceeds the per-file limit",
                input.manifest_name
            )));
        }
        if by_name.insert(input.manifest_name.clone(), input).is_some() {
            return Err(Par2Error::Malformed(format!(
                "duplicate NZB output name {}",
                input.manifest_name
            )));
        }
    }
    if by_name.is_empty() {
        return Err(Par2Error::Malformed("no data files were supplied".into()));
    }
    Ok(by_name)
}

fn validate_parity_files(paths: &[PathBuf]) -> Result<ValidatedParityFiles, Par2Error> {
    let mut bytes = 0u64;
    let mut packets = 0u64;
    let mut recovery_blocks = 0u64;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        ensure_regular_file(path)?;
        let size = fs::metadata(path)?.len();
        if size > MAX_PAR2_FILE_BYTES {
            return Err(Par2Error::Limits(format!(
                "{} exceeds the per-file parity limit",
                path.display()
            )));
        }
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| Par2Error::Limits("parity byte count overflowed".into()))?;
        let stats = validate_par2_packets(path)?;
        packets = packets
            .checked_add(stats.packets)
            .ok_or_else(|| Par2Error::Limits("PAR2 packet count overflowed".into()))?;
        recovery_blocks = recovery_blocks
            .checked_add(stats.recovery_blocks)
            .ok_or_else(|| Par2Error::Limits("PAR2 recovery block count overflowed".into()))?;
        files.push(ValidatedParityFile { path: path.clone() });
    }
    if bytes > MAX_PAR2_INPUT_BYTES {
        return Err(Par2Error::Limits("parity input is too large".into()));
    }
    if packets > MAX_PAR2_PACKET_COUNT {
        return Err(Par2Error::Limits("too many PAR2 packets".into()));
    }
    if recovery_blocks > u64::from(MAX_PAR2_RECOVERY_BLOCKS) {
        return Err(Par2Error::Limits(
            "PAR2 recovery set has too many blocks for in-process repair".into(),
        ));
    }
    Ok(ValidatedParityFiles {
        total: ParityStats {
            bytes,
            packets,
            recovery_blocks,
        },
        files,
    })
}

fn validate_par2_packets(path: &Path) -> Result<ParityFileStats, Par2Error> {
    let file_size = fs::metadata(path)?.len();
    if file_size < PAR2_HEADER_BYTES {
        return Err(Par2Error::Malformed(format!(
            "{} is smaller than one PAR2 packet",
            path.display()
        )));
    }
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut position = 0u64;
    let mut stats = ParityFileStats::default();
    let mut recovery_set_id = None;
    let mut buffer = vec![0u8; SPARSE_COPY_BUFFER_BYTES];
    while position < file_size {
        if file_size - position < PAR2_HEADER_BYTES {
            return Err(Par2Error::Malformed(format!(
                "{} ends in a truncated packet header",
                path.display()
            )));
        }
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        let mut length_bytes = [0u8; 8];
        reader.read_exact(&mut length_bytes)?;
        let packet_len = u64::from_le_bytes(length_bytes);
        if magic != *PAR2_MAGIC
            || packet_len < PAR2_HEADER_BYTES
            || packet_len % 4 != 0
            || packet_len > file_size - position
            || packet_len > MAX_PAR2_SLICE_BYTES + PAR2_HEADER_BYTES
        {
            return Err(Par2Error::Malformed(format!(
                "{} contains an invalid PAR2 packet length",
                path.display()
            )));
        }
        let mut expected_md5 = [0u8; 16];
        reader.read_exact(&mut expected_md5)?;
        let mut hasher = Md5::new();
        let mut packet_prefix = [0u8; 32];
        reader.read_exact(&mut packet_prefix)?;
        hasher.update(packet_prefix);
        let packet_set_id: [u8; 16] = packet_prefix[..16]
            .try_into()
            .map_err(|_| Par2Error::Malformed("invalid PAR2 recovery set id".into()))?;
        if let Some(expected) = recovery_set_id {
            if expected != packet_set_id {
                return Err(Par2Error::Malformed(format!(
                    "{} mixes PAR2 recovery set IDs",
                    path.display()
                )));
            }
        } else {
            recovery_set_id = Some(packet_set_id);
        }
        let packet_type: [u8; 16] = packet_prefix[16..]
            .try_into()
            .map_err(|_| Par2Error::Malformed("invalid PAR2 packet type".into()))?;
        let body_len = packet_len - PAR2_HEADER_BYTES;
        let mut remaining = body_len;

        if packet_type == *PAR2_TYPE_RECOVERY {
            if body_len < 4 {
                return Err(Par2Error::Malformed(format!(
                    "{} contains a truncated PAR2 recovery packet",
                    path.display()
                )));
            }
            stats.recovery_blocks = stats
                .recovery_blocks
                .checked_add(1)
                .ok_or_else(|| Par2Error::Limits("PAR2 recovery block count overflowed".into()))?;
        } else {
            if packet_len > MAX_PAR2_METADATA_PACKET_BYTES {
                return Err(Par2Error::Limits(format!(
                    "{} contains an oversized PAR2 metadata packet",
                    path.display()
                )));
            }
            stats.metadata_bytes = stats
                .metadata_bytes
                .checked_add(packet_len)
                .ok_or_else(|| Par2Error::Limits("PAR2 metadata byte count overflowed".into()))?;
            if stats.metadata_bytes > metadata_memory_limit() {
                return Err(Par2Error::Limits(format!(
                    "{} contains too much PAR2 metadata",
                    path.display()
                )));
            }
            if packet_type == *PAR2_TYPE_FILE_DESC {
                if body_len < 56 {
                    return Err(Par2Error::Malformed(format!(
                        "{} contains a truncated PAR2 file description",
                        path.display()
                    )));
                }
                stats.source_file_descriptors = stats
                    .source_file_descriptors
                    .checked_add(1)
                    .ok_or_else(|| Par2Error::Limits("PAR2 source file count overflowed".into()))?;
                if stats.source_file_descriptors > MAX_PAR2_SOURCE_FILES {
                    return Err(Par2Error::Limits(
                        "PAR2 source set has too many files for in-process repair".into(),
                    ));
                }
            } else if packet_type == *PAR2_TYPE_IFSC {
                if body_len < 16 || (body_len - 16) % 20 != 0 {
                    return Err(Par2Error::Malformed(format!(
                        "{} contains malformed PAR2 slice checksums",
                        path.display()
                    )));
                }
                let slices = (body_len - 16) / 20;
                stats.source_blocks = stats.source_blocks.checked_add(slices).ok_or_else(|| {
                    Par2Error::Limits("PAR2 source block count overflowed".into())
                })?;
                if stats.source_blocks > MAX_PAR2_SOURCE_BLOCKS {
                    return Err(Par2Error::Limits(
                        "PAR2 source set has too many blocks for in-process repair".into(),
                    ));
                }
            } else if packet_type == *PAR2_TYPE_MAIN {
                if body_len < 12 {
                    return Err(Par2Error::Malformed(format!(
                        "{} contains a truncated PAR2 main packet",
                        path.display()
                    )));
                }
                let mut main_prefix = [0u8; 12];
                reader.read_exact(&mut main_prefix)?;
                hasher.update(main_prefix);
                remaining -= main_prefix.len() as u64;
                let source_files =
                    u64::from(u32::from_le_bytes(main_prefix[8..].try_into().unwrap()));
                let expected_body = 12u64
                    .checked_add(source_files.checked_mul(16).ok_or_else(|| {
                        Par2Error::Limits("PAR2 main packet file order overflowed".into())
                    })?)
                    .ok_or_else(|| Par2Error::Limits("PAR2 main packet size overflowed".into()))?;
                if source_files > MAX_PAR2_SOURCE_FILES || body_len != expected_body {
                    return Err(Par2Error::Malformed(format!(
                        "{} contains an invalid PAR2 main packet file order",
                        path.display()
                    )));
                }
            }
        }
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            reader.read_exact(&mut buffer[..count])?;
            hasher.update(&buffer[..count]);
            remaining -= count as u64;
        }
        let actual_md5: [u8; 16] = hasher.finalize().into();
        if actual_md5 != expected_md5 {
            return Err(Par2Error::Malformed(format!(
                "{} contains a PAR2 packet with an invalid MD5",
                path.display()
            )));
        }
        position += packet_len;
        stats.packets += 1;
        if stats.packets > MAX_PAR2_PACKET_COUNT {
            return Err(Par2Error::Limits("too many PAR2 packets".into()));
        }
    }
    Ok(stats)
}

fn parse_index(
    parity_files: &[ValidatedParityFile],
    data_by_name: &HashMap<String, &Par2InputFile>,
    required_incomplete_names: &BTreeSet<String>,
    limits: ArchiveLimits,
) -> Result<(rust_par2::Par2FileSet, SourceStats), Par2Error> {
    let mut errors = Vec::new();
    for parity_file in parity_files {
        match rust_par2::parse(&parity_file.path) {
            Ok(file_set) => {
                if file_set.files.is_empty() {
                    errors.push(format!(
                        "{} has no source files",
                        parity_file.path.display()
                    ));
                    continue;
                }
                match validate_file_set(&file_set, data_by_name, limits).and_then(|source| {
                    validate_incomplete_coverage(&file_set, required_incomplete_names)?;
                    Ok(source)
                }) {
                    Ok(source) => return Ok((file_set, source)),
                    Err(
                        error
                        @ (Par2Error::UnsafePath(_) | Par2Error::Limits(_) | Par2Error::Io(_)),
                    ) => {
                        return Err(error);
                    }
                    Err(error) => errors.push(format!("{}: {error}", parity_file.path.display())),
                }
            }
            Err(error) => errors.push(format!("{}: {error}", parity_file.path.display())),
        }
    }
    Err(Par2Error::Malformed(if errors.is_empty() {
        "no PAR2 index file was found".into()
    } else {
        errors.join("; ")
    }))
}

fn validate_incomplete_coverage(
    file_set: &rust_par2::Par2FileSet,
    required_incomplete_names: &BTreeSet<String>,
) -> Result<(), Par2Error> {
    if required_incomplete_names.is_empty() {
        return Ok(());
    }
    let covered = file_set_names(file_set)?;
    let uncovered = required_incomplete_names
        .iter()
        .filter(|name| !covered.contains(*name))
        .cloned()
        .collect::<Vec<_>>();
    if uncovered.is_empty() {
        Ok(())
    } else {
        Err(Par2Error::Repair(format!(
            "no complete PAR2 recovery set covers incomplete files {}",
            uncovered.join(", ")
        )))
    }
}

fn validate_file_set(
    file_set: &rust_par2::Par2FileSet,
    data_by_name: &HashMap<String, &Par2InputFile>,
    limits: ArchiveLimits,
) -> Result<SourceStats, Par2Error> {
    if file_set.slice_size == 0
        || file_set.slice_size > MAX_PAR2_SLICE_BYTES
        || file_set.slice_size % 2 != 0
    {
        return Err(Par2Error::Limits("invalid PAR2 slice size".into()));
    }
    if file_set.files.len() as u64 > limits.max_entries {
        return Err(Par2Error::Limits("too many PAR2 source files".into()));
    }
    if file_set.files.len() as u64 > MAX_PAR2_SOURCE_FILES {
        return Err(Par2Error::Limits(
            "PAR2 source set has too many files for in-process repair".into(),
        ));
    }
    let ordered_ids: HashSet<_> = file_set.file_order.iter().collect();
    if file_set.file_order.len() != file_set.files.len()
        || ordered_ids.len() != file_set.files.len()
        || file_set
            .file_order
            .iter()
            .any(|id| !file_set.files.contains_key(id))
    {
        return Err(Par2Error::Malformed("incomplete PAR2 file order".into()));
    }
    let mut names = HashSet::new();
    let mut portable_names = HashSet::new();
    let mut declared_bytes = 0u64;
    let mut source_blocks = 0u64;
    for file in file_set.files.values() {
        validate_simple_filename(&file.filename)?;
        validate_rust_par2_addressability(file.size, file_set.slice_size, usize::MAX as u64)?;
        if !portable_names.insert(portable_filename_key(&file.filename)) {
            return Err(Par2Error::Malformed(format!(
                "PAR2 source filename {} collides on case-insensitive filesystems",
                file.filename
            )));
        }
        let Some(input) = data_by_name.get(&file.filename) else {
            return Err(Par2Error::Malformed(format!(
                "PAR2 source file {} is not an NZB output",
                file.filename
            )));
        };
        if input.expected_size.is_some_and(|size| size != file.size) {
            return Err(Par2Error::Malformed(format!(
                "PAR2 size does not match yEnc size for {}",
                file.filename
            )));
        }
        if file.size > limits.max_entry_bytes {
            return Err(Par2Error::Limits(format!(
                "{} exceeds the per-file limit",
                file.filename
            )));
        }
        let expected_slices = file.size.div_ceil(file_set.slice_size);
        if file.slices.len() as u64 != expected_slices {
            return Err(Par2Error::Malformed(format!(
                "PAR2 slice metadata is incomplete for {}",
                file.filename
            )));
        }
        let source_len = fs::metadata(&input.source_path)?.len();
        if source_len > file.size {
            return Err(Par2Error::Limits(format!(
                "{} is larger than its PAR2 source size",
                input.manifest_name
            )));
        }
        declared_bytes = declared_bytes
            .checked_add(file.size)
            .ok_or_else(|| Par2Error::Limits("PAR2 source size overflowed".into()))?;
        if !names.insert(file.filename.as_str()) {
            return Err(Par2Error::Malformed(format!(
                "duplicate PAR2 source filename {}",
                file.filename
            )));
        }
        source_blocks = source_blocks
            .checked_add(expected_slices)
            .ok_or_else(|| Par2Error::Limits("PAR2 source block count overflowed".into()))?;
    }
    if source_blocks > MAX_PAR2_SOURCE_BLOCKS {
        return Err(Par2Error::Limits(
            "PAR2 source set has too many blocks for in-process repair".into(),
        ));
    }
    Ok(SourceStats {
        bytes: declared_bytes,
    })
}

fn file_set_names(file_set: &rust_par2::Par2FileSet) -> Result<BTreeSet<String>, Par2Error> {
    let mut names = BTreeSet::new();
    let mut portable_names = HashSet::new();
    for file in file_set.files.values() {
        validate_simple_filename(&file.filename)?;
        if !portable_names.insert(portable_filename_key(&file.filename)) {
            return Err(Par2Error::Malformed(format!(
                "PAR2 source filename {} collides on case-insensitive filesystems",
                file.filename
            )));
        }
        names.insert(file.filename.clone());
    }
    Ok(names)
}

fn portable_filename_key(name: &str) -> String {
    name.to_lowercase()
}

fn validate_rust_par2_addressability(
    file_size: u64,
    slice_size: u64,
    max_usize: u64,
) -> Result<(), Par2Error> {
    if file_size > max_usize || slice_size > max_usize {
        return Err(Par2Error::Limits(
            "PAR2 source size exceeds this platform's addressable range".into(),
        ));
    }
    Ok(())
}

fn validate_simple_filename(name: &str) -> Result<(), Par2Error> {
    validate_member_path(name).map_err(|_| Par2Error::UnsafePath(name.to_string()))?;
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(Par2Error::UnsafePath(name.to_string()));
    }
    Ok(())
}

fn ensure_regular_file(path: &Path) -> Result<(), Par2Error> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Par2Error::UnsafePath(path.display().to_string()));
    }
    Ok(())
}

fn stage_parity_files(
    stage: &Path,
    parity_files: &[PathBuf],
    cancel: Option<&CancellationToken>,
) -> Result<(), Par2Error> {
    for (index, source) in parity_files.iter().enumerate() {
        check_cancel(cancel)?;
        let destination = stage.join(format!("parity-{index:05}.par2"));
        link_or_copy(source, &destination, cancel)?;
    }
    Ok(())
}

fn stage_data_files(
    stage: &Path,
    file_set: &rust_par2::Par2FileSet,
    data_by_name: &HashMap<String, &Par2InputFile>,
    intact_names: Option<&HashSet<&str>>,
    cancel: Option<&CancellationToken>,
) -> Result<(), Par2Error> {
    for file in file_set.files.values() {
        check_cancel(cancel)?;
        let input = data_by_name
            .get(&file.filename)
            .ok_or_else(|| Par2Error::UnsafePath(file.filename.clone()))?;
        let destination = stage.join(&file.filename);
        if intact_names.is_some_and(|names| names.contains(file.filename.as_str())) {
            link_or_copy(&input.source_path, &destination, cancel)?;
        } else {
            sparse_copy(&input.source_path, &destination, cancel)?;
        }
    }
    Ok(())
}

fn link_or_copy(
    source: &Path,
    destination: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<(), Par2Error> {
    check_cancel(cancel)?;
    ensure_regular_file(source)?;
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(Par2Error::Malformed(format!(
                "PAR2 staging path already exists: {}",
                destination.display()
            )))
        }
        Err(_) => copy_file_with_cancel(source, destination, cancel),
    }
}

fn copy_file_with_cancel(
    source: &Path,
    destination: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<(), Par2Error> {
    check_cancel(cancel)?;
    let mut reader = BufReader::new(fs::File::open(source)?);
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut buffer = vec![0u8; SPARSE_COPY_BUFFER_BYTES];
    loop {
        check_cancel(cancel)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
    }
    writer.sync_all()?;
    Ok(())
}

fn sparse_copy(
    source: &Path,
    destination: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<(), Par2Error> {
    ensure_regular_file(source)?;
    let source_length = fs::metadata(source)?.len();
    let mut reader = BufReader::new(fs::File::open(source)?);
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut buffer = vec![0u8; SPARSE_COPY_BUFFER_BYTES];
    let mut written = 0u64;
    loop {
        check_cancel(cancel)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().all(|byte| *byte == 0) {
            writer.seek(SeekFrom::Current(read as i64))?;
        } else {
            writer.write_all(&buffer[..read])?;
        }
        written += read as u64;
    }
    if written != source_length {
        return Err(Par2Error::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "short read while staging PAR2 input",
        )));
    }
    writer.set_len(source_length)?;
    writer.sync_all()?;
    Ok(())
}

fn promote_outputs(
    stage: &Path,
    names: &BTreeSet<String>,
    data_by_name: &HashMap<String, &Par2InputFile>,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<PathBuf>, Par2Error> {
    let mut promotions = Vec::new();
    for name in names {
        if let Err(error) = check_cancel(cancel) {
            rollback_promotions(&mut promotions);
            return Err(error);
        }
        let input = match data_by_name.get(name) {
            Some(input) => input,
            None => {
                rollback_promotions(&mut promotions);
                return Err(Par2Error::UnsafePath(name.clone()));
            }
        };
        let source = stage.join(name);
        if let Err(error) = ensure_regular_file(&source) {
            rollback_promotions(&mut promotions);
            return Err(error);
        }
        if input.output_path.exists() && input.output_path != input.source_path {
            rollback_promotions(&mut promotions);
            return Err(Par2Error::Repair(format!(
                "repair output {} appeared while the task was running",
                input.output_path.display()
            )));
        }
        let promotion = match promote_one(&source, &input.output_path) {
            Ok(promotion) => promotion,
            Err(error) => {
                rollback_promotions(&mut promotions);
                return Err(error);
            }
        };
        promotions.push(promotion);
    }
    if let Err(error) = check_cancel(cancel) {
        rollback_promotions(&mut promotions);
        return Err(error);
    }
    let outputs = promotions
        .iter()
        .map(|promotion| promotion.destination.clone())
        .collect();
    for promotion in promotions {
        if let Some(backup) = promotion.backup {
            let _ = fs::remove_file(backup);
        }
    }
    Ok(outputs)
}

struct OutputPromotion {
    source: PathBuf,
    destination: PathBuf,
    backup: Option<PathBuf>,
}

fn promote_one(source: &Path, destination: &Path) -> Result<OutputPromotion, Par2Error> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    if !destination.exists() {
        fs::rename(source, destination)?;
        return Ok(OutputPromotion {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            backup: None,
        });
    }
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    let backup = destination.with_file_name(format!(
        ".{file_name}.risuko-par2-backup-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::rename(destination, &backup)?;
    if let Err(error) = fs::rename(source, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(Par2Error::Io(error));
    }
    Ok(OutputPromotion {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        backup: Some(backup),
    })
}

fn rollback_promotions(promotions: &mut Vec<OutputPromotion>) {
    for promotion in promotions.drain(..).rev() {
        if promotion.destination.exists() {
            let _ = fs::rename(&promotion.destination, &promotion.source);
        }
        if let Some(backup) = promotion.backup {
            if backup.exists() {
                let _ = fs::rename(backup, promotion.destination);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPE_MAIN: &[u8; 16] = b"PAR 2.0\0Main\0\0\0\0";
    const TYPE_FILE_DESC: &[u8; 16] = b"PAR 2.0\0FileDesc";
    const TYPE_IFSC: &[u8; 16] = b"PAR 2.0\0IFSC\0\0\0\0";
    const TYPE_RECOVERY: &[u8; 16] = b"PAR 2.0\0RecvSlic";

    fn build_packet(set_id: [u8; 16], packet_type: &[u8; 16], body: &[u8]) -> Vec<u8> {
        let mut data = Vec::with_capacity(32 + body.len());
        data.extend_from_slice(&set_id);
        data.extend_from_slice(packet_type);
        data.extend_from_slice(body);
        let mut packet = Vec::with_capacity(64 + body.len());
        packet.extend_from_slice(PAR2_MAGIC);
        packet.extend_from_slice(&((64 + body.len()) as u64).to_le_bytes());
        packet.extend_from_slice(&Md5::digest(&data));
        packet.extend_from_slice(&data);
        packet
    }

    fn md5(bytes: &[u8]) -> [u8; 16] {
        Md5::digest(bytes).into()
    }

    fn write_fixture(parity_dir: &Path, name: &str, data: &[u8], recovery: bool) -> Vec<PathBuf> {
        let set_id = [0x5a; 16];
        let file_id = [0x11; 16];
        let slice_size = 4usize;
        let mut index = Vec::new();
        let mut main = Vec::new();
        main.extend_from_slice(&(slice_size as u64).to_le_bytes());
        main.extend_from_slice(&1u32.to_le_bytes());
        main.extend_from_slice(&file_id);
        index.extend_from_slice(&build_packet(set_id, TYPE_MAIN, &main));

        let mut desc = Vec::new();
        desc.extend_from_slice(&file_id);
        desc.extend_from_slice(&md5(data));
        desc.extend_from_slice(&md5(&data[..data.len().min(16_384)]));
        desc.extend_from_slice(&(data.len() as u64).to_le_bytes());
        desc.extend_from_slice(name.as_bytes());
        desc.push(0);
        while desc.len() % 4 != 0 {
            desc.push(0);
        }
        index.extend_from_slice(&build_packet(set_id, TYPE_FILE_DESC, &desc));

        let mut ifsc = Vec::new();
        ifsc.extend_from_slice(&file_id);
        let mut blocks = Vec::new();
        for chunk in data.chunks(slice_size) {
            let mut block = vec![0u8; slice_size];
            block[..chunk.len()].copy_from_slice(chunk);
            ifsc.extend_from_slice(&md5(&block));
            ifsc.extend_from_slice(&crate::engine::usenet_pipeline::crc32(&block).to_le_bytes());
            blocks.push(block);
        }
        index.extend_from_slice(&build_packet(set_id, TYPE_IFSC, &ifsc));
        let index_path = parity_dir.join("sample.par2");
        fs::write(&index_path, &index).unwrap();
        if !recovery {
            return vec![index_path];
        }

        let mut recovery_data = vec![0u8; slice_size];
        for block in blocks {
            for (target, value) in recovery_data.iter_mut().zip(block) {
                *target ^= value;
            }
        }
        let mut recovery_body = 0u32.to_le_bytes().to_vec();
        recovery_body.extend_from_slice(&recovery_data);
        let mut volume = index;
        volume.extend_from_slice(&build_packet(set_id, TYPE_RECOVERY, &recovery_body));
        let volume_path = parity_dir.join("sample.vol00+1.par2");
        fs::write(&volume_path, volume).unwrap();
        vec![index_path, volume_path]
    }

    fn repair_request(dir: &Path, parity_files: Vec<PathBuf>) -> Par2RepairRequest {
        let mut limits = platform_limits();
        limits.free_space_reserve_bytes = 0;
        Par2RepairRequest {
            destination: dir.to_path_buf(),
            data_files: vec![Par2InputFile {
                manifest_name: "data.bin".into(),
                source_path: dir.join("data.bin.part"),
                output_path: dir.join("data.bin"),
                expected_size: Some(8),
            }],
            parity_files,
            required_incomplete_names: BTreeSet::from(["data.bin".to_string()]),
            limits,
            active_started_at: None,
            active_elapsed_before_repair: None,
        }
    }

    fn test_limits(max_expanded_bytes: u64) -> ArchiveLimits {
        ArchiveLimits {
            max_entries: 10,
            max_expanded_bytes,
            max_entry_bytes: max_expanded_bytes,
            max_nesting_depth: 1,
            max_compression_ratio: 1,
            free_space_reserve_bytes: 0,
            max_active_seconds: 60,
        }
    }

    #[test]
    fn repair_time_accounts_for_active_assembly_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = repair_request(dir.path(), Vec::new());
        request.limits.max_active_seconds = 60;
        request.active_elapsed_before_repair = Some(Duration::from_secs(61));

        let error = check_active_time(&request).unwrap_err();

        assert!(matches!(error, Par2Error::Limits(_)));
    }

    #[test]
    fn repairs_partial_file_without_changing_original_part_on_failure_paths() {
        let dir = tempfile::tempdir().unwrap();
        let parity = tempfile::tempdir().unwrap();
        let complete = b"ABCDEFGH";
        let parity_files = write_fixture(parity.path(), "data.bin", complete, true);
        let partial = b"ABCD\0\0\0\0";
        fs::write(dir.path().join("data.bin.part"), partial).unwrap();

        let result = verify_or_repair(&repair_request(dir.path(), parity_files)).unwrap();

        assert_eq!(result.report.outcome, Par2Outcome::Repaired);
        assert_eq!(fs::read(dir.path().join("data.bin")).unwrap(), complete);
        assert_eq!(fs::read(dir.path().join("data.bin.part")).unwrap(), partial);
    }

    #[test]
    fn verifies_complete_data_before_the_worker_accepts_it() {
        let dir = tempfile::tempdir().unwrap();
        let parity = tempfile::tempdir().unwrap();
        let complete = b"ABCDEFGH";
        let parity_files = write_fixture(parity.path(), "data.bin", complete, true);
        let output = dir.path().join("data.bin");
        fs::write(&output, complete).unwrap();
        let mut request = repair_request(dir.path(), parity_files);
        request.data_files[0].source_path = output.clone();
        request.required_incomplete_names.clear();

        let result = verify_or_repair(&request).unwrap();

        assert_eq!(result.report.outcome, Par2Outcome::Verified);
        assert_eq!(fs::read(output).unwrap(), complete);
    }

    #[test]
    fn refuses_partial_repair_when_a_required_file_is_not_covered() {
        let dir = tempfile::tempdir().unwrap();
        let parity = tempfile::tempdir().unwrap();
        let complete = b"ABCDEFGH";
        let parity_files = write_fixture(parity.path(), "data.bin", complete, true);
        let partial = b"ABCD\0\0\0\0";
        fs::write(dir.path().join("data.bin.part"), partial).unwrap();
        fs::write(dir.path().join("other.bin.part"), b"\0\0\0\0").unwrap();
        let mut request = repair_request(dir.path(), parity_files);
        request.data_files.push(Par2InputFile {
            manifest_name: "other.bin".into(),
            source_path: dir.path().join("other.bin.part"),
            output_path: dir.path().join("other.bin"),
            expected_size: Some(4),
        });
        request
            .required_incomplete_names
            .insert("other.bin".to_string());

        let error = verify_or_repair(&request).unwrap_err();

        assert!(error
            .to_string()
            .contains("covers incomplete files other.bin"));
        assert_eq!(fs::read(dir.path().join("data.bin.part")).unwrap(), partial);
        assert!(!dir.path().join("data.bin").exists());
    }

    #[test]
    fn preserves_partial_file_when_recovery_is_insufficient() {
        let dir = tempfile::tempdir().unwrap();
        let parity = tempfile::tempdir().unwrap();
        let parity_files = write_fixture(parity.path(), "data.bin", b"ABCDEFGH", false);
        let partial = b"ABCD\0\0\0\0";
        fs::write(dir.path().join("data.bin.part"), partial).unwrap();

        let error = verify_or_repair(&repair_request(dir.path(), parity_files)).unwrap_err();

        assert!(matches!(error, Par2Error::InsufficientRecovery { .. }));
        assert_eq!(fs::read(dir.path().join("data.bin.part")).unwrap(), partial);
    }

    #[test]
    fn cancellation_stops_the_buffered_link_fallback_before_creating_output() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let destination = dir.path().join("staged.bin");
        fs::write(&source, vec![1u8; SPARSE_COPY_BUFFER_BYTES * 2]).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let error = copy_file_with_cancel(&source, &destination, Some(&cancel)).unwrap_err();

        assert!(matches!(error, Par2Error::Cancelled));
        assert!(!destination.exists());
    }

    #[test]
    fn skips_an_index_that_references_a_missing_nzb_output() {
        let input_dir = tempfile::tempdir().unwrap();
        let bad_dir = tempfile::tempdir().unwrap();
        let good_dir = tempfile::tempdir().unwrap();
        let data = b"ABCDEFGH";
        let source = input_dir.path().join("data.bin.part");
        fs::write(&source, data).unwrap();
        let bad = write_fixture(bad_dir.path(), "missing.bin", data, false);
        let good = write_fixture(good_dir.path(), "data.bin", data, false);
        let inputs = vec![Par2InputFile {
            manifest_name: "data.bin".into(),
            source_path: source,
            output_path: input_dir.path().join("data.bin"),
            expected_size: Some(data.len() as u64),
        }];
        let data_by_name = validate_input_files(&inputs, platform_limits()).unwrap();
        let parity = validate_parity_files(&[bad[0].clone(), good[0].clone()]).unwrap();

        let (file_set, _) = parse_index(
            &parity.files,
            &data_by_name,
            &BTreeSet::new(),
            platform_limits(),
        )
        .unwrap();

        assert_eq!(file_set.files.len(), 1);
        assert_eq!(file_set.files.values().next().unwrap().filename, "data.bin");
    }

    #[test]
    fn rejects_unsafe_par2_filenames_before_staging() {
        let dir = tempfile::tempdir().unwrap();
        let parity = tempfile::tempdir().unwrap();
        let parity_files = write_fixture(parity.path(), "../escape.bin", b"ABCDEFGH", true);
        fs::write(dir.path().join("data.bin.part"), b"ABCD\0\0\0\0").unwrap();

        let error = verify_or_repair(&repair_request(dir.path(), parity_files)).unwrap_err();

        assert!(matches!(error, Par2Error::UnsafePath(_)));
    }

    #[test]
    fn rejects_malformed_par2_before_calling_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.par2");
        fs::write(&bad, b"not a parity file").unwrap();
        fs::write(dir.path().join("data.bin.part"), b"ABCD\0\0\0\0").unwrap();

        let error = verify_or_repair(&repair_request(dir.path(), vec![bad])).unwrap_err();

        assert!(matches!(error, Par2Error::Malformed(_)));
    }

    #[test]
    fn rejects_par2_packet_with_a_bad_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let parity = tempfile::tempdir().unwrap();
        let parity_files = write_fixture(parity.path(), "data.bin", b"ABCDEFGH", true);
        let index = &parity_files[0];
        let mut bytes = fs::read(index).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(index, bytes).unwrap();
        fs::write(dir.path().join("data.bin.part"), b"ABCD\0\0\0\0").unwrap();

        let error = verify_or_repair(&repair_request(dir.path(), parity_files)).unwrap_err();

        assert!(matches!(error, Par2Error::Malformed(_)));
    }

    #[test]
    fn counts_parity_and_source_data_in_one_task_budget() {
        let request = Par2RepairRequest {
            destination: PathBuf::from("unused-destination"),
            data_files: vec![Par2InputFile {
                manifest_name: "data.bin".into(),
                source_path: PathBuf::from("unused.part"),
                output_path: PathBuf::from("unused"),
                expected_size: Some(8),
            }],
            parity_files: vec![PathBuf::from("sample.par2")],
            required_incomplete_names: BTreeSet::new(),
            limits: test_limits(10),
            active_started_at: None,
            active_elapsed_before_repair: None,
        };

        let error = validate_task_budget(
            &request,
            SourceStats { bytes: 8 },
            ParityStats {
                bytes: 3,
                packets: 1,
                recovery_blocks: 0,
            },
        )
        .unwrap_err();

        assert!(matches!(error, Par2Error::Limits(_)));
    }

    #[test]
    fn rejects_repair_sets_that_would_create_an_unbounded_matrix() {
        let verification = rust_par2::VerifyResult {
            intact: Vec::new(),
            damaged: vec![rust_par2::DamagedFile {
                filename: "data.bin".into(),
                size: 2_050,
                damaged_block_count: MAX_PAR2_REPAIR_BLOCKS + 1,
                total_block_count: MAX_PAR2_REPAIR_BLOCKS + 1,
                damaged_block_indices: (0..=MAX_PAR2_REPAIR_BLOCKS).collect(),
            }],
            missing: Vec::new(),
            recovery_blocks_available: MAX_PAR2_REPAIR_BLOCKS + 1,
            repair_possible: true,
        };
        let file_set = rust_par2::Par2FileSet {
            recovery_set_id: [0; 16],
            slice_size: 2,
            file_order: Vec::new(),
            files: HashMap::new(),
            recovery_block_count: 0,
            creator: None,
        };

        let error = validate_repair_resources(&verification, &file_set).unwrap_err();

        assert!(matches!(error, Par2Error::Limits(_)));
    }

    #[test]
    fn rejects_excess_recovery_packets_before_parsing_with_rust_par2() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("too-many-recovery.par2");
        let mut bytes = Vec::new();
        for exponent in 0..=MAX_PAR2_RECOVERY_BLOCKS {
            bytes.extend_from_slice(&build_packet(
                [0x5a; 16],
                TYPE_RECOVERY,
                &exponent.to_le_bytes(),
            ));
        }
        fs::write(&path, bytes).unwrap();

        let error = validate_parity_files(&[path]).unwrap_err();

        assert!(matches!(error, Par2Error::Limits(_)));
    }

    #[test]
    fn rejects_oversized_verification_buffers_before_verifying_files() {
        let file_set = rust_par2::Par2FileSet {
            recovery_set_id: [0; 16],
            slice_size: repair_memory_limit(),
            file_order: Vec::new(),
            files: HashMap::new(),
            recovery_block_count: 0,
            creator: None,
        };

        let error = validate_verification_resources(&file_set).unwrap_err();

        assert!(matches!(error, Par2Error::Limits(_)));
    }

    #[test]
    fn rejects_source_sizes_that_rust_par2_cannot_address_on_32_bit_targets() {
        let error =
            validate_rust_par2_addressability(u64::from(u32::MAX) + 1, 4, u64::from(u32::MAX))
                .unwrap_err();

        assert!(matches!(error, Par2Error::Limits(_)));
    }

    #[test]
    fn rejects_case_insensitive_nzb_input_collisions_before_staging() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.part");
        fs::write(&source, b"data").unwrap();
        let inputs = vec![
            Par2InputFile {
                manifest_name: "Data.bin".into(),
                source_path: source.clone(),
                output_path: dir.path().join("Data.bin"),
                expected_size: Some(4),
            },
            Par2InputFile {
                manifest_name: "data.bin".into(),
                source_path: source,
                output_path: dir.path().join("data.bin"),
                expected_size: Some(4),
            },
        ];

        let error = validate_input_files(&inputs, test_limits(10)).unwrap_err();

        assert!(matches!(error, Par2Error::Malformed(_)));
    }

    #[test]
    fn rolls_back_previous_outputs_when_a_later_promotion_fails() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("a.bin"), b"a").unwrap();

        let input_a = Par2InputFile {
            manifest_name: "a.bin".into(),
            source_path: dir.path().join("a.bin.part"),
            output_path: dir.path().join("a.bin"),
            expected_size: Some(1),
        };
        let input_b = Par2InputFile {
            manifest_name: "b.bin".into(),
            source_path: dir.path().join("b.bin.part"),
            output_path: dir.path().join("b.bin"),
            expected_size: Some(1),
        };
        let inputs = HashMap::from([
            (input_a.manifest_name.clone(), &input_a),
            (input_b.manifest_name.clone(), &input_b),
        ]);
        let names = BTreeSet::from(["a.bin".to_string(), "b.bin".to_string()]);

        assert!(promote_outputs(&stage, &names, &inputs, None).is_err());
        assert!(!dir.path().join("a.bin").exists());
        assert_eq!(fs::read(stage.join("a.bin")).unwrap(), b"a");
    }
}
