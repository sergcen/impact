use crate::config::{CONFIG_NAMES, Config, config_signature, matches_any_glob, matches_glob};
use crate::extract::analyze_source;
use crate::git::git_output;
use crate::model::{Graph, GraphStats, Importer, Project, Record, UnresolvedImporter, Workspaces};
use crate::resolver::{Resolver, resolution_watch_matches};
use crate::workspaces::discover_workspaces;
use rayon::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

pub const CACHE_VERSION: u32 = 18;
pub const REVERSE_SHARD_COUNT: usize = 512;
pub const CACHE_METADATA_FILE: &str = "metadata.bin";
const CACHE_CODEC_MAGIC: &[u8; 4] = b"FODC";
const CACHE_CODEC_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileState {
    #[serde(rename = "mtimeMs")]
    pub mtime_ms: f64,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProjectMetadata {
    pub dir: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metadata {
    #[serde(rename = "bindingReversePrefix")]
    pub binding_reverse_prefix: String,
    #[serde(rename = "configFileState", default)]
    pub config_file_state: HashMap<String, Option<FileState>>,
    #[serde(rename = "configSignature", default)]
    pub config_signature: String,
    #[serde(rename = "fileCount")]
    pub file_count: usize,
    #[serde(rename = "graphFile")]
    pub graph_file: String,
    pub head: Option<String>,
    #[serde(rename = "invalidationState", default)]
    pub invalidation_state: HashMap<String, Option<FileState>>,
    pub projects: Vec<ProjectMetadata>,
    #[serde(rename = "reversePrefix")]
    pub reverse_prefix: String,
    #[serde(rename = "reverseShardCount")]
    pub reverse_shard_count: usize,
    #[serde(rename = "reverseShardFiles", default)]
    pub reverse_shard_files: Vec<String>,
    #[serde(rename = "unresolvedFile")]
    pub unresolved_file: String,
    #[serde(rename = "validationFile")]
    pub validation_file: String,
    pub version: u32,
    #[serde(rename = "workingTreeSignature", default)]
    pub working_tree_signature: Option<String>,
    #[serde(rename = "workingTreeFiles", default)]
    pub working_tree_files: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ReverseShard {
    #[serde(rename = "f", alias = "files")]
    files: Vec<String>,
    #[serde(rename = "i", alias = "importers")]
    importers: HashMap<String, Vec<Importer>>,
}

#[derive(Deserialize, Serialize)]
struct StoredGraph {
    #[serde(rename = "exportsByFile", default)]
    exports_by_file: HashMap<String, Vec<String>>,
    files: Vec<String>,
    records: HashMap<String, Record>,
    reverse: HashMap<String, Vec<Importer>>,
    workspaces: Workspaces,
}

#[derive(Serialize)]
struct StoredGraphRef<'a> {
    #[serde(rename = "exportsByFile")]
    exports_by_file: &'a HashMap<String, Vec<String>>,
    files: &'a [String],
    records: &'a HashMap<String, Record>,
    reverse: &'a HashMap<String, Vec<Importer>>,
    workspaces: &'a Workspaces,
}

struct WorkingTreeState {
    files: HashMap<String, String>,
    signature: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct NativeFileState {
    modified_nanos: u32,
    modified_seconds: u64,
    size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ValidationFile {
    name: String,
    state: NativeFileState,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ValidationDirectory {
    path: String,
    state: NativeFileState,
    files: Vec<ValidationFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ValidationBatch {
    path: String,
    directories: Vec<ValidationDirectory>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ValidationSnapshot {
    batches: Vec<ValidationBatch>,
    git_index: Option<NativeFileState>,
}

#[derive(Deserialize)]
struct BorrowedValidationFile<'a> {
    #[serde(borrow)]
    name: &'a str,
    state: NativeFileState,
}

#[derive(Deserialize)]
struct BorrowedValidationDirectory<'a> {
    #[serde(borrow)]
    path: &'a str,
    state: NativeFileState,
    #[serde(borrow)]
    files: Vec<BorrowedValidationFile<'a>>,
}

#[derive(Deserialize)]
struct BorrowedValidationBatch<'a> {
    #[serde(borrow)]
    path: &'a str,
    #[serde(borrow)]
    directories: Vec<BorrowedValidationDirectory<'a>>,
}

#[derive(Deserialize)]
struct BorrowedValidationSnapshot<'a> {
    #[serde(borrow)]
    batches: Vec<BorrowedValidationBatch<'a>>,
    git_index: Option<NativeFileState>,
}

fn reverse_shard_file(metadata: &Metadata, number: usize) -> String {
    metadata
        .reverse_shard_files
        .get(number)
        .cloned()
        .unwrap_or_else(|| format!("{}{:03x}.bin", metadata.reverse_prefix, number))
}

fn reverse_shard_files(metadata: &Metadata) -> HashSet<String> {
    (0..metadata.reverse_shard_count)
        .map(|number| reverse_shard_file(metadata, number))
        .collect()
}

fn cleanup_cache_generations(cache_directory: &Path, metadata: &Metadata) {
    let mut current = reverse_shard_files(metadata);
    current.insert(metadata.graph_file.clone());
    current.insert(metadata.unresolved_file.clone());
    current.insert(metadata.validation_file.clone());
    current.insert(CACHE_METADATA_FILE.into());

    let Ok(entries) = fs::read_dir(cache_directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(file) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let is_cache_artifact = file == "metadata.json"
            || file == "native-metadata.json"
            || file.starts_with("graph-")
            || file.starts_with("reverse-")
            || file.starts_with("unresolved-")
            || file.starts_with("validation-");
        if is_cache_artifact && !current.contains(&file) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub struct Snapshot {
    cache_directory: PathBuf,
    pub metadata: Metadata,
    shards: HashMap<usize, ReverseShard>,
    validation: String,
}

impl Snapshot {
    pub fn load(cwd: &Path, cache_directory: &Path, trust_cache: bool) -> Result<Self, String> {
        let metadata = read_cache_metadata(cache_directory)?;
        if metadata.version != CACHE_VERSION
            || metadata.reverse_shard_count != REVERSE_SHARD_COUNT
            || metadata.head != read_git_head(cwd)
            || !states_match(cwd, &metadata.config_file_state)
            || !states_match(cwd, &metadata.invalidation_state)
            || (!trust_cache && !native_validation_matches(cwd, cache_directory, &metadata))
        {
            return Err("incompatible snapshot".into());
        }
        Ok(Self {
            cache_directory: cache_directory.to_path_buf(),
            metadata,
            shards: HashMap::new(),
            validation: if trust_cache { "trusted" } else { "automatic" }.into(),
        })
    }

    fn shard(&mut self, file: &str) -> Result<&ReverseShard, String> {
        let number = reverse_shard(file);
        if !self.shards.contains_key(&number) {
            let file = reverse_shard_file(&self.metadata, number);
            let shard = read_cache_file(&self.cache_directory.join(file))?;
            self.shards.insert(number, shard);
        }
        self.shards
            .get(&number)
            .ok_or_else(|| "missing reverse shard".into())
    }

    pub fn contains(&mut self, file: &str) -> Result<bool, String> {
        Ok(self
            .shard(file)?
            .files
            .iter()
            .any(|candidate| candidate == file))
    }

    pub fn importers(&mut self, file: &str) -> Result<Vec<Importer>, String> {
        Ok(self
            .shard(file)?
            .importers
            .get(file)
            .cloned()
            .unwrap_or_default())
    }

    pub fn validation(&self) -> &str {
        &self.validation
    }
}

fn encode_cache_file(value: &impl Serialize) -> Result<Vec<u8>, String> {
    let payload = rmp_serde::to_vec_named(value).map_err(|error| error.to_string())?;
    let compressed = lz4_flex::compress_prepend_size(&payload);
    let mut bytes = Vec::with_capacity(CACHE_CODEC_MAGIC.len() + 1 + compressed.len());
    bytes.extend_from_slice(CACHE_CODEC_MAGIC);
    bytes.push(CACHE_CODEC_VERSION);
    bytes.extend_from_slice(&compressed);
    Ok(bytes)
}

fn decode_cache_payload(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < CACHE_CODEC_MAGIC.len() + 1
        || &bytes[..CACHE_CODEC_MAGIC.len()] != CACHE_CODEC_MAGIC
    {
        return Err("invalid cache file magic".into());
    }
    if bytes[CACHE_CODEC_MAGIC.len()] != CACHE_CODEC_VERSION {
        return Err("unsupported cache codec version".into());
    }
    lz4_flex::decompress_size_prepended(&bytes[CACHE_CODEC_MAGIC.len() + 1..])
        .map_err(|error| error.to_string())
}

fn decode_cache_file<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let payload = decode_cache_payload(bytes)?;
    rmp_serde::from_slice(&payload).map_err(|error| error.to_string())
}

fn read_cache_file<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    decode_cache_file(&bytes)
}

fn read_cache_payload(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    decode_cache_payload(&bytes)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn write_cache_file_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = encode_cache_file(value)?;
    write_bytes_atomic(path, &bytes)
}

fn read_stored_graph(path: &Path) -> Result<StoredGraph, String> {
    read_cache_file(path)
}

fn write_stored_graph_atomic(path: &Path, graph: &StoredGraphRef<'_>) -> Result<(), String> {
    write_cache_file_atomic(path, graph)
}

pub fn read_cache_metadata(cache_directory: &Path) -> Result<Metadata, String> {
    read_cache_file(&cache_directory.join(CACHE_METADATA_FILE))
}

pub fn file_state(path: &Path) -> Option<FileState> {
    let metadata = fs::metadata(path).ok()?;
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(FileState {
        mtime_ms: duration.as_secs_f64() * 1_000.0,
        size: metadata.len(),
    })
}

fn native_file_state(path: &Path) -> Option<NativeFileState> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(NativeFileState {
        modified_nanos: modified.subsec_nanos(),
        modified_seconds: modified.as_secs(),
        size: metadata.len(),
    })
}

fn state_equal(left: Option<&FileState>, right: Option<&FileState>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.size == right.size && (left.mtime_ms - right.mtime_ms).abs() <= 0.1
        }
        _ => false,
    }
}

