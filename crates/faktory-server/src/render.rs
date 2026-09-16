//! Bounded external renderer orchestration.

use std::{
    collections::{BTreeSet, HashMap, hash_map::Entry},
    fmt,
    path::{Component, Path, PathBuf},
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
    io::AsyncWriteExt as _,
    sync::{Semaphore, mpsc},
    task::AbortHandle,
};

use crate::model::{
    GeometryFactsRecord, OutputManifest, RenderedModelOutput, RenderedOutput, Repository,
    RepositoryError, ShadedProjectionImages, TechnicalProjection, TechnicalProjectionImages,
    validate_geometry_facts, validate_output_summaries,
};
use crate::visual::{UnavailableVisualRenderer, VisualCoordinator, VisualRendererError};

pub(crate) const PROJECTION_WIDTH: u32 = 640;
pub(crate) const PROJECTION_HEIGHT: u32 = 480;
pub(crate) const MAX_PROJECTION_IMAGE_BYTES: usize = 512 * 1024;
pub(crate) const MAX_VISUAL_IMAGE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_GLB_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_BUNDLE_BYTES: u64 = 192 * 1024 * 1024;

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

#[derive(serde::Serialize)]
struct DependencyManifest<'a> {
    format: &'static str,
    root_model_id: &'a str,
    nodes: Vec<DependencyNode<'a>>,
}

#[derive(serde::Serialize)]
struct DependencyNode<'a> {
    model_id: &'a str,
    package: &'a str,
    project_revision: &'a str,
    dependencies: &'a [String],
}

#[derive(Clone, Debug)]
pub struct RenderQueue {
    sender: mpsc::Sender<RenderJob>,
    admitted: Arc<Mutex<HashMap<RenderJob, usize>>>,
    repository: Repository,
    visual: VisualCoordinator,
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
        Self::start_with_visual(
            repository,
            config,
            VisualCoordinator::new(Arc::new(UnavailableVisualRenderer), Duration::from_secs(1)),
        )
    }

    pub fn start_with_visual(
        repository: Repository,
        config: RenderConfig,
        visual: VisualCoordinator,
    ) -> Result<Self, RepositoryError> {
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
        let worker_visual = visual.clone();
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
                let visual = worker_visual.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    run_job(&repository, &config, &visual, &job).await;
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
            visual,
            feeder: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            worker,
        })
    }

    #[must_use]
    pub fn visual(&self) -> VisualCoordinator {
        self.visual.clone()
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

async fn run_job(
    repository: &Repository,
    config: &RenderConfig,
    visual: &VisualCoordinator,
    job: &RenderJob,
) {
    if !claim_render(repository, job).await {
        return;
    }
    let result = render(repository, config, visual, job).await;
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
            Err(error) => {
                repository.mark_degraded();
                tracing::warn!(
                    repository.error.kind = error.kind(),
                    "render claim remains recoverable only by startup reconciliation"
                );
                return false;
            }
        }
    }
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
        match persisted {
            Ok(()) => return,
            Err(_) if attempt < 4 => {
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(error) => {
                repository.mark_degraded();
                tracing::warn!(
                    repository.error.kind = error.kind(),
                    "render terminal state remains recoverable only by startup reconciliation"
                );
                return;
            }
        }
    }
}

async fn render(
    repository: &Repository,
    config: &RenderConfig,
    visual: &VisualCoordinator,
    job: &RenderJob,
) -> Result<RenderedOutput, &'static str> {
    let directory = tempfile::tempdir().map_err(|_| "temporary storage unavailable")?;
    let (project_root, entrypoint, dependency_root) =
        materialize_render_inputs(repository, directory.path(), job).await?;
    let output_root = directory.path().join("output");
    let mut command = tokio::process::Command::new(&config.command[0]);
    command
        .args(&config.command[1..])
        .arg(&project_root)
        .arg(&entrypoint)
        .arg(&dependency_root)
        .arg(&output_root)
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
    read_rendered_bundle(&output_root, config.max_output_bytes, visual).await
}

