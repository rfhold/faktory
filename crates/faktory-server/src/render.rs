//! Bounded external renderer orchestration.

use std::{
    collections::{HashMap, hash_map::Entry},
    fmt,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::AbortHandle,
};

use crate::model::{
    GeometryFactsRecord, RenderedOutput, Repository, RepositoryError, validate_geometry_facts,
};

#[derive(Clone)]
pub struct RenderConfig {
    pub command: Vec<String>,
    pub queue_capacity: usize,
    pub concurrency: usize,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl fmt::Debug for RenderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RenderConfig")
            .field("queue_capacity", &self.queue_capacity)
            .field("concurrency", &self.concurrency)
            .field("timeout", &self.timeout)
            .field("max_output_bytes", &self.max_output_bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RenderJob {
    model_id: String,
    revision: String,
}

#[derive(Clone, Debug)]
pub struct RenderQueue {
    sender: mpsc::Sender<RenderJob>,
    admitted: Arc<Mutex<HashMap<RenderJob, usize>>>,
    repository: Repository,
    feeder: Arc<Mutex<Option<AbortHandle>>>,
    #[cfg(test)]
    worker: AbortHandle,
}

#[derive(Debug)]
pub struct RenderReservation {
    permit: mpsc::OwnedPermit<RenderJob>,
    admitted: Arc<Mutex<HashMap<RenderJob, usize>>>,
}

impl RenderReservation {
    pub fn submit(self, model_id: String, revision: String) {
        let Self { permit, admitted } = self;
        let job = RenderJob { model_id, revision };
        let should_send = match admitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(job.clone())
        {
            Entry::Vacant(entry) => {
                entry.insert(1);
                true
            }
            Entry::Occupied(mut entry) if *entry.get() == 1 => {
                *entry.get_mut() = 2;
                true
            }
            Entry::Occupied(_) => false,
        };
        if should_send {
            permit.send(job);
        }
    }
}

impl RenderQueue {
    pub fn start(repository: Repository, config: RenderConfig) -> Result<Self, RepositoryError> {
        if config.command.is_empty()
            || config.queue_capacity == 0
            || config.concurrency == 0
            || config.timeout.is_zero()
            || config.max_output_bytes < 12
        {
            return Err(RepositoryError::Invalid);
        }
        let (sender, mut receiver) = mpsc::channel::<RenderJob>(config.queue_capacity);
        let semaphore = Arc::new(Semaphore::new(config.concurrency));
        let admitted = Arc::new(Mutex::new(HashMap::new()));
        let worker_admitted = admitted.clone();
        let worker_repository = repository.clone();
        let worker = tokio::spawn(async move {
            loop {
                let Ok(permit) = semaphore.clone().acquire_owned().await else {
                    return;
                };
                let Some(job) = receiver.recv().await else {
                    return;
                };
                let repository = worker_repository.clone();
                let config = config.clone();
                let admitted = worker_admitted.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    run_job(&repository, &config, &job).await;
                    let mut admitted = admitted
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Entry::Occupied(mut entry) = admitted.entry(job) {
                        if *entry.get() == 1 {
                            entry.remove();
                        } else {
                            *entry.get_mut() -= 1;
                        }
                    }
                });
            }
        });
        #[cfg(test)]
        let worker = worker.abort_handle();
        #[cfg(not(test))]
        drop(worker);
        Ok(Self {
            sender,
            admitted,
            repository,
            feeder: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            worker,
        })
    }

    pub async fn reserve(&self) -> Result<RenderReservation, RepositoryError> {
        let permit = self.sender.clone().reserve_owned().await.map_err(|_| {
            self.repository.mark_degraded();
            tracing::warn!("render queue stopped before work could be admitted");
            RepositoryError::Unavailable
        })?;
        Ok(RenderReservation {
            permit,
            admitted: self.admitted.clone(),
        })
    }

    pub async fn reconcile(&self, repository: &Repository) -> Result<usize, RepositoryError> {
        let jobs = repository.reconcile().await?;
        let count = jobs.len();
        if jobs.is_empty() {
            return Ok(0);
        }
        let queue = self.clone();
        let feeder = tokio::spawn(async move {
            for (model_id, revision) in jobs {
                let Ok(reservation) = queue.reserve().await else {
                    return;
                };
                reservation.submit(model_id, revision);
            }
        })
        .abort_handle();
        let previous = self
            .feeder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(feeder);
        if let Some(previous) = previous {
            previous.abort();
        }
        Ok(count)
    }