fn states_match(cwd: &Path, stored: &HashMap<String, Option<FileState>>) -> bool {
    stored.iter().all(|(file, expected)| {
        state_equal(expected.as_ref(), file_state(&cwd.join(file)).as_ref())
    })
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(1_099_511_628_211);
    }
}

fn working_tree_directory_files(cwd: &Path, path: &str) -> Result<Vec<String>, String> {
    let mut files: Vec<String> = git_output(
        cwd,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            path,
        ],
        false,
    )?
    .split('\0')
    .filter(|file| !file.is_empty())
    .map(|file| file.replace('\\', "/"))
    .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

fn working_tree_file_signature(cwd: &Path, path: &str, state: &str) -> String {
    let mut hash = 14_695_981_039_346_656_037_u64;
    hash_bytes(&mut hash, state.as_bytes());
    if let Ok(contents) = fs::read(cwd.join(path)) {
        hash_bytes(&mut hash, &contents);
    }
    format!("{hash:016x}")
}

fn working_tree_state(cwd: &Path) -> Result<WorkingTreeState, String> {
    let status = git_output(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        false,
    )?;
    let mut files = BTreeMap::new();

    let mut fields = status.split('\0').filter(|field| !field.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let state = &entry[..2];
        let path = &entry[3..];
        if state == "??" && cwd.join(path).is_dir() {
            for file in working_tree_directory_files(cwd, path)? {
                files.insert(file.clone(), working_tree_file_signature(cwd, &file, state));
            }
        } else {
            files.insert(
                path.to_string(),
                working_tree_file_signature(cwd, path, state),
            );
        }

        if (state.contains('R') || state.contains('C'))
            && let Some(source) = fields.next()
        {
            files.insert(
                source.to_string(),
                working_tree_file_signature(cwd, source, &format!("{state}:source")),
            );
        }
    }

    let mut hash = 14_695_981_039_346_656_037_u64;
    for (file, fingerprint) in &files {
        hash_bytes(&mut hash, file.as_bytes());
        hash_bytes(&mut hash, &[0]);
        hash_bytes(&mut hash, fingerprint.as_bytes());
        hash_bytes(&mut hash, &[0]);
    }
    Ok(WorkingTreeState {
        files: files.into_iter().collect(),
        signature: format!("{hash:016x}"),
    })
}

fn config_file_state(cwd: &Path, config_path: Option<&Path>) -> HashMap<String, Option<FileState>> {
    let mut files: HashSet<String> = CONFIG_NAMES.iter().map(|name| name.to_string()).collect();
    if let Some(config_path) = config_path
        && let Ok(relative) = config_path.strip_prefix(cwd)
    {
        files.insert(relative.to_string_lossy().replace('\\', "/"));
    }
    files
        .into_iter()
        .map(|file| {
            let state = file_state(&cwd.join(&file));
            (file, state)
        })
        .collect()
}

fn invalidation_state(
    cwd: &Path,
    config: &Config,
    files: &[String],
) -> HashMap<String, Option<FileState>> {
    let patterns: Vec<String> = config
        .root_inputs
        .iter()
        .filter(|input| input.invalidate_graph)
        .flat_map(|input| input.patterns.clone())
        .collect();
    files
        .iter()
        .filter(|file| matches_any_glob(file, &patterns))
        .map(|file| (file.clone(), file_state(&cwd.join(file))))
        .collect()
}

pub fn reverse_shard(file: &str) -> usize {
    let mut hash = 2_166_136_261_u32;
    for code_unit in file.encode_utf16() {
        hash ^= u32::from(code_unit);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as usize % REVERSE_SHARD_COUNT
}

fn git_directory(cwd: &Path) -> Option<PathBuf> {
    let dot_git = cwd.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let pointer = fs::read_to_string(&dot_git).ok()?;
    let path = pointer.trim().strip_prefix("gitdir:")?.trim();
    let path = Path::new(path);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    })
}

fn git_index_state(cwd: &Path) -> Option<NativeFileState> {
    native_file_state(&git_directory(cwd)?.join("index"))
}

fn validation_directory_is_included(
    cwd: &Path,
    cache_directory: &Path,
    entry: &walkdir::DirEntry,
) -> bool {
    if entry.path() == cwd || !entry.file_type().is_dir() {
        return true;
    }
    !entry.path().starts_with(cache_directory)
        && !matches!(
            entry.file_name().to_str(),
            Some(".git" | "node_modules" | "dist" | "build" | "build2" | "target")
        )
}