async fn read_rendered_bundle(
    output_root: &Path,
    max_bytes: usize,
    visual: &VisualCoordinator,
) -> Result<RenderedOutput, &'static str> {
    let manifest = read_bounded(
        &output_root.join("outputs.json"),
        max_bytes,
        "renderer produced no output manifest",
        "rendered manifest",
    )
    .await?;
    let parsed: OutputManifest =
        serde_json::from_slice(&manifest).map_err(|_| "rendered output manifest is invalid")?;
    if parsed.format != OutputManifest::FORMAT
        || parsed.outputs.is_empty()
        || parsed.outputs.len() > 64
        || parsed
            .canonical_bytes()
            .map_err(|_| "rendered output manifest is invalid")?
            != manifest
    {
        return Err("rendered output manifest is invalid");
    }
    validate_output_summaries(&parsed.outputs)
        .map_err(|_| "rendered output manifest is invalid")?;
    validate_bundle_size(output_root, &parsed).await?;
    let mut outputs = Vec::with_capacity(parsed.outputs.len());
    for summary in parsed.outputs {
        let root = output_root.join("outputs").join(&summary.output_id);
        let glb = read_glb(&root.join("model.glb"), max_bytes).await?;
        let preview = read_svg(&root.join("preview.svg"), max_bytes).await?;
        let facts = read_facts(&root.join("facts.json"), max_bytes).await?;
        if facts != summary.facts {
            return Err("rendered facts do not match output manifest");
        }
        let mut images = Vec::with_capacity(TechnicalProjection::ALL.len());
        for projection in TechnicalProjection::ALL {
            let svg = read_svg(
                &root
                    .join("projections")
                    .join(format!("{}.svg", projection.as_str())),
                max_bytes,
            )
            .await?;
            images.push(rasterize_projection(&svg)?);
        }
        let [isometric, front, back, left, right, top, bottom] = images
            .try_into()
            .map_err(|_| "renderer produced incomplete projections")?;
        let projections = TechnicalProjectionImages {
            isometric,
            front,
            back,
            left,
            right,
            top,
            bottom,
        };
        let shaded = if summary.primary {
            Some(render_shaded(visual, glb.clone()).await?)
        } else {
            None
        };
        outputs.push(RenderedModelOutput {
            summary,
            glb,
            preview,
            projections,
            shaded,
        });
    }
    Ok(RenderedOutput { manifest, outputs })
}

async fn validate_bundle_size(
    output_root: &Path,
    manifest: &OutputManifest,
) -> Result<(), &'static str> {
    let mut total = 0_u64;
    add_bundle_file_size(&mut total, &output_root.join("outputs.json")).await?;
    for summary in &manifest.outputs {
        let root = output_root.join("outputs").join(&summary.output_id);
        for relative in ["model.glb", "preview.svg", "facts.json"] {
            add_bundle_file_size(&mut total, &root.join(relative)).await?;
        }
        for projection in TechnicalProjection::ALL {
            add_bundle_file_size(
                &mut total,
                &root
                    .join("projections")
                    .join(format!("{}.svg", projection.as_str())),
            )
            .await?;
        }
    }
    Ok(())
}

async fn add_bundle_file_size(total: &mut u64, path: &Path) -> Result<(), &'static str> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| "rendered output bundle is invalid")?;
    if !metadata.is_file() {
        return Err("rendered output bundle is invalid");
    }
    *total =
        checked_bundle_total(*total, metadata.len()).ok_or("rendered output bundle is invalid")?;
    Ok(())
}

fn checked_bundle_total(total: u64, file_size: u64) -> Option<u64> {
    total
        .checked_add(file_size)
        .filter(|total| *total <= MAX_BUNDLE_BYTES)
}

