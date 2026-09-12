//! One asynchronous screenshot at a time, including GPU readback and PNG writing.
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Resource, Default)]
pub(super) struct NativeCapture {
    pending: Option<Pending>,
    failed: bool,
}

struct Pending {
    entity: Entity,
    path: PathBuf,
    started: Instant,
    write: Option<Task<Result<(), String>>>,
}

impl NativeCapture {
    pub(super) fn request(&mut self, commands: &mut Commands, path: PathBuf) -> bool {
        if self.pending.is_some() || self.failed {
            return false;
        }
        let entity = commands
            .spawn(Screenshot::primary_window())
            .observe(captured)
            .id();
        self.pending = Some(Pending {
            entity,
            path,
            started: Instant::now(),
            write: None,
        });
        true
    }

    pub(super) fn poll(&mut self) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if let Some(result) = pending
            .write
            .as_mut()
            .and_then(|task| block_on(poll_once(task)))
        {
            match result {
                Ok(()) => info!("Screenshot saved to {}", pending.path.display()),
                Err(error) => {
                    error!("Cannot save screenshot {}: {error}", pending.path.display());
                    self.failed = true;
                }
            }
            self.pending = None;
        } else if pending.started.elapsed() >= CAPTURE_TIMEOUT && !self.failed {
            // Keep the pending slot occupied: a stalled writer must never permit
            // another capture to allocate a second image or queue another task.
            error!(
                "Screenshot capture/write timed out: {}",
                pending.path.display()
            );
            self.failed = true;
        }
    }

    pub(super) fn ready_to_exit(&self) -> bool {
        self.pending.is_none() || self.failed
    }

    pub(super) fn failed(&self) -> bool {
        self.failed
    }
}

fn captured(event: On<ScreenshotCaptured>, mut capture: ResMut<NativeCapture>) {
    let Some(pending) = capture.pending.as_mut() else {
        return;
    };
    if pending.entity != event.entity || pending.write.is_some() {
        return;
    }
    let image = event.image.clone();
    let path = pending.path.clone();
    pending.write = Some(AsyncComputeTaskPool::get().spawn(async move { save(image, path) }));
}

fn save(image: Image, path: PathBuf) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    // Match Bevy's save_to_disk: HDR alpha contains brightness, so export RGB.
    image
        .try_into_dynamic()
        .map_err(|error| error.to_string())?
        .to_rgb8()
        .save(path)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_capture_blocks_duplicates_and_timeout_fails_exit() {
        let mut world = World::new();
        let mut capture = NativeCapture::default();
        assert!(capture.request(&mut world.commands(), PathBuf::from("unused.png")));
        assert!(!capture.request(&mut world.commands(), PathBuf::from("duplicate.png")));
        assert!(!capture.ready_to_exit());
        capture.pending.as_mut().unwrap().started = Instant::now() - CAPTURE_TIMEOUT;
        capture.poll();
        assert!(capture.failed());
        assert!(capture.ready_to_exit());
        assert!(!capture.request(&mut world.commands(), PathBuf::from("after-timeout.png")));
    }

    #[test]
    fn completed_writer_releases_slot_and_reports_errors() {
        let pool = bevy::tasks::TaskPool::new();
        for success in [true, false] {
            let mut capture = NativeCapture {
                pending: Some(Pending {
                    entity: Entity::PLACEHOLDER,
                    path: PathBuf::from("test.png"),
                    started: Instant::now(),
                    write: Some(pool.spawn(async move {
                        if success {
                            Ok(())
                        } else {
                            Err("test write error".into())
                        }
                    })),
                }),
                failed: false,
            };
            while capture.pending.is_some() {
                capture.poll();
                assert!(
                    capture
                        .pending
                        .as_ref()
                        .is_none_or(|p| p.started.elapsed() < Duration::from_secs(5))
                );
                std::thread::yield_now();
            }
            assert!(capture.ready_to_exit());
            assert_eq!(capture.failed(), !success);
        }
    }
}