    #[cfg(test)]
    fn stop_worker(&self) {
        self.worker.abort();
    }

    #[cfg(test)]
    fn admitted_len(&self) -> usize {
        self.admitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .sum()
    }
}

async fn run_job(repository: &Repository, config: &RenderConfig, job: &RenderJob) {
    if !claim_render(repository, job).await {
        return;
    }
    let result = render(repository, config, job).await;
    persist_terminal(repository, job, &result).await;
}

async fn claim_render(repository: &Repository, job: &RenderJob) -> bool {
    let mut delay = Duration::from_millis(50);
    for attempt in 0..5 {
        match repository.set_rendering(&job.model_id, &job.revision).await {
            Ok(claimed) => return claimed,
            Err(_) if attempt < 4 => {
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(_) => break,
        }
    }
    repository.mark_degraded();
    tracing::warn!("render claim remains recoverable only by startup reconciliation");
    false
}

async fn persist_terminal(
    repository: &Repository,
    job: &RenderJob,
    result: &Result<RenderedOutput, &'static str>,
) {
    let mut delay = Duration::from_millis(50);
    for attempt in 0..5 {
        let persisted = match result {
            Ok(output) => {
                match repository
                    .complete_render(&job.model_id, &job.revision, output.clone())
                    .await
                {
                    Err(RepositoryError::Conflict) => {
                        repository
                            .fail_render(
                                &job.model_id,
                                &job.revision,
                                "rendered artifacts conflict with immutable storage",
                            )
                            .await
                    }
                    result => result,
                }
            }
            Err(message) => {
                repository
                    .fail_render(&job.model_id, &job.revision, message)
                    .await
            }
        };
        if persisted.is_ok() {
            return;
        }
        if attempt < 4 {
            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2);
        }
    }
    repository.mark_degraded();
    tracing::warn!("render terminal state remains recoverable only by startup reconciliation");
}

async fn render(
    repository: &Repository,
    config: &RenderConfig,
    job: &RenderJob,
) -> Result<RenderedOutput, &'static str> {
    let source = repository
        .source(&job.model_id, &job.revision)
        .await
        .map_err(|_| "source unavailable")?;
    let directory = tempfile::tempdir().map_err(|_| "temporary storage unavailable")?;
    let source_path = directory.path().join("source.py");
    let glb_path = directory.path().join("model.glb");
    let preview_path = directory.path().join("preview.svg");
    let facts_path = directory.path().join("facts.json");
    tokio::fs::write(&source_path, source)
        .await
        .map_err(|_| "temporary storage unavailable")?;
    let mut command = tokio::process::Command::new(&config.command[0]);
    command
        .args(&config.command[1..])
        .arg(&source_path)
        .arg(&glb_path)
        .arg(&preview_path)
        .arg(&facts_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|_| "renderer could not start")?;
    let status = if let Ok(status) = tokio::time::timeout(config.timeout, child.wait()).await {
        status.map_err(|_| "renderer could not run")?
    } else {
        terminate_renderer(&mut child).await;
        return Err("render timed out");
    };
    if !status.success() {
        return Err("renderer rejected source");
    }
    let glb = read_glb(&glb_path, config.max_output_bytes).await?;
    let preview = read_svg(&preview_path, config.max_output_bytes).await?;
    let facts = read_facts(&facts_path, config.max_output_bytes).await?;
    Ok(RenderedOutput {
        glb,
        preview,
        facts,
    })
}