#[allow(clippy::too_many_lines)]
async fn materialize_render_inputs(
    repository: &Repository,
    staging_root: &Path,
    job: &RenderJob,
) -> Result<(PathBuf, String, PathBuf), &'static str> {
    let closure = repository
        .resolve_project_closure(&job.model_id, &job.revision)
        .await
        .map_err(|_| "model dependency closure unavailable")?;
    let project = closure
        .iter()
        .find(|item| item.identity.model_id == job.model_id)
        .ok_or("project unavailable")?
        .project
        .clone();
    let project_root = staging_root.join("project");
    let dependency_root = staging_root.join("dependencies");
    tokio::fs::create_dir(&project_root)
        .await
        .map_err(|_| "temporary storage unavailable")?;
    tokio::fs::create_dir(&dependency_root)
        .await
        .map_err(|_| "temporary storage unavailable")?;

    let mut project_paths = BTreeSet::new();
    for file in &project.files {
        materialize_file(
            &project_root,
            &file.path,
            file.content.as_bytes(),
            &mut project_paths,
        )
        .await
        .map_err(|()| "project unavailable")?;
    }

    let mut dependency_paths = BTreeSet::new();
    materialize_file(
        &dependency_root,
        "faktory_models/__init__.py",
        b"",
        &mut dependency_paths,
    )
    .await
    .map_err(|()| "temporary storage unavailable")?;
    for resolved in &closure {
        let package = format!(
            "faktory_models/m_{}",
            resolved.identity.model_id.replace('-', "_")
        );
        let mut exported = false;
        for file in resolved.project.files.iter().filter(|file| {
            file.path == "faktory_model/__init__.py" || file.path.starts_with("faktory_model/")
        }) {
            exported = true;
            let suffix = file
                .path
                .strip_prefix("faktory_model/")
                .ok_or("model dependency closure unavailable")?;
            materialize_file(
                &dependency_root,
                &format!("{package}/{suffix}"),
                file.content.as_bytes(),
                &mut dependency_paths,
            )
            .await
            .map_err(|()| "model dependency closure unavailable")?;
        }
        if !exported && resolved.identity.model_id == job.model_id {
            materialize_file(
                &dependency_root,
                &format!("{package}/__init__.py"),
                b"",
                &mut dependency_paths,
            )
            .await
            .map_err(|()| "temporary storage unavailable")?;
        }
    }
    let manifest = DependencyManifest {
        format: "faktory-model-dependencies-v1",
        root_model_id: &job.model_id,
        nodes: closure
            .iter()
            .map(|item| DependencyNode {
                model_id: &item.identity.model_id,
                package: &item.identity.package,
                project_revision: &item.identity.project_revision,
                dependencies: &item.identity.dependencies,
            })
            .collect(),
    };
    let mut manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|_| "model dependency closure unavailable")?;
    manifest_bytes.push(b'\n');
    materialize_file(
        &dependency_root,
        "dependencies.json",
        &manifest_bytes,
        &mut dependency_paths,
    )
    .await
    .map_err(|()| "temporary storage unavailable")?;
    safe_relative_path(&project.entrypoint).map_err(|()| "project unavailable")?;
    Ok((project_root, project.entrypoint, dependency_root))
}

async fn materialize_file(
    root: &Path,
    relative: &str,
    content: &[u8],
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), ()> {
    let relative = safe_relative_path(relative)?;
    if !paths.insert(relative.clone()) {
        return Err(());
    }
    let destination = root.join(relative);
    let parent = destination.parent().ok_or(())?;
    tokio::fs::create_dir_all(parent).await.map_err(|_| ())?;
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .await
        .map_err(|_| ())?;
    output.write_all(content).await.map_err(|_| ())?;
    output.flush().await.map_err(|_| ())
}

fn safe_relative_path(path: &str) -> Result<PathBuf, ()> {
    if path.is_empty() {
        return Err(());
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(());
    }
    Ok(path.to_owned())
}

async fn render_shaded(
    visual: &VisualCoordinator,
    glb: Bytes,
) -> Result<ShadedProjectionImages, &'static str> {
    let shaded = visual
        .render_canonical(glb)
        .await
        .map_err(|error| match error {
            VisualRendererError::Unavailable => "visual renderer unavailable",
            VisualRendererError::InvalidResponse => "visual renderer returned invalid output",
        })?;
    let [
        shaded_isometric,
        shaded_front,
        shaded_back,
        shaded_left,
        shaded_right,
        shaded_top,
        shaded_bottom,
    ] = shaded
        .images
        .into_iter()
        .map(|(_, image)| image)
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "visual renderer produced incomplete projections")?;
    Ok(ShadedProjectionImages {
        isometric: shaded_isometric,
        front: shaded_front,
        back: shaded_back,
        left: shaded_left,
        right: shaded_right,
        top: shaded_top,
        bottom: shaded_bottom,
    })
}

