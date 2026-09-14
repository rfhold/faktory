//! Optional bounded Pyroscope CPU profiling.

use std::{
    io,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use pyroscope::{
    backend::{BackendConfig, PprofConfig, pprof_backend},
    pyroscope::{PyroscopeAgent, PyroscopeAgentBuilder, PyroscopeAgentRunning},
};

use crate::observability::{SERVICE_NAME, SERVICE_NAMESPACE, TelemetryConfig};

const SAMPLE_RATE: u32 = 100;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanupStatus {
    Completed,
    Failed,
    TimedOut,
}

pub struct ProfilingGuard {
    agent: Option<PyroscopeAgent<PyroscopeAgentRunning>>,
}

impl std::fmt::Debug for ProfilingGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProfilingGuard")
            .field("enabled", &self.agent.is_some())
            .finish()
    }
}

impl ProfilingGuard {
    pub fn shutdown(mut self) {
        tracing::info!("pyroscope profiler shutdown started");
        let status = self.agent.take().map_or(CleanupStatus::Completed, |agent| {
            cleanup_with_timeout(move || cleanup_agent(agent), SHUTDOWN_TIMEOUT)
        });
        match status {
            CleanupStatus::Completed => tracing::info!(
                profiler.shutdown.outcome = "completed",
                "pyroscope profiler shutdown completed"
            ),
            CleanupStatus::Failed => tracing::warn!(
                profiler.shutdown.outcome = "failed",
                "pyroscope profiler shutdown failed"
            ),
            CleanupStatus::TimedOut => tracing::warn!(
                profiler.shutdown.outcome = "timed_out",
                "pyroscope profiler shutdown timed out"
            ),
        }
    }
}

impl Drop for ProfilingGuard {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.take() {
            detach_cleanup(move || cleanup_agent(agent));
        }
    }
}

fn cleanup_agent(agent: PyroscopeAgent<PyroscopeAgentRunning>) -> Result<(), ()> {
    let agent = agent.stop().map_err(|_| ())?;
    agent.shutdown();
    Ok(())
}

fn cleanup_with_timeout<F>(cleanup: F, timeout: Duration) -> CleanupStatus
where
    F: FnOnce() -> Result<(), ()> + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    let Ok(handle) = thread::Builder::new()
        .name("pyroscope-shutdown".to_owned())
        .spawn(move || {
            let status = match cleanup() {
                Ok(()) => CleanupStatus::Completed,
                Err(()) => CleanupStatus::Failed,
            };
            let _ = sender.send(status);
        })
    else {
        return CleanupStatus::Failed;
    };

    match receiver.recv_timeout(timeout) {
        Ok(status) => match handle.join() {
            Ok(()) => status,
            Err(_) => CleanupStatus::Failed,
        },
        Err(RecvTimeoutError::Disconnected) => {
            drop(handle.join());
            CleanupStatus::Failed
        }
        Err(RecvTimeoutError::Timeout) => {
            drop(handle);
            CleanupStatus::TimedOut
        }
    }
}

fn detach_cleanup<F>(cleanup: F)
where
    F: FnOnce() -> Result<(), ()> + Send + 'static,
{
    drop(
        thread::Builder::new()
            .name("pyroscope-cleanup".to_owned())
            .spawn(move || {
                let _ = cleanup();
            }),
    );
}

pub fn init(
    config: &TelemetryConfig,
) -> Result<ProfilingGuard, Box<dyn std::error::Error + Send + Sync>> {
    let Some(url) = config.pyroscope_url.as_ref() else {
        tracing::info!(pyroscope.enabled = false, "pyroscope profiler disabled");
        return Ok(ProfilingGuard { agent: None });
    };
    let agent = PyroscopeAgentBuilder::new(
        url.as_str(),
        SERVICE_NAME,
        SAMPLE_RATE,
        "pyroscope-rs",
        env!("CARGO_PKG_VERSION"),
        pprof_backend(PprofConfig::default(), BackendConfig::default()),
    )
    .tags(profiling_tags(config))
    .build()
    .map_err(|_| io::Error::other("failed to initialize Pyroscope profiler"))?
    .start()
    .map_err(|_| io::Error::other("failed to start Pyroscope profiler"))?;

    tracing::info!(
        pyroscope.enabled = true,
        profile.type = "cpu",
        sample_rate_hz = SAMPLE_RATE,
        "pyroscope profiler initialized"
    );
    Ok(ProfilingGuard { agent: Some(agent) })
}

fn profiling_tags(config: &TelemetryConfig) -> Vec<(&'static str, &str)> {
    let mut tags = vec![
        ("service_namespace", SERVICE_NAMESPACE),
        (
            "deployment_environment_name",
            config.deployment_environment.as_str(),
        ),
    ];
    if let Some(namespace) = config.k8s_namespace.as_deref() {
        tags.push(("namespace", namespace));
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn cleanup_is_bounded_and_reports_sanitized_status() {
        assert_eq!(
            cleanup_with_timeout(|| Ok(()), Duration::from_secs(1)),
            CleanupStatus::Completed
        );
        assert_eq!(
            cleanup_with_timeout(|| Err(()), Duration::from_secs(1)),
            CleanupStatus::Failed
        );
        let (release, blocked) = mpsc::channel();
        let started = Instant::now();
        assert_eq!(
            cleanup_with_timeout(
                move || blocked.recv().map_err(|_| ()),
                Duration::from_millis(10),
            ),
            CleanupStatus::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(release);
    }

    #[test]
    fn tags_are_stable_and_exclude_pod_identity() {
        let config = TelemetryConfig {
            deployment_environment: "preview".to_owned(),
            k8s_namespace: Some("faktory-system".to_owned()),
            k8s_pod_name: Some("faktory-abc".to_owned()),
            k8s_pod_uid: Some("pod-uid".to_owned()),
            pyroscope_url: None,
        };
        assert_eq!(
            profiling_tags(&config),
            vec![
                ("service_namespace", "faktory"),
                ("deployment_environment_name", "preview"),
                ("namespace", "faktory-system"),
            ]
        );
    }
}