#[cfg(unix)]
async fn terminate_renderer(child: &mut tokio::process::Child) {
    if let Some(id) = child.id()
        && let Ok(id) = i32::try_from(id)
        && let Some(group) = rustix::process::Pid::from_raw(id)
    {
        let _kill_result =
            rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    drop(child.wait().await);
}

#[cfg(not(unix))]
async fn terminate_renderer(child: &mut tokio::process::Child) {
    drop(child.kill().await);
    drop(child.wait().await);
}

async fn read_glb(path: &Path, max_bytes: usize) -> Result<Bytes, &'static str> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| "renderer produced no artifact")?;
    let length = usize::try_from(metadata.len()).map_err(|_| "rendered artifact is too large")?;
    if length < 12 || length > max_bytes {
        return Err("rendered artifact has invalid size");
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| "rendered artifact is unavailable")?;
    if bytes.len() < 12 || bytes.len() > max_bytes {
        return Err("rendered artifact has invalid size");
    }
    if &bytes[0..4] != b"glTF"
        || u32::from_le_bytes(
            bytes[4..8]
                .try_into()
                .map_err(|_| "rendered artifact is invalid")?,
        ) != 2
        || usize::try_from(u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| "rendered artifact is invalid")?,
        ))
        .ok()
            != Some(bytes.len())
    {
        return Err("rendered artifact is invalid");
    }
    validate_glb_chunks(&bytes)?;
    Ok(Bytes::from(bytes))
}

fn validate_glb_chunks(bytes: &[u8]) -> Result<(), &'static str> {
    const JSON_CHUNK: u32 = 0x4e4f_534a;
    const BIN_CHUNK: u32 = 0x004e_4942;
    let mut offset = 12;
    let (json_type, json) = read_glb_chunk(bytes, &mut offset)?;
    if json_type != JSON_CHUNK || json.is_empty() {
        return Err("rendered artifact is invalid");
    }
    let mut values = serde_json::Deserializer::from_slice(json).into_iter::<serde_json::Value>();
    let Some(Ok(value)) = values.next() else {
        return Err("rendered artifact is invalid");
    };
    if !value.is_object()
        || !json[values.byte_offset()..]
            .iter()
            .all(|byte| *byte == b' ')
    {
        return Err("rendered artifact is invalid");
    }
    if offset < bytes.len() {
        let (bin_type, _) = read_glb_chunk(bytes, &mut offset)?;
        if bin_type != BIN_CHUNK {
            return Err("rendered artifact is invalid");
        }
    }
    if offset != bytes.len() {
        return Err("rendered artifact is invalid");
    }
    Ok(())
}

fn read_glb_chunk<'a>(
    bytes: &'a [u8],
    offset: &mut usize,
) -> Result<(u32, &'a [u8]), &'static str> {
    let header_end = offset
        .checked_add(8)
        .filter(|end| *end <= bytes.len())
        .ok_or("rendered artifact is invalid")?;
    let length = usize::try_from(u32::from_le_bytes(
        bytes[*offset..*offset + 4]
            .try_into()
            .map_err(|_| "rendered artifact is invalid")?,
    ))
    .map_err(|_| "rendered artifact is invalid")?;
    if length % 4 != 0 {
        return Err("rendered artifact is invalid");
    }
    let chunk_type = u32::from_le_bytes(
        bytes[*offset + 4..header_end]
            .try_into()
            .map_err(|_| "rendered artifact is invalid")?,
    );
    let chunk_end = header_end
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or("rendered artifact is invalid")?;
    *offset = chunk_end;
    Ok((chunk_type, &bytes[header_end..chunk_end]))
}

async fn read_svg(path: &Path, max_bytes: usize) -> Result<Bytes, &'static str> {
    let bytes = read_bounded(
        path,
        max_bytes,
        "renderer produced no preview",
        "rendered preview",
    )
    .await?;
    let svg = std::str::from_utf8(&bytes).map_err(|_| "rendered preview is invalid")?;
    validate_svg(svg)?;
    Ok(bytes)
}

async fn read_facts(path: &Path, max_bytes: usize) -> Result<GeometryFactsRecord, &'static str> {
    let bytes = read_bounded(
        path,
        max_bytes,
        "renderer produced no facts",
        "rendered facts",
    )
    .await?;
    let facts: GeometryFactsRecord =
        serde_json::from_slice(&bytes).map_err(|_| "rendered facts are invalid")?;
    validate_geometry_facts(facts).map_err(|_| "rendered facts are invalid")?;
    Ok(facts)
}