fn rasterize_projection(svg: &[u8]) -> Result<Bytes, &'static str> {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default())
        .map_err(|_| "rendered projection is invalid")?;
    let expected_size =
        resvg::tiny_skia::Size::from_wh(640.0, 480.0).ok_or("invalid projection dimensions")?;
    if tree.size() != expected_size {
        return Err("rendered projection has invalid dimensions");
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(PROJECTION_WIDTH, PROJECTION_HEIGHT)
        .ok_or("projection rasterization failed")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );
    let png = pixmap
        .encode_png()
        .map_err(|_| "projection encoding failed")?;
    validate_projection_png(&png)?;
    Ok(Bytes::from(png))
}

pub(crate) fn validate_projection_png(png: &[u8]) -> Result<(), &'static str> {
    validate_png(png, MAX_PROJECTION_IMAGE_BYTES)
}

pub(crate) fn validate_visual_png(png: &[u8]) -> Result<(), &'static str> {
    validate_png(png, MAX_VISUAL_IMAGE_BYTES)
}

fn validate_png(png: &[u8], max_bytes: usize) -> Result<(), &'static str> {
    if png.len() > max_bytes
        || png.len() < 24
        || &png[..8] != b"\x89PNG\r\n\x1a\n"
        || &png[12..16] != b"IHDR"
        || u32::from_be_bytes(
            png[16..20]
                .try_into()
                .map_err(|_| "rendered projection is invalid")?,
        ) != PROJECTION_WIDTH
        || u32::from_be_bytes(
            png[20..24]
                .try_into()
                .map_err(|_| "rendered projection is invalid")?,
        ) != PROJECTION_HEIGHT
    {
        return Err("rendered projection has invalid size");
    }
    let decoded =
        resvg::tiny_skia::Pixmap::decode_png(png).map_err(|_| "rendered projection is invalid")?;
    if decoded.width() != PROJECTION_WIDTH
        || decoded.height() != PROJECTION_HEIGHT
        || decoded.pixels().iter().any(|pixel| pixel.alpha() != 255)
    {
        return Err("rendered projection is invalid");
    }
    Ok(())
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
    let max_bytes = max_bytes.min(MAX_GLB_BYTES);
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
    use std::{
        collections::BTreeSet,
        io,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::prelude::*;

    use super::*;
    use crate::model::project::ProjectFile;
    use crate::storage::{
        InMemoryObjectStore, ObjectStore, PutCondition, StorageError, StoredObject,
    };
    use crate::visual::{VisualRenderResult, VisualRenderSpec, VisualRenderer};

    const PRIVATE_SENTINEL: &str = "private-render-repository-material";

    #[derive(Debug)]
    struct SuccessfulVisualRenderer;

    #[async_trait]
    impl VisualRenderer for SuccessfulVisualRenderer {
        async fn render(
            &self,
            _glb: Bytes,
            spec: VisualRenderSpec,
        ) -> Result<VisualRenderResult, VisualRendererError> {
            let mut pixmap = resvg::tiny_skia::Pixmap::new(PROJECTION_WIDTH, PROJECTION_HEIGHT)
                .expect("visual pixmap");
            pixmap.fill(resvg::tiny_skia::Color::WHITE);
            let image = Bytes::from(pixmap.encode_png().expect("visual PNG"));
            let names: Vec<&str> = match spec {
                VisualRenderSpec::Canonical { .. } => TechnicalProjection::ALL
                    .into_iter()
                    .map(TechnicalProjection::as_str)
                    .collect(),
                VisualRenderSpec::View { .. } => vec!["view"],
            };
            Ok(VisualRenderResult {
                images: names
                    .into_iter()
                    .map(|name| (name.to_owned(), image.clone()))
                    .collect(),
            })
        }
    }

    fn visual() -> VisualCoordinator {
        VisualCoordinator::new(Arc::new(SuccessfulVisualRenderer), Duration::from_secs(1))
    }

    #[derive(Debug, Default)]
    struct TransientPutFailures {
        inner: InMemoryObjectStore,
        remaining: AtomicUsize,
        attempts: AtomicUsize,
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
            if key.ends_with("/model.json") {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                if self
                    .remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
                {
                    return Err(StorageError::Unavailable);
                }
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

    #[derive(Clone, Debug, Default)]
    struct RecordingWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for RecordingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("recording writer").extend(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn warning_subscriber(output: &RecordingWriter) -> tracing::Dispatch {
        let writer = output.clone();
        tracing::Dispatch::new(
            tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .event_format(crate::observability::JsonEventFormatter)
                    .with_writer(move || writer.clone())
                    .with_filter(crate::observability::json_filter()),
            ),
        )
    }

    fn assert_repository_warning(output: &RecordingWriter, message: &str) {
        let stdout = String::from_utf8(output.0.lock().expect("recording writer").clone())
            .expect("UTF-8 stdout");
        assert!(!stdout.contains(PRIVATE_SENTINEL));
        let events = stdout
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON event"))
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1, "unexpected warning events: {stdout}");
        let event = events[0].as_object().expect("event object");
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["message"], message);
        assert_eq!(event["repository.error.kind"], "unavailable");
        assert_eq!(event["target"], "faktory_server::render");
        assert!(event["timestamp"].is_string());
        assert_eq!(
            event.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "level",
                "message",
                "repository.error.kind",
                "target",
                "timestamp",
            ])
        );
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

    fn pseudo_projection_png() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&PROJECTION_WIDTH.to_be_bytes());
        bytes.extend_from_slice(&PROJECTION_HEIGHT.to_be_bytes());
        bytes
    }

    fn rendered_output(glb: &'static [u8]) -> RenderedOutput {
        let summary = crate::model::ModelOutputSummaryRecord {
            output_id: "primary".to_owned(),
            role: crate::model::OutputRoleRecord::Assembly,
            primary: true,
            facts: GeometryFactsRecord {
                volume_cubic_millimeters: 1.0,
                size_millimeters: crate::model::GeometrySizeRecord {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            },
        };
        RenderedOutput {
            manifest: OutputManifest {
                format: OutputManifest::FORMAT.to_owned(),
                outputs: vec![summary.clone()],
            }
            .canonical_bytes()
            .expect("manifest"),
            outputs: vec![RenderedModelOutput {
                summary,
                glb: Bytes::from_static(glb),
                preview: Bytes::from_static(b"<svg></svg>"),
                projections: TechnicalProjectionImages::all(Bytes::from_static(b"png")),
                shaded: Some(ShadedProjectionImages::all(Bytes::from_static(
                    b"shaded-png",
                ))),
            }],
        }
    }

    #[test]
    fn accepts_only_normal_relative_staging_paths() {
        assert_eq!(
            safe_relative_path("parts/main.py"),
            Ok(PathBuf::from("parts/main.py"))
        );
        for path in ["", ".", "../main.py", "parts/../main.py", "/main.py"] {
            assert_eq!(safe_relative_path(path), Err(()), "accepted {path}");
        }
    }

    #[tokio::test]
    async fn materializes_root_project_export_and_dependency_manifest() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_project_from_files(
                "part",
                "Part",
                vec![
                    ProjectFile {
                        path: "parts/main.py".to_owned(),
                        content: "from faktory_models.m_part import VALUE\nresult = VALUE\n"
                            .to_owned(),
                    },
                    ProjectFile {
                        path: "parts/helper.py".to_owned(),
                        content: "HELPER = True\n".to_owned(),
                    },
                    ProjectFile {
                        path: "faktory_model/__init__.py".to_owned(),
                        content: "VALUE = 123\n".to_owned(),
                    },
                ],
                "parts/main.py".to_owned(),
                vec![],
                "Project hints",
            )
            .await
            .expect("create project");
        let directory = tempfile::tempdir().expect("temp directory");

        let (project_root, entrypoint, dependency_root) = materialize_render_inputs(
            &repository,
            directory.path(),
            &RenderJob {
                model_id: model.id,
                revision: model.desired_source_revision,
            },
        )
        .await
        .expect("materialize render inputs");

        assert_eq!(entrypoint, "parts/main.py");
        assert_eq!(
            tokio::fs::read_to_string(project_root.join("parts/helper.py"))
                .await
                .expect("project helper"),
            "HELPER = True\n"
        );
        assert!(project_root.join("AGENTS.md").is_file());
        assert_eq!(
            tokio::fs::read_to_string(dependency_root.join("faktory_models/m_part/__init__.py"))
                .await
                .expect("root package"),
            "VALUE = 123\n"
        );
        assert!(dependency_root.join("faktory_models/__init__.py").is_file());
        assert!(dependency_root.join("dependencies.json").is_file());
    }

    #[tokio::test]
    async fn validates_complete_glb_v2_chunk_structure() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("model.glb");
        for valid in [glb_fixture(None), glb_fixture(Some(&[1, 2, 3, 0]))] {
            tokio::fs::write(&path, &valid)
                .await
                .expect("write fixture");
            assert_eq!(
                read_glb(&path, valid.len()).await.expect("exact limit"),
                valid
            );
            assert_eq!(
                read_glb(&path, valid.len() - 1).await,
                Err("rendered artifact has invalid size")
            );
            assert_eq!(read_glb(&path, 100).await.expect("valid GLB"), valid);
        }
        assert_eq!(MAX_GLB_BYTES, 64 * 1024 * 1024);

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

    #[test]
    fn enforces_exact_aggregate_bundle_boundary_with_checked_arithmetic() {
        assert_eq!(
            checked_bundle_total(MAX_BUNDLE_BYTES - 1, 1),
            Some(MAX_BUNDLE_BYTES)
        );
        assert_eq!(checked_bundle_total(MAX_BUNDLE_BYTES, 1), None);
        assert_eq!(checked_bundle_total(u64::MAX, 1), None);
        assert_eq!(MAX_BUNDLE_BYTES, 192 * 1024 * 1024);
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

    #[test]
    fn rasterizes_deterministic_bounded_pngs() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="480"><path d="M10 10L100 100" stroke="#000"/></svg>"##;
        let first = rasterize_projection(svg).expect("rasterize SVG");
        let second = rasterize_projection(svg).expect("rasterize SVG again");

        assert_eq!(first, second);
        assert!(first.len() <= MAX_PROJECTION_IMAGE_BYTES);
        assert_eq!(&first[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(first[16..20].try_into().unwrap()), 640);
        assert_eq!(u32::from_be_bytes(first[20..24].try_into().unwrap()), 480);
        let decoded = resvg::tiny_skia::Pixmap::decode_png(&first).expect("decode PNG");
        let background = decoded.pixel(639, 479).expect("background pixel");
        assert_eq!(
            (
                background.red(),
                background.green(),
                background.blue(),
                background.alpha(),
            ),
            (255, 255, 255, 255)
        );
        assert!(rasterize_projection(b"not svg").is_err());

        let mut wrong_dimensions = first.to_vec();
        wrong_dimensions[16..20].copy_from_slice(&639_u32.to_be_bytes());
        assert!(validate_projection_png(&wrong_dimensions).is_err());
        let oversized = vec![0; MAX_PROJECTION_IMAGE_BYTES + 1];
        assert!(validate_projection_png(&oversized).is_err());
        let pseudo_png = pseudo_projection_png();
        assert!(validate_projection_png(&pseudo_png).is_err());
        let transparent = resvg::tiny_skia::Pixmap::new(PROJECTION_WIDTH, PROJECTION_HEIGHT)
            .expect("transparent pixmap")
            .encode_png()
            .expect("encode transparent PNG");
        assert!(validate_projection_png(&transparent).is_err());
    }

    #[test]
    fn rejects_projection_svgs_without_exact_intrinsic_dimensions() {
        for invalid in [
            br#"<svg xmlns="http://www.w3.org/2000/svg"><path/></svg>"#.as_slice(),
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="639" height="480"><path/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="479"><path/></svg>"#,
        ] {
            assert!(rasterize_projection(invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn renderer_receives_all_projection_paths_and_returns_complete_outputs() {
        let directory = tempfile::tempdir().expect("temp directory");
        let script_path = directory.path().join("renderer.sh");
        tokio::fs::write(
            &script_path,
            b"test \"$#\" -eq 4 || exit 2\nmkdir -p \"$4.tmp/outputs/primary/projections\"\nprintf '{\"format\":\"faktory-outputs-v1\",\"outputs\":[{\"output_id\":\"primary\",\"role\":\"assembly\",\"primary\":true,\"facts\":{\"volume_cubic_millimeters\":24.0,\"size_millimeters\":{\"x\":2.0,\"y\":3.0,\"z\":4.0}}}]}\\n' > \"$4.tmp/outputs.json\"\nprintf 'glTF\\002\\000\\000\\000\\060\\000\\000\\000\\034\\000\\000\\000JSON{\"asset\":{\"version\":\"2.0\"}} ' > \"$4.tmp/outputs/primary/model.glb\"\nprintf '<svg></svg>' > \"$4.tmp/outputs/primary/preview.svg\"\nprintf '{\"volume_cubic_millimeters\":24,\"size_millimeters\":{\"x\":2,\"y\":3,\"z\":4}}' > \"$4.tmp/outputs/primary/facts.json\"\nfor name in isometric front back left right top bottom; do printf '<svg width=\"640\" height=\"480\"></svg>' > \"$4.tmp/outputs/primary/projections/$name.svg\"; done\nmv \"$4.tmp\" \"$4\"\n",
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
            &visual(),
            &RenderJob {
                model_id: model.id,
                revision: model.desired_source_revision,
            },
        )
        .await
        .expect("render outputs");
        assert_eq!(output.outputs[0].glb, glb_fixture(None));
        for projection in TechnicalProjection::ALL {
            validate_projection_png(output.outputs[0].projections.get(projection))
                .expect("valid PNG");
        }
        assert_eq!(
            output.outputs[0].preview,
            Bytes::from_static(b"<svg></svg>")
        );
        assert!(
            (output.outputs[0].summary.facts.volume_cubic_millimeters - 24.0).abs() < f64::EPSILON
        );
    }

    #[tokio::test]
    async fn python_worker_bundle_is_consumed_by_rust_reader() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let model = repository
            .create_model(
                "python-worker",
                "Python worker",
                b"import cadquery as cq\nresult = cq.Workplane(\"XY\").box(2, 3, 4)\n",
            )
            .await
            .expect("create model");
        let output = render(
            &repository,
            &RenderConfig {
                command: vec![
                    "uv".to_owned(),
                    "run".to_owned(),
                    "--directory".to_owned(),
                    workspace.to_string_lossy().into_owned(),
                    "python".to_owned(),
                    "-m".to_owned(),
                    "renderer".to_owned(),
                ],
                queue_capacity: 1,
                concurrency: 1,
                timeout: Duration::from_secs(30),
                max_output_bytes: 64 * 1024 * 1024,
            },
            &visual(),
            &RenderJob {
                model_id: model.id,
                revision: model.desired_source_revision,
            },
        )
        .await
        .expect("consume Python worker bundle");

        assert_eq!(output.outputs.len(), 1);
        assert_eq!(output.outputs[0].summary.output_id, "primary");
        assert_eq!(
            output.outputs[0].summary.role,
            crate::model::OutputRoleRecord::Part
        );
        for projection in TechnicalProjection::ALL {
            validate_projection_png(output.outputs[0].projections.get(projection))
                .expect("valid projection PNG");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn visual_failure_fails_desired_render_and_preserves_last_good_revision() {
        let directory = tempfile::tempdir().expect("temp directory");
        let script_path = directory.path().join("renderer.sh");
        tokio::fs::write(
            &script_path,
            b"mkdir -p \"$4.tmp/outputs/primary/projections\"\nprintf '{\"format\":\"faktory-outputs-v1\",\"outputs\":[{\"output_id\":\"primary\",\"role\":\"assembly\",\"primary\":true,\"facts\":{\"volume_cubic_millimeters\":24.0,\"size_millimeters\":{\"x\":2.0,\"y\":3.0,\"z\":4.0}}}]}\\n' > \"$4.tmp/outputs.json\"\nprintf 'glTF\\002\\000\\000\\000\\060\\000\\000\\000\\034\\000\\000\\000JSON{\"asset\":{\"version\":\"2.0\"}} ' > \"$4.tmp/outputs/primary/model.glb\"\nprintf '<svg></svg>' > \"$4.tmp/outputs/primary/preview.svg\"\nprintf '{\"volume_cubic_millimeters\":24,\"size_millimeters\":{\"x\":2,\"y\":3,\"z\":4}}' > \"$4.tmp/outputs/primary/facts.json\"\nfor name in isometric front back left right top bottom; do printf '<svg width=\"640\" height=\"480\"></svg>' > \"$4.tmp/outputs/primary/projections/$name.svg\"; done\nmv \"$4.tmp\" \"$4\"\n",
        )
        .await
        .expect("write renderer");
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
        let first = repository
            .create_model("part", "Part", b"first")
            .await
            .expect("create model");
        repository
            .complete_render(
                &first.id,
                &first.desired_source_revision,
                rendered_output(b"old"),
            )
            .await
            .expect("complete old render");
        let edited = repository
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
            .expect("edit model")
            .record;
        let config = RenderConfig {
            command: vec![
                "/bin/sh".to_owned(),
                script_path.to_string_lossy().into_owned(),
            ],
            queue_capacity: 1,
            concurrency: 1,
            timeout: Duration::from_secs(1),
            max_output_bytes: 1024,
        };
        let unavailable =
            VisualCoordinator::new(Arc::new(UnavailableVisualRenderer), Duration::from_secs(1));
        run_job(
            &repository,
            &config,
            &unavailable,
            &RenderJob {
                model_id: edited.id.clone(),
                revision: edited.desired_source_revision,
            },
        )
        .await;

        let failed = repository.get_model("part").await.expect("model").record;
        assert_eq!(failed.render_state, crate::model::StoredRenderState::Failed);
        assert_eq!(
            failed.current_successful_source_revision,
            first.desired_source_revision
        );
        assert_eq!(
            repository
                .geometry("part", &first.desired_source_revision)
                .await
                .expect("last-good geometry"),
            Bytes::from_static(b"old")
        );
    }

    #[tokio::test]
    async fn render_claim_exhaustion_degrades_readiness_and_logs_only_error_kind() {
        let store = Arc::new(TransientPutFailures::default());
        let repository = Repository::new(store.clone(), 1);
        let readiness_repository = repository.clone();
        let model = repository
            .create_model(PRIVATE_SENTINEL, "Part", b"source")
            .await
            .expect("accept source");
        let job = RenderJob {
            model_id: model.id,
            revision: model.desired_source_revision,
        };
        store.attempts.store(0, Ordering::SeqCst);
        store.remaining.store(5, Ordering::SeqCst);
        let output = RecordingWriter::default();

        let claimed = claim_render(&repository, &job)
            .with_subscriber(warning_subscriber(&output))
            .await;

        assert!(!claimed);
        assert_eq!(store.attempts.load(Ordering::SeqCst), 5);
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
            crate::model::StoredRenderState::Pending
        );
        assert_repository_warning(
            &output,
            "render claim remains recoverable only by startup reconciliation",
        );
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
                &crate::model::output_geometry_key(
                    &replacement.id,
                    &replacement.desired_source_revision,
                    "primary",
                ),
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
        assert_eq!(
            failed.current_successful_facts,
            Some(first_output.outputs[0].summary.facts)
        );
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
    async fn terminal_state_write_exhaustion_degrades_readiness_and_logs_only_error_kind() {
        let store = Arc::new(TransientPutFailures::default());
        let repository = Repository::new(store.clone(), 1);
        let readiness_repository = repository.clone();
        let model = repository
            .create_model(PRIVATE_SENTINEL, "Part", b"source")
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
        store.attempts.store(0, Ordering::SeqCst);
        store.remaining.store(5, Ordering::SeqCst);
        let output = RecordingWriter::default();

        persist_terminal(&repository, &job, &Err("safe failure"))
            .with_subscriber(warning_subscriber(&output))
            .await;

        assert_eq!(store.attempts.load(Ordering::SeqCst), 5);
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
        assert_repository_warning(
            &output,
            "render terminal state remains recoverable only by startup reconciliation",
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
            render(&repository, &config, &visual(), &job).await,
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