fn collect_validation_directories(
    cwd: &Path,
    config: &Config,
    root: &Path,
    paths: &mut BTreeSet<String>,
) {
    let cache_directory = cwd.join(&config.cache.directory);
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| validation_directory_is_included(cwd, &cache_directory, entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir())
    {
        if let Ok(path) = entry.path().strip_prefix(cwd) {
            paths.insert(path.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn validation_batch_path(path: &str) -> &str {
    path.match_indices('/')
        .nth(1)
        .map_or(path, |(index, _)| &path[..index])
}

fn validation_relative_path<'a>(batch: &str, path: &'a str) -> &'a str {
    if batch.is_empty() {
        path
    } else if path == batch {
        ""
    } else {
        path.strip_prefix(batch)
            .and_then(|path| path.strip_prefix('/'))
            .unwrap_or(path)
    }
}

fn validation_full_path(batch: &str, path: &str) -> String {
    match (batch.is_empty(), path.is_empty()) {
        (true, _) => path.to_string(),
        (_, true) => batch.to_string(),
        _ => format!("{batch}/{path}"),
    }
}

fn create_validation_snapshot(
    cwd: &Path,
    config: &Config,
    files: &[String],
    previous_snapshot: Option<&ValidationSnapshot>,
    changed_directories: &[String],
) -> Result<ValidationSnapshot, String> {
    let files: Result<Vec<_>, String> = files
        .par_iter()
        .map(|path| {
            let (directory, name) = path
                .rsplit_once('/')
                .map_or(("", path.as_str()), |(directory, name)| (directory, name));
            Ok((
                directory.to_string(),
                ValidationFile {
                    name: name.to_string(),
                    state: native_file_state(&cwd.join(path))
                        .ok_or_else(|| format!("cannot stat validation file {path}"))?,
                },
            ))
        })
        .collect();
    let mut files_by_directory: BTreeMap<String, Vec<ValidationFile>> = BTreeMap::new();
    for (directory, file) in files? {
        files_by_directory.entry(directory).or_default().push(file);
    }
    for files in files_by_directory.values_mut() {
        files.sort_by(|left, right| left.name.cmp(&right.name));
    }

    let mut directory_paths = BTreeSet::new();
    if let Some(previous) = previous_snapshot {
        directory_paths.extend(
            previous
                .batches
                .iter()
                .flat_map(|batch| {
                    batch
                        .directories
                        .iter()
                        .map(|directory| validation_full_path(&batch.path, &directory.path))
                })
                .filter(|path| cwd.join(path).is_dir()),
        );
        for changed in changed_directories {
            let changed = cwd.join(changed);
            let root = if changed.is_dir() {
                changed
            } else {
                changed.parent().unwrap_or(cwd).to_path_buf()
            };
            collect_validation_directories(cwd, config, &root, &mut directory_paths);
        }
        for directory in files_by_directory.keys() {
            let mut directory = Some(Path::new(directory));
            while let Some(path) = directory {
                directory_paths.insert(path.to_string_lossy().replace('\\', "/"));
                directory = path.parent();
            }
        }
    } else {
        collect_validation_directories(cwd, config, cwd, &mut directory_paths);
    }
    directory_paths.extend(files_by_directory.keys().cloned());
    let directories: Result<Vec<_>, String> = directory_paths
        .into_iter()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|path| {
            Ok(ValidationDirectory {
                state: native_file_state(&cwd.join(&path))
                    .ok_or_else(|| format!("cannot stat validation directory {path}"))?,
                files: files_by_directory.get(&path).cloned().unwrap_or_default(),
                path,
            })
        })
        .collect();
    let mut batches: BTreeMap<String, Vec<ValidationDirectory>> = BTreeMap::new();
    for mut directory in directories? {
        let batch = validation_batch_path(&directory.path).to_string();
        directory.path = validation_relative_path(&batch, &directory.path).to_string();
        batches.entry(batch).or_default().push(directory);
    }

    Ok(ValidationSnapshot {
        batches: batches
            .into_iter()
            .map(|(path, directories)| ValidationBatch { path, directories })
            .collect(),
        git_index: git_index_state(cwd),
    })
}

#[cfg(unix)]
fn native_file_state_from_stat(stat: &libc::stat) -> Option<NativeFileState> {
    let (modified_seconds, modified_nanos) = (stat.st_mtime, stat.st_mtime_nsec);

    Some(NativeFileState {
        modified_nanos: u32::try_from(modified_nanos).ok()?,
        modified_seconds: u64::try_from(modified_seconds).ok()?,
        size: u64::try_from(stat.st_size).ok()?,
    })
}

#[cfg(unix)]
fn native_file_state_for_fd(fd: RawFd) -> Option<NativeFileState> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fstat` initializes the provided `stat` buffer on success.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful `fstat` call above initialized the whole value.
    let stat = unsafe { stat.assume_init() };
    native_file_state_from_stat(&stat)
}

#[cfg(unix)]
fn with_c_path<T>(
    path: &str,
    buffer: &mut [u8; 4096],
    action: impl FnOnce(*const libc::c_char) -> T,
) -> Option<T> {
    let bytes = path.as_bytes();
    if bytes.contains(&0) {
        return None;
    }
    if bytes.len() < buffer.len() {
        buffer[..bytes.len()].copy_from_slice(bytes);
        buffer[bytes.len()] = 0;
        return Some(action(buffer.as_ptr().cast()));
    }
    let path = std::ffi::CString::new(bytes).ok()?;
    Some(action(path.as_ptr()))
}

#[cfg(unix)]
fn native_file_state_at(
    directory_fd: RawFd,
    name: &str,
    buffer: &mut [u8; 4096],
) -> Option<NativeFileState> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = with_c_path(name, buffer, |name| {
        // SAFETY: `name` is NUL-terminated for this call, the directory descriptor
        // remains open, and `fstatat` initializes `stat` when it succeeds.
        unsafe { libc::fstatat(directory_fd, name, stat.as_mut_ptr(), 0) }
    })?;
    if result != 0 {
        return None;
    }
    // SAFETY: the successful `fstatat` call above initialized the whole value.
    let stat = unsafe { stat.assume_init() };
    native_file_state_from_stat(&stat)
}

#[cfg(unix)]
fn open_validation_batch(root_fd: RawFd, path: &str, buffer: &mut [u8; 4096]) -> Option<OwnedFd> {
    let fd = with_c_path(path, buffer, |path| {
        // SAFETY: `path` is NUL-terminated for the duration of this call and
        // `root_fd` remains open while validation runs.
        unsafe {
            libc::openat(
                root_fd,
                path,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        }
    })?;
    if fd < 0 {
        return None;
    }
    // SAFETY: the successful `openat` call returned a new owned descriptor.
    Some(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(unix)]
fn native_file_state_at_joined(
    directory_fd: RawFd,
    directory: &str,
    name: &str,
    buffer: &mut [u8; 4096],
) -> Option<NativeFileState> {
    let required = directory.len() + usize::from(!directory.is_empty()) + name.len();
    if required < buffer.len() {
        let mut length = 0;
        if !directory.is_empty() {
            buffer[..directory.len()].copy_from_slice(directory.as_bytes());
            length += directory.len();
            buffer[length] = b'/';
            length += 1;
        }
        buffer[length..length + name.len()].copy_from_slice(name.as_bytes());
        length += name.len();
        buffer[length] = 0;

        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the stack buffer contains a NUL-terminated relative path,
        // `directory_fd` remains open, and `fstatat` initializes `stat` on success.
        if unsafe { libc::fstatat(directory_fd, buffer.as_ptr().cast(), stat.as_mut_ptr(), 0) } != 0
        {
            return None;
        }
        // SAFETY: the successful `fstatat` call above initialized the whole value.
        let stat = unsafe { stat.assume_init() };
        return native_file_state_from_stat(&stat);
    }

    let path = if directory.is_empty() {
        name.to_string()
    } else {
        format!("{directory}/{name}")
    };
    native_file_state_at(directory_fd, &path, buffer)
}

#[cfg(unix)]
fn validation_directory_matches(
    batch_fd: RawFd,
    directory: &BorrowedValidationDirectory<'_>,
    buffer: &mut [u8; 4096],
) -> bool {
    let state = if directory.path.is_empty() {
        native_file_state_for_fd(batch_fd)
    } else {
        native_file_state_at(batch_fd, directory.path, buffer)
    };
    state.as_ref() == Some(&directory.state)
        && directory.files.iter().all(|file| {
            native_file_state_at_joined(batch_fd, directory.path, file.name, buffer).as_ref()
                == Some(&file.state)
        })
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct ValidationTask<'a> {
    batch_path: &'a str,
    directories: &'a [BorrowedValidationDirectory<'a>],
}

#[cfg(unix)]
fn validation_task_matches(
    root_fd: RawFd,
    task: ValidationTask<'_>,
    buffer: &mut [u8; 4096],
) -> bool {
    let owned_batch = if task.batch_path.is_empty() {
        None
    } else {
        match open_validation_batch(root_fd, task.batch_path, buffer) {
            Some(directory) => Some(directory),
            None => return false,
        }
    };
    let batch_fd = owned_batch.as_ref().map_or(root_fd, AsRawFd::as_raw_fd);
    task.directories
        .iter()
        .all(|directory| validation_directory_matches(batch_fd, directory, buffer))
}

#[cfg(unix)]
fn validation_snapshot_matches(cwd: &Path, snapshot: &BorrowedValidationSnapshot<'_>) -> bool {
    let Ok(root) = fs::File::open(cwd) else {
        return false;
    };
    let root_fd = root.as_raw_fd();
    let directory_count: usize = snapshot
        .batches
        .iter()
        .map(|batch| batch.directories.len())
        .sum();
    if directory_count < 128 {
        let mut buffer = [0_u8; 4096];
        return snapshot.batches.iter().all(|batch| {
            validation_task_matches(
                root_fd,
                ValidationTask {
                    batch_path: batch.path,
                    directories: &batch.directories,
                },
                &mut buffer,
            )
        });
    }

    #[cfg(target_os = "macos")]
    let worker_count = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .div_ceil(2)
        .clamp(4, 8);
    #[cfg(not(target_os = "macos"))]
    let worker_count = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .min(8);
    let mut tasks: Vec<(usize, ValidationTask<'_>)> = snapshot
        .batches
        .iter()
        .flat_map(|batch| {
            batch.directories.chunks(128).map(|directories| {
                let weight = directories
                    .iter()
                    .map(|directory| 1 + directory.files.len())
                    .sum();
                (
                    weight,
                    ValidationTask {
                        batch_path: batch.path,
                        directories,
                    },
                )
            })
        })
        .collect();
    tasks.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    let mut workers: Vec<(usize, Vec<ValidationTask<'_>>)> =
        (0..worker_count).map(|_| (0, vec![])).collect();
    for (weight, task) in tasks {
        let worker = workers
            .iter_mut()
            .min_by_key(|(current_weight, _)| *current_weight)
            .expect("validation has at least one worker");
        worker.0 += weight;
        worker.1.push(task);
    }
    workers.into_par_iter().all(|(_, tasks)| {
        let mut buffer = [0_u8; 4096];
        tasks
            .into_iter()
            .all(|task| validation_task_matches(root_fd, task, &mut buffer))
    })
}

#[cfg(not(unix))]
fn validation_snapshot_matches(cwd: &Path, snapshot: &BorrowedValidationSnapshot<'_>) -> bool {
    snapshot.batches.par_iter().all(|batch| {
        batch.directories.iter().all(|directory| {
            let path = cwd.join(&batch.path).join(directory.path);
            native_file_state(&path).as_ref() == Some(&directory.state)
                && directory.files.iter().all(|file| {
                    native_file_state(&path.join(file.name)).as_ref() == Some(&file.state)
                })
        })
    })
}

fn read_validation_payload(path: &Path) -> Result<Vec<u8>, String> {
    read_cache_payload(path)
}

fn native_validation_matches(cwd: &Path, cache_directory: &Path, metadata: &Metadata) -> bool {
    let payload = match read_validation_payload(&cache_directory.join(&metadata.validation_file)) {
        Ok(payload) => payload,
        Err(_) => return false,
    };
    let snapshot: BorrowedValidationSnapshot<'_> = match rmp_serde::from_slice(&payload) {
        Ok(snapshot) => snapshot,
        Err(_) => return false,
    };
    snapshot.git_index == git_index_state(cwd) && validation_snapshot_matches(cwd, &snapshot)
}

fn changed_validation_directories(
    cwd: &Path,
    cache_directory: &Path,
    metadata: &Metadata,
) -> Vec<String> {
    let payload = match read_validation_payload(&cache_directory.join(&metadata.validation_file)) {
        Ok(payload) => payload,
        Err(_) => return vec![],
    };
    let snapshot: BorrowedValidationSnapshot<'_> = match rmp_serde::from_slice(&payload) {
        Ok(snapshot) => snapshot,
        Err(_) => return vec![],
    };
    snapshot
        .batches
        .par_iter()
        .flat_map_iter(|batch| {
            batch
                .directories
                .iter()
                .map(move |directory| (batch.path, directory))
        })
        .filter(|(batch, directory)| {
            native_file_state(&cwd.join(batch).join(directory.path)).as_ref()
                != Some(&directory.state)
        })
        .map(|(batch, directory)| validation_full_path(batch, directory.path))
        .collect()
}

pub fn read_git_head(cwd: &Path) -> Option<String> {
    fn read_trimmed(path: &Path) -> Option<String> {
        fs::read_to_string(path)
            .ok()
            .map(|value| value.trim().to_string())
    }
    let git_directory = git_directory(cwd)?;
    let head = read_trimmed(&git_directory.join("HEAD"))?;
    if let Some(reference) = head.strip_prefix("ref: ") {
        let common = read_trimmed(&git_directory.join("commondir"))
            .map(|path| git_directory.join(path))
            .unwrap_or_else(|| git_directory.clone());
        read_trimmed(&git_directory.join(reference))
            .or_else(|| read_trimmed(&common.join(reference)))
            .or_else(|| {
                read_trimmed(&common.join("packed-refs")).and_then(|packed| {
                    packed.lines().find_map(|line| {
                        let (hash, name) = line.split_once(' ')?;
                        (name == reference).then(|| hash.to_string())
                    })
                })
            })
    } else {
        Some(head)
    }
}

fn list_repository_files(cwd: &Path, config: &Config) -> Vec<String> {
    let listed = git_output(cwd, &["ls-files", "-co", "--exclude-standard", "-z"], true)
        .ok()
        .map(|output| {
            output
                .split('\0')
                .filter(|file| !file.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    let files = listed.unwrap_or_else(|| {
        WalkDir::new(cwd)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir()
                    || !matches!(
                        entry.file_name().to_str(),
                        Some(".git" | "node_modules" | "dist" | "build" | "build2" | "target")
                    )
            })
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| entry.path().strip_prefix(cwd).ok().map(Path::to_path_buf))
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect()
    });
    let root_patterns: Vec<String> = config
        .root_inputs
        .iter()
        .flat_map(|input| input.patterns.clone())
        .collect();
    let extensions: HashSet<&str> = config.extensions.iter().map(String::as_str).collect();
    let mut filtered: Vec<String> = files
        .into_iter()
        .filter(|file| {
            let extension = Path::new(file)
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(".{value}"));
            let explicit = file == &config.workspace_file || matches_any_glob(file, &root_patterns);
            (explicit
                || extension
                    .as_deref()
                    .is_some_and(|value| extensions.contains(value)))
                && matches_any_glob(file, &config.include)
                && !matches_any_glob(file, &config.exclude)
        })
        .collect();
    filtered.sort();
    filtered.dedup();
    filtered
}

fn should_index_file(cwd: &Path, config: &Config, file: &str) -> bool {
    if !cwd.join(file).is_file() {
        return false;
    }
    let extension = Path::new(file)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"));
    let root_patterns: Vec<String> = config
        .root_inputs
        .iter()
        .flat_map(|input| input.patterns.clone())
        .collect();
    let explicit = file == config.workspace_file || matches_any_glob(file, &root_patterns);
    (explicit
        || extension
            .as_ref()
            .is_some_and(|extension| config.extensions.contains(extension)))
        && matches_any_glob(file, &config.include)
        && !matches_any_glob(file, &config.exclude)
}

fn is_parsable(file: &str) -> bool {
    matches!(
        Path::new(file).extension().and_then(|value| value.to_str()),
        Some(
            "ts" | "tsx"
                | "mts"
                | "cts"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "json"
                | "css"
                | "scss"
                | "less"
        )
    )
}

fn expand_glob(importer: &str, pattern: &str, known_files: &HashSet<String>) -> Vec<String> {
    if !pattern.starts_with('.') {
        return vec![];
    }
    known_files
        .iter()
        .filter(|file| importer_glob_matches(importer, pattern, file))
        .cloned()
        .collect()
}

fn importer_glob_matches(importer: &str, pattern: &str, file: &str) -> bool {
    if !pattern.starts_with('.') {
        return false;
    }
    let importer_directory = Path::new(importer)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    pathdiff::diff_paths(file, importer_directory).is_some_and(|relative| {
        let relative = relative.to_string_lossy().replace('\\', "/");
        let relative = if relative.starts_with('.') {
            relative
        } else {
            format!("./{relative}")
        };
        matches_glob(&relative, pattern)
    })
}

fn resolve_dependencies(
    file: &str,
    dependencies: &[crate::model::Dependency],
    resolver: &Resolver<'_>,
    known_files: &HashSet<String>,
) -> (
    Vec<crate::model::Dependency>,
    Vec<crate::model::ResolutionWatch>,
) {
    let mut resolved = vec![];
    let mut resolution_watches = vec![];
    for dependency in dependencies {
        if let Some(pattern) = &dependency.glob_pattern {
            resolved.push(dependency.clone());
            for target in expand_glob(file, pattern, known_files) {
                let mut expanded = dependency.clone();
                expanded.target = Some(target);
                expanded.glob_pattern = None;
                resolved.push(expanded);
            }
        } else {
            let (dependency, watches) = resolver.resolve_with_watches(file, dependency);
            resolved.push(dependency);
            resolution_watches.extend(watches);
        }
    }
    resolution_watches.sort();
    resolution_watches.dedup();
    (resolved, resolution_watches)
}

fn read_record(
    cwd: &Path,
    file: &str,
    resolver: &Resolver<'_>,
    known_files: &HashSet<String>,
) -> Option<Record> {
    let path = cwd.join(file);
    let metadata = fs::metadata(&path).ok()?;
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    let mut dependencies = vec![];
    let mut exports = vec![];
    let mut raw_dependencies = vec![];
    let mut resolution_watches = vec![];
    if is_parsable(file) {
        let source = fs::read_to_string(&path).ok()?;
        let analysis = analyze_source(file, &source);
        exports = analysis.export_fingerprints.into_keys().collect();
        raw_dependencies = analysis.dependencies;
        (dependencies, resolution_watches) =
            resolve_dependencies(file, &raw_dependencies, resolver, known_files);
    }
    Some(Record {
        dependencies,
        exports,
        mtime_ms: duration.as_secs_f64() * 1_000.0,
        raw_dependencies,
        resolution_watches,
        size: metadata.len(),
    })
}

fn create_indexes(
    records: &HashMap<String, Record>,
    workspaces: &Workspaces,
) -> (
    HashMap<String, Vec<Importer>>,
    HashMap<String, Vec<UnresolvedImporter>>,
) {
    let mut reverse: HashMap<String, Vec<Importer>> = HashMap::new();
    let mut unresolved: HashMap<String, Vec<UnresolvedImporter>> = HashMap::new();
    for (importer, record) in records {
        for dependency in &record.dependencies {
            if let Some(target) = &dependency.target {
                reverse.entry(target.clone()).or_default().push(Importer {
                    bindings: dependency.bindings.clone(),
                    importer: importer.clone(),
                    kind: dependency.kind.clone(),
                    specifier: dependency.specifier.clone(),
                });
            } else if let Some(workspace) = &dependency.workspace {
                unresolved
                    .entry(workspace.clone())
                    .or_default()
                    .push(UnresolvedImporter {
                        importer: importer.clone(),
                        project: workspaces
                            .project_for_file(importer)
                            .map(|project| project.name.clone()),
                        reason: dependency
                            .reason
                            .clone()
                            .unwrap_or_else(|| "unresolved".into()),
                        specifier: dependency.specifier.clone(),
                    });
            }
        }
    }
    for importers in reverse.values_mut() {
        importers.sort_by(|left, right| left.importer.cmp(&right.importer));
    }
    (reverse, unresolved)
}

struct CacheWrite<'a> {
    changed_directories: Option<&'a [String]>,
    changed_shards: Option<&'a HashSet<usize>>,
    config_path: Option<&'a Path>,
    records: &'a HashMap<String, Record>,
}

fn write_cache(
    cwd: &Path,
    cache_directory: &Path,
    config: &Config,
    graph: &Graph,
    workspaces: &Workspaces,
    options: CacheWrite<'_>,
) -> Result<(), String> {
    fs::create_dir_all(cache_directory).map_err(|error| error.to_string())?;
    let previous = read_cache_metadata(cache_directory).ok();
    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let prefix = format!("reverse-{generation}-{}-", std::process::id());
    let graph_file = format!("graph-{generation}-{}.bin", std::process::id());
    let unresolved_file = format!("unresolved-{generation}-{}.bin", std::process::id());
    let validation_file = format!("validation-{generation}-{}.bin", std::process::id());
    let previous_validation: Option<ValidationSnapshot> = options
        .changed_directories
        .and(previous.as_ref())
        .and_then(|metadata| {
            read_cache_file(&cache_directory.join(&metadata.validation_file)).ok()
        });
    let shard_numbers: HashSet<usize> = options
        .changed_shards
        .cloned()
        .unwrap_or_else(|| (0..REVERSE_SHARD_COUNT).collect());
    let mut shards: HashMap<usize, ReverseShard> = shard_numbers
        .iter()
        .map(|number| (*number, ReverseShard::default()))
        .collect();
    for file in &graph.files {
        if let Some(shard) = shards.get_mut(&reverse_shard(file)) {
            shard.files.push(file.clone());
        }
    }
    for (target, importers) in &graph.reverse {
        if let Some(shard) = shards.get_mut(&reverse_shard(target)) {
            shard.importers.insert(target.clone(), importers.clone());
        }
    }
    let mut shard_files = Vec::with_capacity(REVERSE_SHARD_COUNT);
    for number in 0..REVERSE_SHARD_COUNT {
        if !shard_numbers.contains(&number) {
            let file = previous
                .as_ref()
                .map(|metadata| reverse_shard_file(metadata, number))
                .filter(|file| cache_directory.join(file).is_file())
                .ok_or_else(|| format!("missing reusable reverse shard {number}"))?;
            shard_files.push(file);
        } else {
            let file = format!("{prefix}{number:03x}.bin");
            write_cache_file_atomic(
                &cache_directory.join(&file),
                shards.get(&number).ok_or("missing changed reverse shard")?,
            )?;
            shard_files.push(file);
        }
    }
    write_stored_graph_atomic(
        &cache_directory.join(&graph_file),
        &StoredGraphRef {
            exports_by_file: &graph.exports_by_file,
            files: &graph.files,
            records: options.records,
            reverse: &graph.reverse,
            workspaces,
        },
    )?;
    write_cache_file_atomic(
        &cache_directory.join(&unresolved_file),
        &graph.unresolved_by_workspace,
    )?;
    let working_tree = working_tree_state(cwd).ok();
    write_cache_file_atomic(
        &cache_directory.join(&validation_file),
        &create_validation_snapshot(
            cwd,
            config,
            &graph.files,
            previous_validation.as_ref(),
            options.changed_directories.unwrap_or_default(),
        )?,
    )?;
    let metadata = Metadata {
        binding_reverse_prefix: prefix.clone(),
        config_file_state: config_file_state(cwd, options.config_path),
        config_signature: config_signature(config),
        file_count: graph.files.len(),
        graph_file,
        head: read_git_head(cwd),
        invalidation_state: invalidation_state(cwd, config, &graph.files),
        projects: workspaces
            .projects
            .iter()
            .map(|project| ProjectMetadata {
                dir: project.dir.clone(),
                name: project.name.clone(),
            })
            .collect(),
        reverse_prefix: prefix,
        reverse_shard_count: REVERSE_SHARD_COUNT,
        reverse_shard_files: shard_files,
        unresolved_file,
        validation_file,
        version: CACHE_VERSION,
        working_tree_signature: working_tree.as_ref().map(|state| state.signature.clone()),
        working_tree_files: working_tree.map(|state| state.files).unwrap_or_default(),
    };
    write_cache_file_atomic(&cache_directory.join(CACHE_METADATA_FILE), &metadata)?;
    cleanup_cache_generations(cache_directory, &metadata);
    Ok(())
}

pub fn load_valid_metadata(
    cwd: &Path,
    cache_directory: &Path,
    config: &Config,
    config_path: Option<&Path>,
    trust_cache: bool,
) -> Result<Metadata, String> {
    let metadata = read_cache_metadata(cache_directory)?;
    if metadata.version != CACHE_VERSION
        || metadata.reverse_shard_count != REVERSE_SHARD_COUNT
        || metadata.head != read_git_head(cwd)
        || metadata.config_signature != config_signature(config)
        || !states_match(cwd, &metadata.config_file_state)
        || !states_match(cwd, &metadata.invalidation_state)
        || (!trust_cache && !native_validation_matches(cwd, cache_directory, &metadata))
        || !generation_is_complete(cache_directory, &metadata)
        || config_path.is_some_and(|path| {
            path.strip_prefix(cwd)
                .ok()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .is_some_and(|path| !metadata.config_file_state.contains_key(&path))
        })
    {
        return Err("incompatible graph cache".into());
    }
    Ok(metadata)
}

fn generation_is_complete(cache_directory: &Path, metadata: &Metadata) -> bool {
    cache_directory.join(&metadata.graph_file).is_file()
        && cache_directory.join(&metadata.unresolved_file).is_file()
        && cache_directory.join(&metadata.validation_file).is_file()
        && (0..metadata.reverse_shard_count).all(|number| {
            cache_directory
                .join(reverse_shard_file(metadata, number))
                .is_file()
        })
}

fn refresh_validation_generation(
    cwd: &Path,
    cache_directory: &Path,
    config: &Config,
    metadata: &mut Metadata,
    files: &[String],
    head: Option<String>,
    working_tree: WorkingTreeState,
) -> Result<(), String> {
    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let previous = metadata.validation_file.clone();
    let validation_file = format!("validation-{generation}-{}.bin", std::process::id());
    write_cache_file_atomic(
        &cache_directory.join(&validation_file),
        &create_validation_snapshot(cwd, config, files, None, &[])?,
    )?;
    metadata.head = head;
    metadata.validation_file = validation_file;
    metadata.working_tree_signature = Some(working_tree.signature);
    metadata.working_tree_files = working_tree.files;
    write_cache_file_atomic(&cache_directory.join(CACHE_METADATA_FILE), metadata)?;
    if previous != metadata.validation_file {
        let _ = fs::remove_file(cache_directory.join(previous));
    }
    Ok(())
}

fn cached_graph_with_workspaces(
    cwd: &Path,
    cache_directory: &Path,
    config: &Config,
    config_path: Option<&Path>,
    trust_cache: bool,
) -> Result<(Graph, Workspaces), String> {
    let metadata = load_valid_metadata(cwd, cache_directory, config, config_path, trust_cache)?;
    let stored = read_stored_graph(&cache_directory.join(&metadata.graph_file))?;
    let unresolved = read_cache_file(&cache_directory.join(&metadata.unresolved_file))?;
    Ok((
        Graph {
            exports_by_file: stored.exports_by_file,
            files: stored.files,
            reverse: stored.reverse,
            stats: GraphStats::cached(
                metadata.file_count,
                if trust_cache { "trusted" } else { "automatic" },
            ),
            unresolved_by_workspace: unresolved,
        },
        stored.workspaces,
    ))
}

fn cached_graph(
    cwd: &Path,
    cache_directory: &Path,
    config: &Config,
    config_path: Option<&Path>,
    trust_cache: bool,
) -> Result<Graph, String> {
    cached_graph_with_workspaces(cwd, cache_directory, config, config_path, trust_cache)
        .map(|(graph, _)| graph)
}

fn load_incremental_metadata(
    cwd: &Path,
    cache_directory: &Path,
    config: &Config,
    config_path: Option<&Path>,
) -> Result<Metadata, String> {
    let metadata = read_cache_metadata(cache_directory)?;
    if metadata.version != CACHE_VERSION
        || metadata.reverse_shard_count != REVERSE_SHARD_COUNT
        || metadata.config_signature != config_signature(config)
        || !states_match(cwd, &metadata.config_file_state)
        || metadata.working_tree_signature.is_none()
        || !generation_is_complete(cache_directory, &metadata)
        || config_path.is_some_and(|path| {
            path.strip_prefix(cwd)
                .ok()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .is_some_and(|path| !metadata.config_file_state.contains_key(&path))
        })
    {
        return Err("graph cache cannot be updated incrementally".into());
    }
    Ok(metadata)
}

fn changed_between_heads(
    cwd: &Path,
    previous: &Option<String>,
    current: &Option<String>,
) -> Result<Vec<String>, String> {
    if previous == current {
        return Ok(vec![]);
    }
    let (Some(previous), Some(current)) = (previous, current) else {
        return Err("Git HEAD cannot be compared incrementally".into());
    };
    Ok(git_output(
        cwd,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            previous,
            current,
        ],
        false,
    )?
    .split('\0')
    .filter(|file| !file.is_empty())
    .map(|file| file.replace('\\', "/"))
    .collect())
}

fn resolution_input_changed(file: &str, config: &Config) -> bool {
    let name = Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = Path::new(file).extension().and_then(|value| value.to_str());
    file == config.workspace_file
        || name == "package.json"
        || matches!(extension, Some("json" | "jsonc"))
}

fn workspace_input_changed(file: &str, config: &Config) -> bool {
    file == config.workspace_file
        || Path::new(file).file_name().and_then(|name| name.to_str()) == Some("package.json")
}

fn mark_dependency_shards(record: &Record, shards: &mut HashSet<usize>) {
    shards.extend(
        record
            .dependencies
            .iter()
            .filter_map(|dependency| dependency.target.as_deref())
            .map(reverse_shard),
    );
}

fn record_affected_by_file_set(
    importer: &str,
    record: &Record,
    changed_files: &HashSet<String>,
) -> bool {
    record.resolution_watches.iter().any(|watch| {
        changed_files
            .iter()
            .any(|file| resolution_watch_matches(watch, file))
    }) || record.raw_dependencies.iter().any(|dependency| {
        dependency.glob_pattern.as_deref().is_some_and(|pattern| {
            changed_files
                .iter()
                .any(|file| importer_glob_matches(importer, pattern, file))
        })
    })
}

fn incremental_graph(
    cwd: &Path,
    cache_directory: &Path,
    config: &Config,
    config_path: Option<&Path>,
) -> Result<(Graph, Workspaces), String> {
    let mut metadata = load_incremental_metadata(cwd, cache_directory, config, config_path)?;
    let changed_directories = changed_validation_directories(cwd, cache_directory, &metadata);
    let stored = read_stored_graph(&cache_directory.join(&metadata.graph_file))?;
    if stored.records.len() != stored.files.len() {
        return Err("cached records are incomplete".into());
    }

    let current_working_tree = working_tree_state(cwd)?;
    let current_head = read_git_head(cwd);
    let mut changed_files = BTreeSet::new();
    for file in metadata
        .working_tree_files
        .keys()
        .chain(current_working_tree.files.keys())
    {
        if metadata.working_tree_files.get(file) != current_working_tree.files.get(file) {
            changed_files.insert(file.clone());
        }
    }
    changed_files.extend(changed_between_heads(cwd, &metadata.head, &current_head)?);

    if changed_files.is_empty() {
        refresh_validation_generation(
            cwd,
            cache_directory,
            config,
            &mut metadata,
            &stored.files,
            current_head,
            current_working_tree,
        )?;
        let unresolved_by_workspace =
            read_cache_file(&cache_directory.join(&metadata.unresolved_file))?;
        let graph = Graph {
            exports_by_file: stored.exports_by_file,
            files: stored.files,
            reverse: stored.reverse,
            stats: GraphStats::cached(metadata.file_count, "automatic"),
            unresolved_by_workspace,
        };
        return Ok((graph, stored.workspaces));
    }

    let previous_files: HashSet<String> = stored.records.keys().cloned().collect();
    let mut known_files = previous_files.clone();
    for file in &changed_files {
        if should_index_file(cwd, config, file) {
            known_files.insert(file.clone());
        } else {
            known_files.remove(file);
        }
    }
    let file_set_changed = previous_files != known_files;
    let file_set_delta: HashSet<String> = previous_files
        .symmetric_difference(&known_files)
        .cloned()
        .collect();
    changed_files.extend(file_set_delta.iter().cloned());

    let changed_graph_files: HashSet<String> = changed_files
        .iter()
        .filter(|file| previous_files.contains(*file) || known_files.contains(*file))
        .cloned()
        .collect();
    let reresolve_all = changed_files
        .iter()
        .any(|file| resolution_input_changed(file, config));
    let reresolve = file_set_changed || reresolve_all;
    let workspaces = if changed_files
        .iter()
        .any(|file| workspace_input_changed(file, config))
    {
        discover_workspaces(cwd, config)?
    } else {
        stored.workspaces
    };
    let resolver = Resolver::new(config, cwd, &known_files, &workspaces);
    let mut records = stored.records;
    let mut changed_shards = HashSet::new();

    if reresolve {
        let reresolved_shards = records
            .par_iter_mut()
            .filter_map(|(file, record)| {
                if changed_graph_files.contains(file) || !known_files.contains(file) {
                    return None;
                }
                if !reresolve_all && !record_affected_by_file_set(file, record, &file_set_delta) {
                    return None;
                }
                let (dependencies, resolution_watches) =
                    resolve_dependencies(file, &record.raw_dependencies, &resolver, &known_files);
                if dependencies == record.dependencies
                    && resolution_watches == record.resolution_watches
                {
                    return None;
                }
                let dependencies_changed = dependencies != record.dependencies;
                let mut shards = vec![];
                if dependencies_changed {
                    shards.extend(
                        record
                            .dependencies
                            .iter()
                            .filter_map(|dependency| dependency.target.as_deref())
                            .map(reverse_shard),
                    );
                }
                record.dependencies = dependencies;
                record.resolution_watches = resolution_watches;
                if dependencies_changed {
                    shards.extend(
                        record
                            .dependencies
                            .iter()
                            .filter_map(|dependency| dependency.target.as_deref())
                            .map(reverse_shard),
                    );
                }
                Some(shards)
            })
            .reduce(Vec::new, |mut left, right| {
                left.extend(right);
                left
            });
        changed_shards.extend(reresolved_shards);
    }

    let mut parsed_files = 0;
    for file in &changed_graph_files {
        let previous = records.remove(file);
        if let Some(previous) = &previous {
            mark_dependency_shards(previous, &mut changed_shards);
        }
        if previous.is_some() != known_files.contains(file) {
            changed_shards.insert(reverse_shard(file));
        }
        if known_files.contains(file)
            && let Some(record) = read_record(cwd, file, &resolver, &known_files)
        {
            parsed_files += 1;
            mark_dependency_shards(&record, &mut changed_shards);
            records.insert(file.clone(), record);
        }
    }

    let mut indexed_files: Vec<String> = records.keys().cloned().collect();
    indexed_files.sort();
    let (reverse, unresolved_by_workspace) = create_indexes(&records, &workspaces);
    let exports_by_file = records
        .iter()
        .filter(|(_, record)| !record.exports.is_empty())
        .map(|(file, record)| (file.clone(), record.exports.clone()))
        .collect();
    let graph = Graph {
        exports_by_file,
        files: indexed_files,
        reverse,
        stats: GraphStats {
            cache: "hit".into(),
            parsed_files,
            reused_files: records.len().saturating_sub(parsed_files),
            snapshot: "incremental".into(),
            validation: "automatic".into(),
        },
        unresolved_by_workspace,
    };
    write_cache(
        cwd,
        cache_directory,
        config,
        &graph,
        &workspaces,
        CacheWrite {
            changed_directories: Some(&changed_directories),
            changed_shards: Some(&changed_shards),
            config_path,
            records: &records,
        },
    )?;
    Ok((graph, workspaces))
}

pub fn update_cached_graph(
    cwd: &Path,
    config: &Config,
    config_path: Option<&Path>,
) -> Result<(Graph, Workspaces), String> {
    let cache_directory = cwd.join(&config.cache.directory);
    cached_graph_with_workspaces(cwd, &cache_directory, config, config_path, false)
        .or_else(|_| incremental_graph(cwd, &cache_directory, config, config_path))
}

#[derive(Clone, Copy)]
pub struct BuildOptions {
    pub rebuild: bool,
    pub strict: bool,
    pub trust_cache: bool,
    pub use_cache: bool,
}

pub fn build_graph(
    cwd: &Path,
    config: &Config,
    config_path: Option<&Path>,
    workspaces: &Workspaces,
    options: BuildOptions,
) -> Result<Graph, String> {
    let cache_directory = cwd.join(&config.cache.directory);
    if options.use_cache
        && !options.rebuild
        && !options.strict
        && let Ok(graph) = cached_graph(
            cwd,
            &cache_directory,
            config,
            config_path,
            options.trust_cache,
        )
    {
        return Ok(graph);
    }
    if options.use_cache
        && !options.rebuild
        && !options.strict
        && !options.trust_cache
        && let Ok((graph, _)) = incremental_graph(cwd, &cache_directory, config, config_path)
    {
        return Ok(graph);
    }
    let files = list_repository_files(cwd, config);
    let known_files: HashSet<String> = files.iter().cloned().collect();
    let resolver = Resolver::new(config, cwd, &known_files, workspaces);
    let records: HashMap<String, Record> = files
        .par_iter()
        .filter_map(|file| {
            read_record(cwd, file, &resolver, &known_files).map(|record| (file.clone(), record))
        })
        .collect();
    let mut indexed_files: Vec<String> = records.keys().cloned().collect();
    indexed_files.sort();
    let (reverse, unresolved_by_workspace) = create_indexes(&records, workspaces);
    let exports_by_file = records
        .iter()
        .filter(|(_, record)| !record.exports.is_empty())
        .map(|(file, record)| (file.clone(), record.exports.clone()))
        .collect();
    let mut graph = Graph {
        exports_by_file,
        files: indexed_files,
        reverse,
        stats: GraphStats {
            cache: "miss".into(),
            parsed_files: records.len(),
            reused_files: 0,
            snapshot: "rebuilt".into(),
            validation: "strict".into(),
        },
        unresolved_by_workspace,
    };
    if options.use_cache {
        write_cache(
            cwd,
            &cache_directory,
            config,
            &graph,
            workspaces,
            CacheWrite {
                changed_directories: None,
                changed_shards: None,
                config_path,
                records: &records,
            },
        )?;
    } else {
        graph.stats.snapshot = "disabled".into();
    }
    Ok(graph)
}

pub fn metadata_projects(metadata: &Metadata) -> Vec<Project> {
    metadata
        .projects
        .iter()
        .map(|project| Project {
            dir: project.dir.clone(),
            manifest: serde_json::Value::Null,
            manifest_path: format!("{}/package.json", project.dir),
            name: project.name.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_hash_matches_the_original_utf16_fnv() {
        assert_eq!(reverse_shard("packages/core/src/index.ts"), 175);
        assert_eq!(reverse_shard("приложение/модуль.ts"), 122);
    }

    #[test]
    fn stored_graph_binary_round_trips_workspace_manifests() {
        let exports_by_file = HashMap::new();
        let files = vec!["packages/example/src/repeated-module-name.ts".into(); 128];
        let records = HashMap::new();
        let reverse = HashMap::new();
        let workspaces = Workspaces {
            projects: vec![Project {
                dir: "packages/example".into(),
                manifest: serde_json::json!({
                    "name": "@fixture/example",
                    "exports": { ".": "./src/index.ts" }
                }),
                manifest_path: "packages/example/package.json".into(),
                name: "@fixture/example".into(),
            }],
        };
        let graph = StoredGraphRef {
            exports_by_file: &exports_by_file,
            files: &files,
            records: &records,
            reverse: &reverse,
            workspaces: &workspaces,
        };
        let message_pack = rmp_serde::to_vec_named(&graph).expect("serialize MessagePack");
        let bytes = encode_cache_file(&graph).expect("encode compressed cache file");
        assert_eq!(&bytes[..4], CACHE_CODEC_MAGIC);
        assert!(bytes.len() < message_pack.len());
        let stored: StoredGraph = decode_cache_file(&bytes).expect("decode cache file");
        assert_eq!(stored.workspaces.projects[0].name, "@fixture/example");
        assert_eq!(
            stored.workspaces.projects[0].manifest["exports"]["."],
            "./src/index.ts"
        );

        let mut incompatible = bytes;
        incompatible[4] = CACHE_CODEC_VERSION + 1;
        assert_eq!(
            decode_cache_file::<StoredGraph>(&incompatible)
                .err()
                .expect("reject incompatible codec"),
            "unsupported cache codec version"
        );
    }

    #[test]
    fn zero_copy_native_validation_detects_file_edits_and_directory_additions() {
        let root = tempfile::tempdir().expect("validation fixture");
        let source_directory = root.path().join("components/example/src");
        fs::create_dir_all(&source_directory).expect("create source directory");
        fs::write(
            source_directory.join("index.ts"),
            "export const value = 1;\n",
        )
        .expect("write source");
        let files = vec!["components/example/src/index.ts".into()];
        let snapshot =
            create_validation_snapshot(root.path(), &Config::default(), &files, None, &[])
                .expect("snapshot");
        let payload = rmp_serde::to_vec_named(&snapshot).expect("validation payload");
        let borrowed: BorrowedValidationSnapshot<'_> =
            rmp_serde::from_slice(&payload).expect("borrowed validation snapshot");
        let prefix = borrowed
            .batches
            .iter()
            .find(|batch| batch.path == "components/example")
            .expect("validation prefix batch");
        let source = prefix
            .directories
            .iter()
            .find(|directory| directory.path == "src")
            .expect("source validation directory");
        assert_eq!(source.files[0].name, "index.ts");
        let payload_start = payload.as_ptr() as usize;
        let payload_end = payload_start + payload.len();
        let prefix_start = prefix.path.as_ptr() as usize;
        let file_start = source.files[0].name.as_ptr() as usize;
        assert!(prefix_start >= payload_start);
        assert!(prefix_start + prefix.path.len() <= payload_end);
        assert!(file_start >= payload_start);
        assert!(file_start + source.files[0].name.len() <= payload_end);
        assert!(validation_snapshot_matches(root.path(), &borrowed));

        fs::write(
            source_directory.join("index.ts"),
            "export const changedValue = 2;\n",
        )
        .expect("edit source");
        assert!(!validation_snapshot_matches(root.path(), &borrowed));

        let snapshot =
            create_validation_snapshot(root.path(), &Config::default(), &files, None, &[])
                .expect("snapshot");
        let payload = rmp_serde::to_vec_named(&snapshot).expect("validation payload");
        let borrowed: BorrowedValidationSnapshot<'_> =
            rmp_serde::from_slice(&payload).expect("borrowed validation snapshot");
        // Some CI filesystems expose directory mtimes with one-second precision.
        // Cross a timestamp tick before testing directory-addition detection.
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        fs::write(
            source_directory.join("added.ts"),
            "export const added = true;\n",
        )
        .expect("add source");
        assert!(!validation_snapshot_matches(root.path(), &borrowed));
    }

    #[test]
    fn validation_batching_supports_arbitrary_repository_layouts() {
        let root = tempfile::tempdir().expect("repository fixture");
        let sources = [
            ("index.ts", "export const root = true;\n"),
            ("src/main.ts", "export const main = true;\n"),
            (
                "tools/generators/templates/task.ts",
                "export const task = true;\n",
            ),
        ];
        for (path, source) in sources {
            let path = root.path().join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create arbitrary source directory");
            }
            fs::write(path, source).expect("write arbitrary source");
        }
        let files = sources
            .into_iter()
            .map(|(path, _)| path.to_string())
            .collect::<Vec<_>>();
        let snapshot =
            create_validation_snapshot(root.path(), &Config::default(), &files, None, &[])
                .expect("layout-independent snapshot");

        let root_batch = snapshot
            .batches
            .iter()
            .find(|batch| batch.path.is_empty())
            .expect("repository root batch");
        assert!(root_batch.directories[0].path.is_empty());
        assert_eq!(root_batch.directories[0].files[0].name, "index.ts");
        let source_batch = snapshot
            .batches
            .iter()
            .find(|batch| batch.path == "src")
            .expect("single-level source batch");
        assert_eq!(source_batch.directories[0].files[0].name, "main.ts");
        let tools_batch = snapshot
            .batches
            .iter()
            .find(|batch| batch.path == "tools/generators")
            .expect("deep source batch");
        let templates = tools_batch
            .directories
            .iter()
            .find(|directory| directory.path == "templates")
            .expect("relative nested directory");
        assert_eq!(templates.files[0].name, "task.ts");

        let payload = rmp_serde::to_vec_named(&snapshot).expect("validation payload");
        let borrowed: BorrowedValidationSnapshot<'_> =
            rmp_serde::from_slice(&payload).expect("borrowed validation snapshot");
        assert!(validation_snapshot_matches(root.path(), &borrowed));
        fs::write(root.path().join("index.ts"), "export const root = false;\n")
            .expect("edit root source");
        assert!(!validation_snapshot_matches(root.path(), &borrowed));
    }
}