async fn read_bounded(
    path: &Path,
    max_bytes: usize,
    missing: &'static str,
    description: &'static str,
) -> Result<Bytes, &'static str> {
    let metadata = tokio::fs::metadata(path).await.map_err(|_| missing)?;
    let length = usize::try_from(metadata.len()).map_err(|_| match description {
        "rendered preview" => "rendered preview is too large",
        _ => "rendered facts are too large",
    })?;
    if length == 0 || length > max_bytes {
        return Err(match description {
            "rendered preview" => "rendered preview has invalid size",
            _ => "rendered facts have invalid size",
        });
    }
    let bytes = tokio::fs::read(path).await.map_err(|_| match description {
        "rendered preview" => "rendered preview is unavailable",
        _ => "rendered facts are unavailable",
    })?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(match description {
            "rendered preview" => "rendered preview has invalid size",
            _ => "rendered facts have invalid size",
        });
    }
    Ok(Bytes::from(bytes))
}

fn validate_svg(svg: &str) -> Result<(), &'static str> {
    let mut reader = Reader::from_str(svg);
    let mut depth = 0_usize;
    let mut root_seen = false;
    let mut root_closed = false;
    let mut declaration_seen = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| "rendered preview is invalid")?;
        match event {
            Event::Start(element) => {
                if root_closed || (depth == 0 && root_seen) {
                    return Err("rendered preview is invalid");
                }
                validate_svg_element(&element, reader.decoder(), depth == 0)?;
                root_seen = true;
                depth = depth.checked_add(1).ok_or("rendered preview is invalid")?;
            }
            Event::Empty(element) => {
                if root_closed || (depth == 0 && root_seen) {
                    return Err("rendered preview is invalid");
                }
                validate_svg_element(&element, reader.decoder(), depth == 0)?;
                root_seen = true;
                if depth == 0 {
                    root_closed = true;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or("rendered preview is invalid")?;
                if depth == 0 {
                    root_closed = true;
                }
            }
            Event::Decl(_) if !declaration_seen && !root_seen => declaration_seen = true,
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Comment(_) => {}
            Event::Eof if root_seen && root_closed && depth == 0 => return Ok(()),
            Event::Eof | Event::Decl(_) | Event::Text(_) => {
                return Err("rendered preview is invalid");
            }
            Event::CData(_) | Event::PI(_) | Event::DocType(_) | Event::GeneralRef(_) => {
                return Err("rendered preview contains active content");
            }
        }
    }
}

fn validate_svg_element(
    element: &BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
    root: bool,
) -> Result<(), &'static str> {
    let name = element.name();
    let name = name.as_ref();
    if (root && name != b"svg") || !matches!(name, b"svg" | b"g" | b"path") {
        return Err("rendered preview contains active content");
    }
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| "rendered preview is invalid")?;
        let key = attribute.key.as_ref();
        let local_key = key.rsplit(|byte| *byte == b':').next().unwrap_or(key);
        let lowercase_key = local_key.to_ascii_lowercase();
        if lowercase_key.len() > 2 && lowercase_key.starts_with(b"on") {
            return Err("rendered preview contains active content");
        }
        let value = attribute
            .decode_and_unescape_value(decoder)
            .map_err(|_| "rendered preview is invalid")?;
        let value = value.trim();
        if matches!(lowercase_key.as_slice(), b"href" | b"src")
            && !value.is_empty()
            && !value.starts_with('#')
        {
            return Err("rendered preview contains active content");
        }
        if lowercase_key == b"style" {
            let lowercase_value = value.to_ascii_lowercase();
            if lowercase_value.contains("url(")
                || lowercase_value.contains("@import")
                || lowercase_value.contains("expression(")
            {
                return Err("rendered preview contains active content");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::storage::{
        InMemoryObjectStore, ObjectStore, PutCondition, StorageError, StoredObject,
    };

    #[derive(Debug, Default)]
    struct TransientPutFailures {
        inner: InMemoryObjectStore,
        remaining: AtomicUsize,
    }

    #[async_trait]
    impl ObjectStore for TransientPutFailures {
        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            self.inner.get(key).await
        }

        async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
            self.inner.list(prefix).await
        }

        async fn put(
            &self,
            key: &str,
            bytes: Bytes,
            condition: PutCondition,
        ) -> Result<String, StorageError> {
            if key.ends_with("/model.json")
                && self
                    .remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
            {
                return Err(StorageError::Unavailable);
            }
            self.inner.put(key, bytes, condition).await
        }

        async fn delete(&self, key: &str, expected_etag: &str) -> Result<(), StorageError> {
            self.inner.delete(key, expected_etag).await
        }

        async fn ready(&self) -> Result<(), StorageError> {
            self.inner.ready().await
        }
    }

    fn glb_fixture(bin: Option<&[u8]>) -> Vec<u8> {
        let json = br#"{"asset":{"version":"2.0"}} "#;
        let length = 12 + 8 + json.len() + bin.map_or(0, |bin| 8 + bin.len());
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(length).expect("fixture length").to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(json.len())
                .expect("JSON length")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(b"JSON");
        bytes.extend_from_slice(json);
        if let Some(bin) = bin {
            bytes.extend_from_slice(&u32::try_from(bin.len()).expect("BIN length").to_le_bytes());
            bytes.extend_from_slice(b"BIN\0");
            bytes.extend_from_slice(bin);
        }
        bytes
    }

    fn rendered_output(glb: &'static [u8]) -> RenderedOutput {
        RenderedOutput {
            glb: Bytes::from_static(glb),
            preview: Bytes::from_static(b"<svg></svg>"),
            facts: GeometryFactsRecord {
                volume_cubic_millimeters: 1.0,
                size_millimeters: crate::model::GeometrySizeRecord {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            },
        }
    }

    #[tokio::test]
    async fn validates_complete_glb_v2_chunk_structure() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("model.glb");
        for valid in [glb_fixture(None), glb_fixture(Some(&[1, 2, 3, 0]))] {
            tokio::fs::write(&path, &valid)
                .await
                .expect("write fixture");
            assert_eq!(read_glb(&path, 100).await.expect("valid GLB"), valid);
        }

        let mut zero_json = b"glTF\x02\0\0\0\x14\0\0\0\0\0\0\0JSON".to_vec();
        let mut invalid_json = glb_fixture(None);
        invalid_json[20..48].copy_from_slice(b"null                        ");
        let mut invalid_utf8 = glb_fixture(None);
        invalid_utf8[20] = 0xff;
        let mut illegal_padding = glb_fixture(None);
        *illegal_padding.last_mut().expect("JSON padding") = b'\n';
        let mut truncated = glb_fixture(Some(&[1, 2, 3, 0]));
        truncated.truncate(truncated.len() - 1);
        let truncated_length = u32::try_from(truncated.len()).expect("fixture length");
        truncated[8..12].copy_from_slice(&truncated_length.to_le_bytes());
        for invalid in [
            b"glTF\x02\0\0\0\x0c\0\0\0".to_vec(),
            std::mem::take(&mut zero_json),
            invalid_json,
            invalid_utf8,
            illegal_padding,
            truncated,
            b"not a glb!!!".to_vec(),
        ] {
            tokio::fs::write(&path, invalid)
                .await
                .expect("write fixture");
            assert!(read_glb(&path, 100).await.is_err());
        }
    }

    #[tokio::test]
    async fn parses_exact_box_facts_and_rejects_invalid_facts() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("facts.json");
        let valid = br#"{"volume_cubic_millimeters":24,"size_millimeters":{"x":2,"y":3,"z":4}}"#;
        tokio::fs::write(&path, valid).await.expect("write facts");
        assert_eq!(
            read_facts(&path, valid.len()).await.expect("valid facts"),
            GeometryFactsRecord {
                volume_cubic_millimeters: 24.0,
                size_millimeters: crate::model::GeometrySizeRecord {
                    x: 2.0,
                    y: 3.0,
                    z: 4.0,
                },
            }
        );

        for invalid in [
            br#"{"volume_cubic_millimeters":24,"size_millimeters":{"x":2,"y":3}}"#.as_slice(),
            br#"{"volume_cubic_millimeters":-1,"size_millimeters":{"x":2,"y":3,"z":4}}"#,
            br#"{"volume_cubic_millimeters":1e400,"size_millimeters":{"x":2,"y":3,"z":4}}"#,
            br#"{"volume_cubic_millimeters":24,"size_millimeters":{"x":2,"y":3,"z":4},"extra":true}"#,
            b"not json",
        ] {
            tokio::fs::write(&path, invalid).await.expect("write facts");
            assert_eq!(read_facts(&path, 1024).await, Err("rendered facts are invalid"));
        }
        tokio::fs::write(&path, valid).await.expect("write facts");
        assert_eq!(
            read_facts(&path, valid.len() - 1).await,
            Err("rendered facts have invalid size")
        );
    }

    #[tokio::test]
    async fn validates_svg_root_and_rejects_active_content() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("preview.svg");
        for valid in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><path/></svg>",
            "<?xml version=\"1.0\"?><svg></svg>",
        ] {
            tokio::fs::write(&path, valid).await.expect("write SVG");
            read_svg(&path, 1024).await.expect("valid SVG");
        }
        for invalid in [
            "not svg",
            "<svg>",
            "<svg><g></svg>",
            "<svg><script>alert(1)</script></svg>",
            "<svg onload=\"alert(1)\"></svg>",
            "<svg><path href=\"java&#x73;cript:alert(1)\"/></svg>",
            "<!DOCTYPE svg [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]><svg></svg>",
        ] {
            tokio::fs::write(&path, invalid).await.expect("write SVG");
            assert!(read_svg(&path, 1024).await.is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn renderer_receives_four_paths_and_returns_all_outputs() {
        let directory = tempfile::tempdir().expect("temp directory");
        let script_path = directory.path().join("renderer.sh");
        tokio::fs::write(
            &script_path,
            b"test \"$#\" -eq 4 || exit 2\nprintf 'glTF\\002\\000\\000\\000\\060\\000\\000\\000\\034\\000\\000\\000JSON{\"asset\":{\"version\":\"2.0\"}} ' > \"$2\"\nprintf '<svg></svg>' > \"$3\"\nprintf '{\"volume_cubic_millimeters\":24,\"size_millimeters\":{\"x\":2,\"y\":3,\"z\":4}}' > \"$4\"\n",
        )
        .await
        .expect("write renderer");
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        let output = render(
            &repository,
            &RenderConfig {
                command: vec![
                    "/bin/sh".to_owned(),
                    script_path.to_string_lossy().into_owned(),
                ],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 1024,
            },
            &RenderJob {
                model_id: model.id,
                revision: model.desired_source_revision,
            },
        )
        .await
        .expect("render outputs");
        assert_eq!(output.glb, glb_fixture(None));
        assert_eq!(output.preview, Bytes::from_static(b"<svg></svg>"));
        assert!((output.facts.volume_cubic_millimeters - 24.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn terminal_state_write_retries_transient_failures() {
        let store = Arc::new(TransientPutFailures::default());
        let repository = Repository::new(store.clone(), 1);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        let job = RenderJob {
            model_id: model.id.clone(),
            revision: model.desired_source_revision,
        };
        assert!(
            repository
                .set_rendering(&job.model_id, &job.revision)
                .await
                .expect("claim render")
        );
        store.remaining.store(2, Ordering::SeqCst);

        persist_terminal(&repository, &job, &Err("safe failure")).await;

        let model = repository
            .get_model(&model.id)
            .await
            .expect("model after retries")
            .record;
        assert_eq!(model.render_state, crate::model::StoredRenderState::Failed);
        assert_eq!(model.render_error, "safe failure");
        repository
            .ready()
            .await
            .expect("repository remains healthy");
    }

    #[tokio::test]
    async fn immutable_output_conflict_fails_desired_revision_and_preserves_last_good() {
        let store = Arc::new(InMemoryObjectStore::default());
        let repository = Repository::new(store.clone(), 1);
        let first = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("accept first source");
        let first_output = rendered_output(b"first-glb");
        repository
            .complete_render(
                &first.id,
                &first.desired_source_revision,
                first_output.clone(),
            )
            .await
            .expect("complete first render");
        let replacement = repository
            .edit_model(
                &first.id,
                &first.desired_source_revision,
                None,
                Some(&[crate::model::SourcePatch {
                    old: "first".to_owned(),
                    new: "second".to_owned(),
                }]),
            )
            .await
            .expect("accept replacement")
            .record;
        assert!(
            repository
                .set_rendering(&replacement.id, &replacement.desired_source_revision)
                .await
                .expect("claim render")
        );
        store
            .put(
                &crate::model::geometry_key(&replacement.id, &replacement.desired_source_revision),
                Bytes::from_static(b"conflicting-glb"),
                PutCondition::Absent,
            )
            .await
            .expect("store conflicting GLB");
        let replacement_output = rendered_output(b"replacement-glb");

        persist_terminal(
            &repository,
            &RenderJob {
                model_id: replacement.id.clone(),
                revision: replacement.desired_source_revision.clone(),
            },
            &Ok(replacement_output),
        )
        .await;

        let failed = repository
            .get_model(&replacement.id)
            .await
            .expect("failed replacement")
            .record;
        assert_eq!(failed.render_state, crate::model::StoredRenderState::Failed);
        assert_eq!(
            failed.render_error,
            "rendered artifacts conflict with immutable storage"
        );
        assert_eq!(
            failed.current_successful_source_revision,
            first.desired_source_revision
        );
        assert_eq!(failed.current_successful_facts, Some(first_output.facts));
    }

    #[tokio::test]
    async fn stale_terminal_completion_does_not_fail_newer_revision() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let first = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("accept first source");
        assert!(
            repository
                .set_rendering(&first.id, &first.desired_source_revision)
                .await
                .expect("claim first render")
        );
        let replacement = repository
            .edit_model(
                &first.id,
                &first.desired_source_revision,
                None,
                Some(&[crate::model::SourcePatch {
                    old: "first".to_owned(),
                    new: "second".to_owned(),
                }]),
            )
            .await
            .expect("accept replacement")
            .record;
        let stale_output = rendered_output(b"stale-glb");

        persist_terminal(
            &repository,
            &RenderJob {
                model_id: first.id.clone(),
                revision: first.desired_source_revision,
            },
            &Ok(stale_output),
        )
        .await;

        let current = repository
            .get_model(&replacement.id)
            .await
            .expect("newer model")
            .record;
        assert_eq!(
            current.desired_source_revision,
            replacement.desired_source_revision
        );
        assert_eq!(
            current.render_state,
            crate::model::StoredRenderState::Pending
        );
        assert!(current.render_error.is_empty());
    }

    #[tokio::test]
    async fn terminal_state_write_exhaustion_degrades_readiness_across_clones() {
        let store = Arc::new(TransientPutFailures::default());
        let repository = Repository::new(store.clone(), 1);
        let readiness_repository = repository.clone();
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        let job = RenderJob {
            model_id: model.id,
            revision: model.desired_source_revision,
        };
        assert!(
            repository
                .set_rendering(&job.model_id, &job.revision)
                .await
                .expect("claim render")
        );
        store.remaining.store(5, Ordering::SeqCst);

        persist_terminal(&repository, &job, &Err("safe failure")).await;

        assert_eq!(
            readiness_repository.ready().await,
            Err(RepositoryError::Unavailable)
        );
        assert_eq!(
            repository
                .get_model(&job.model_id)
                .await
                .expect("stranded model")
                .record
                .render_state,
            crate::model::StoredRenderState::Rendering
        );
    }

    #[tokio::test]
    async fn cancelled_and_dropped_reservations_free_capacity_without_admission() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let queue = RenderQueue::start(
            repository.clone(),
            RenderConfig {
                command: vec!["unused".to_owned()],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 12,
            },
        )
        .expect("render queue");
        let reservation = queue.reserve().await.expect("first reservation");
        for _ in 0..20 {
            assert!(
                tokio::time::timeout(Duration::from_millis(1), queue.reserve())
                    .await
                    .is_err()
            );
        }
        assert_eq!(queue.admitted_len(), 0);
        assert!(repository.list_models().await.expect("models").is_empty());

        drop(reservation);
        let recovered = tokio::time::timeout(Duration::from_secs(1), queue.reserve())
            .await
            .expect("reservation wait")
            .expect("capacity recovered");
        drop(recovered);
        assert_eq!(queue.admitted_len(), 0);
    }

    #[tokio::test]
    async fn closed_admission_is_unavailable_and_degrades_readiness() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let queue = RenderQueue::start(
            repository.clone(),
            RenderConfig {
                command: vec!["unused".to_owned()],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(1),
                max_output_bytes: 12,
            },
        )
        .expect("render queue");
        queue.stop_worker();
        queue.sender.closed().await;

        assert!(matches!(
            queue.reserve().await,
            Err(RepositoryError::Unavailable)
        ));
        assert_eq!(repository.ready().await, Err(RepositoryError::Unavailable));
        assert!(repository.list_models().await.expect("models").is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_queue_accepts_distinct_jobs_and_suppresses_duplicate_renders() {
        let directory = tempfile::tempdir().expect("temp directory");
        let invocations = directory.path().join("invocations");
        let script = format!(
            "sleep 0.05; printf x >> '{}'; exit 1",
            invocations.display()
        );
        let config = RenderConfig {
            command: vec!["/bin/sh".to_owned(), "-c".to_owned(), script],
            queue_capacity: 1,
            concurrency: 1,
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
        };
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let queue = RenderQueue::start(repository.clone(), config).expect("render queue");
        let mut models = Vec::new();
        for (id, name) in [("one", "One"), ("two", "Two"), ("three", "Three")] {
            models.push(
                repository
                    .create_model(id, name, b"source")
                    .await
                    .expect("accept source"),
            );
        }
        queue.reserve().await.expect("first admission").submit(
            models[0].id.clone(),
            models[0].desired_source_revision.clone(),
        );
        for _ in 0..20 {
            queue.reserve().await.expect("duplicate admission").submit(
                models[0].id.clone(),
                models[0].desired_source_revision.clone(),
            );
            assert!(queue.admitted_len() <= 2);
        }
        for model in &models[1..] {
            queue
                .reserve()
                .await
                .expect("distinct admission")
                .submit(model.id.clone(), model.desired_source_revision.clone());
            assert!(queue.admitted_len() <= 2);
        }

        for _ in 0..100 {
            let all_failed = repository
                .list_models()
                .await
                .expect("list models")
                .iter()
                .all(|model| model.render_state == crate::model::StoredRenderState::Failed);
            if all_failed {
                let invocations = tokio::fs::read(&invocations).await.expect("invocations");
                assert_eq!(invocations, b"xxx");
                assert_eq!(queue.admitted_len(), 0);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("accepted render jobs did not drain");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reconciliation_installs_one_bounded_feeder_without_waiting_for_backlog() {
        let directory = tempfile::tempdir().expect("temp directory");
        let invocations = directory.path().join("reconciled");
        let script = format!(
            "sleep 0.05; printf x >> '{}'; exit 1",
            invocations.display()
        );
        let config = RenderConfig {
            command: vec!["/bin/sh".to_owned(), "-c".to_owned(), script],
            queue_capacity: 1,
            concurrency: 1,
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
        };
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        for index in 0..6 {
            repository
                .create_model(
                    &format!("part-{index}"),
                    &format!("Part {index}"),
                    b"source",
                )
                .await
                .expect("accept source");
        }
        let queue = RenderQueue::start(repository.clone(), config).expect("render queue");

        let count = tokio::time::timeout(Duration::from_millis(100), queue.reconcile(&repository))
            .await
            .expect("reconciliation should not wait for rendering")
            .expect("reconciliation scan");
        assert_eq!(count, 6);

        for _ in 0..200 {
            assert!(queue.admitted_len() <= 2);
            let all_failed = repository
                .list_models()
                .await
                .expect("list models")
                .iter()
                .all(|model| model.render_state == crate::model::StoredRenderState::Failed);
            if all_failed {
                assert_eq!(
                    tokio::fs::read(&invocations)
                        .await
                        .expect("reconciled invocations"),
                    b"xxxxxx"
                );
                assert_eq!(queue.admitted_len(), 0);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("reconciliation backlog did not drain");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_terminates_renderer_descendants() {
        let directory = tempfile::tempdir().expect("temp directory");
        let pid_path = directory.path().join("descendant.pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pid_path.display());
        let config = RenderConfig {
            command: vec!["/bin/sh".to_owned(), "-c".to_owned(), script],
            queue_capacity: 1,
            concurrency: 1,
            timeout: Duration::from_millis(500),
            max_output_bytes: 1024,
        };
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("accept source");
        let job = RenderJob {
            model_id: model.id,
            revision: model.desired_source_revision,
        };

        assert_eq!(
            render(&repository, &config, &job).await,
            Err("render timed out")
        );
        let pid = tokio::fs::read_to_string(&pid_path)
            .await
            .expect("descendant pid")
            .trim()
            .parse::<i32>()
            .expect("numeric pid");
        let pid = rustix::process::Pid::from_raw(pid).expect("positive pid");
        for _ in 0..20 {
            if rustix::process::test_kill_process(pid).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("renderer descendant survived timeout");
    }
}
